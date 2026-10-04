use super::{
    WorkspaceService,
    search_helpers::{
        append_after_context, compile_search, drain_ready, flush_pending, include_matches,
        open_text_file, truncate_utf8,
    },
    search_state::{
        cleanup_expired, evict_oldest, has_next_file, next_file_entry, owner_key, push_warning,
        root_version, validate_state,
    },
    walk::configured_walker,
};
use crate::{
    BudgetTracker, FsSearchMatch, FsSearchPageData, FsSearchRequest, FsSearchScanPage,
    OperationContext, ProgressLimiter, RuntimeError, RuntimeResult, ToolBudget, TraversalOptions,
    TruncationReason,
};
use globset::GlobSet;
use ignore::Walk;
use regex::Regex;
use std::{
    collections::{HashMap, VecDeque},
    fs::File,
    io::BufReader,
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};
use uuid::Uuid;

#[path = "filesystem_search_file.rs"]
mod file;
#[path = "filesystem_search_scan.rs"]
mod scan;
use scan::scan_page;

const SEARCH_STATE_TTL: Duration = Duration::from_secs(15 * 60);
const MAX_ACTIVE_SEARCH_STATES: usize = 128;
const HARD_SEARCH_TIMEOUT: Duration = Duration::from_secs(60);
const HARD_SEARCH_FILES: u64 = 1_000_000;
const HARD_SEARCH_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const HARD_SEARCH_OUTPUT: u64 = 4 * 1024 * 1024;
const HARD_SEARCH_FILE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct SearchProgress {
    pub path: PathBuf,
    pub files_scanned: u64,
    pub bytes_scanned: u64,
    pub matches_found: usize,
}

pub(super) struct SearchStateStore {
    states: Mutex<HashMap<String, SearchState>>,
}

impl Default for SearchStateStore {
    fn default() -> Self {
        Self {
            states: Mutex::new(HashMap::new()),
        }
    }
}

pub(super) struct SearchState {
    pub(super) root: PathBuf,
    pub(super) root_version: String,
    pub(super) owner: String,
    pub(super) request_fingerprint: String,
    pub(super) source: SearchSource,
    pub(super) pending_entry: Option<SearchCandidate>,
    pub(super) index_generation: Option<u64>,
    pub(super) index_freshness: crate::IndexFreshness,
    pub(super) stale_entries_detected: u64,
    current_file: Option<FileScanState>,
    pub(super) expires_at: Instant,
}

pub(super) enum SearchSource {
    Direct(Box<Walk>),
    Indexed {
        entries: Vec<super::repository_index::IndexedPathCandidate>,
        position: usize,
    },
}

#[derive(Clone)]
pub(super) struct SearchCandidate {
    pub(super) path: PathBuf,
    pub(super) indexed: Option<super::repository_index::IndexedPathCandidate>,
}

struct FileScanState {
    path: PathBuf,
    reader: Option<BufReader<File>>,
    identity: super::FileIdentity,
    line_number: u64,
    byte_offset: u64,
    context_before: VecDeque<String>,
    pending: Vec<PendingMatch>,
    ready: VecDeque<FsSearchMatch>,
    matches_in_file: usize,
    invalid_utf8_reported: bool,
}

pub(super) struct PendingMatch {
    pub(super) value: FsSearchMatch,
    pub(super) after_remaining: usize,
}

pub(super) struct CompiledSearch {
    pub(super) regex: Regex,
    pub(super) includes: Option<GlobSet>,
}

impl WorkspaceService {
    pub async fn search_v2(
        &self,
        context: &OperationContext,
        request: &FsSearchRequest,
        state_id: Option<&str>,
        expected_root_version: Option<&str>,
        progress: impl Fn(SearchProgress) + Send + Sync + 'static,
    ) -> RuntimeResult<(FsSearchScanPage, Option<String>)> {
        let memory = request.budget.max_output_bytes.min(64 * 1024 * 1024);
        let _admission = self.admission.try_admit(&context.agent_id, 1, memory)?;
        let root = self.existing(&request.path)?;
        root.revalidate()?;
        let mut request = request.clone();
        request.budget.timeout_ms = request.budget.timeout_ms.min(60_000);
        request.budget.max_files_scanned =
            request.budget.max_files_scanned.clamp(1, HARD_SEARCH_FILES);
        request.budget.max_bytes_scanned =
            request.budget.max_bytes_scanned.clamp(1, HARD_SEARCH_BYTES);
        request.budget.max_output_bytes =
            request.budget.max_output_bytes.clamp(1, HARD_SEARCH_OUTPUT);
        request.budget.max_file_bytes = request
            .budget
            .max_file_bytes
            .clamp(1, HARD_SEARCH_FILE_BYTES);
        let tracker = BudgetTracker::new(
            context.cancellation.clone(),
            ToolBudget::intersect([
                &ToolBudget {
                    deadline: Some(Instant::now() + HARD_SEARCH_TIMEOUT),
                    ..ToolBudget::default()
                },
                &ToolBudget {
                    deadline: Some(
                        Instant::now() + Duration::from_millis(request.budget.timeout_ms),
                    ),
                    ..ToolBudget::default()
                },
            ]),
        );
        let owner = owner_key(context);
        let store = self.search_states.clone();
        let service = self.clone();
        let state_id = state_id.map(str::to_owned);
        let expected_root_version = expected_root_version.map(str::to_owned);

        tokio::task::spawn_blocking(move || {
            let compiled = compile_search(&request)?;
            let version = root_version(&root)?;
            let fingerprint = serde_json::to_string(&request)
                .map_err(|error| RuntimeError::new("search_request_invalid", error.to_string()))?;
            let mut states = store.states.lock().map_err(|_| {
                RuntimeError::new("search_state_poisoned", "search state lock is poisoned")
            })?;
            cleanup_expired(&mut states);
            let continuing = state_id.is_some();
            let (id, mut state) = if let Some(id) = state_id {
                let state = states.remove(&id).ok_or_else(|| {
                    RuntimeError::new(
                        "cursor_expired",
                        "search cursor state expired; restart search",
                    )
                })?;
                validate_state(
                    &state,
                    &root,
                    &version,
                    &owner,
                    &fingerprint,
                    expected_root_version.as_deref(),
                )?;
                if let Some(expected_generation) = state.index_generation {
                    let status = service.index_status(&root)?;
                    if !status.available
                        || status.freshness != crate::IndexFreshness::Fresh
                        || status.generation != expected_generation
                    {
                        return Err(RuntimeError::new(
                            "cursor_stale",
                            "repository index generation changed after cursor issuance; restart search",
                        ));
                    }
                }
                (id, state)
            } else {
                let indexed = if !request.include_ignored && request.exclude.is_empty() {
                    service.fresh_index_candidates_where(&root, |path, entry_type| {
                        entry_type == "file"
                            && include_matches(&root, path, compiled.includes.as_ref())
                    })?
                } else {
                    None
                };
                let (source, index_generation, index_freshness) = if let Some(indexed) = indexed {
                    (
                        SearchSource::Indexed {
                            entries: indexed.entries,
                            position: 0,
                        },
                        Some(indexed.generation),
                        indexed.freshness,
                    )
                } else {
                    let walker = configured_walker(
                        &root,
                        &TraversalOptions {
                            include_hidden: true,
                            include_ignored: request.include_ignored,
                            exclude: request.exclude.clone(),
                            ..TraversalOptions::default()
                        },
                    )?
                    .build();
                    (
                        SearchSource::Direct(Box::new(walker)),
                        None,
                        crate::IndexFreshness::Unknown,
                    )
                };
                (
                    Uuid::new_v4().to_string(),
                    SearchState {
                        root: root.to_path_buf(),
                        root_version: version.clone(),
                        owner,
                        request_fingerprint: fingerprint,
                        source,
                        pending_entry: None,
                        index_generation,
                        index_freshness,
                        stale_entries_detected: 0,
                        current_file: None,
                        expires_at: Instant::now() + SEARCH_STATE_TTL,
                    },
                )
            };
            drop(states);

            if let Some(file) = state.current_file.as_mut() {
                file.resume(&service)?;
            }
            let page = match scan_page(&mut state, &request, &compiled, &tracker, &progress) {
                Ok(page) => page,
                Err(error) if error.code == "index_stale_detected" => {
                    service.mark_index_stale(&root);
                    if continuing {
                        return Err(RuntimeError::new(
                            "cursor_stale",
                            "repository index changed after cursor issuance; restart search",
                        ));
                    }
                    let walker = configured_walker(
                        &root,
                        &TraversalOptions {
                            include_hidden: true,
                            include_ignored: request.include_ignored,
                            exclude: request.exclude.clone(),
                            ..TraversalOptions::default()
                        },
                    )?
                    .build();
                    state.source = SearchSource::Direct(Box::new(walker));
                    state.pending_entry = None;
                    state.index_freshness = crate::IndexFreshness::Stale;
                    scan_page(&mut state, &request, &compiled, &tracker, &progress)?
                }
                Err(error) => return Err(error),
            };
            if page.has_more {
                // Cursor state must not retain a file handle between requests.
                if let Some(file) = state.current_file.as_mut() {
                    file.suspend();
                }
                state.expires_at = Instant::now() + SEARCH_STATE_TTL;
                let mut states = store.states.lock().map_err(|_| {
                    RuntimeError::new("search_state_poisoned", "search state lock is poisoned")
                })?;
                cleanup_expired(&mut states);
                if states.len() >= MAX_ACTIVE_SEARCH_STATES {
                    evict_oldest(&mut states);
                }
                states.insert(id.clone(), state);
                Ok((page, Some(id)))
            } else {
                Ok((page, None))
            }
        })
        .await
        .map_err(super::join_error)?
    }
}
