use super::*;
use std::{io::BufRead, time::UNIX_EPOCH};

fn indexed_candidate_matches_live(
    candidate: &super::super::repository_index::IndexedPathCandidate,
) -> RuntimeResult<bool> {
    let metadata = match std::fs::symlink_metadata(&candidate.path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(super::super::io_error(error)),
    };
    let modified_at_ns = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |value| value.as_nanos());
    let entry_type = if metadata.file_type().is_symlink() {
        "symlink"
    } else if metadata.is_dir() {
        "directory"
    } else {
        "file"
    };
    Ok(metadata.len() == candidate.size
        && modified_at_ns == candidate.modified_at_ns
        && entry_type == candidate.entry_type)
}

pub(super) fn scan_page(
    state: &mut SearchState,
    request: &FsSearchRequest,
    compiled: &CompiledSearch,
    tracker: &BudgetTracker,
    progress: &impl Fn(SearchProgress),
) -> RuntimeResult<FsSearchScanPage> {
    let limit = request.limit.clamp(1, 5_000);
    let max_files = request.budget.max_files_scanned.clamp(1, HARD_SEARCH_FILES);
    let max_bytes = request.budget.max_bytes_scanned.clamp(1, HARD_SEARCH_BYTES);
    let max_output = request.budget.max_output_bytes.clamp(1, HARD_SEARCH_OUTPUT);
    let mut matches = Vec::with_capacity(limit.min(256));
    let mut files_scanned = 0_u64;
    let mut bytes_scanned = 0_u64;
    let mut output_bytes = 0_u64;
    let mut files_skipped_by_size = 0_u64;
    let mut binary_files_skipped = 0_u64;
    let mut errors_skipped = 0_u64;
    let mut warnings = Vec::new();
    let mut truncation_reason = None;
    let progress_limiter = ProgressLimiter::new(10, 100);

    loop {
        if let Err(error) = tracker.checkpoint() {
            truncation_reason = Some(if error.code == "operationCancelled" {
                TruncationReason::Cancelled
            } else {
                TruncationReason::TimeBudget
            });
            break;
        }
        if files_scanned >= max_files && state.current_file.is_none() {
            truncation_reason = Some(TruncationReason::FileBudget);
            break;
        }
        if bytes_scanned >= max_bytes {
            truncation_reason = Some(TruncationReason::ByteBudget);
            break;
        }
        if matches.len() >= limit {
            if let Some(file) = state.current_file.as_mut() {
                if file.ready.is_empty()
                    && file.pending.is_empty()
                    && file
                        .reader
                        .as_mut()
                        .expect("active scan reader")
                        .fill_buf()
                        .map_err(super::super::io_error)?
                        .is_empty()
                {
                    state.current_file = None;
                }
            }
            if state.current_file.is_some() || has_next_file(state, &mut warnings)? {
                truncation_reason = Some(TruncationReason::ItemLimit);
            }
            break;
        }
        if let Some(file) = state.current_file.as_mut() {
            if !drain_ready(
                &mut file.ready,
                &mut matches,
                &mut output_bytes,
                max_output,
                limit,
            )? {
                truncation_reason = Some(if matches.len() >= limit {
                    TruncationReason::ItemLimit
                } else {
                    TruncationReason::OutputLimit
                });
                break;
            }
        }

        if state.current_file.is_none() {
            let Some(entry) = next_file_entry(state, &mut warnings)? else {
                break;
            };
            if let Some(indexed) = entry.indexed.as_ref()
                && !indexed_candidate_matches_live(indexed)?
            {
                state.stale_entries_detected = state.stale_entries_detected.saturating_add(1);
                return Err(RuntimeError::new(
                    "index_stale_detected",
                    "repository index changed during search; retry with direct traversal",
                ));
            }
            if !include_matches(&state.root, &entry.path, compiled.includes.as_ref()) {
                continue;
            }
            let metadata = match std::fs::symlink_metadata(&entry.path) {
                Ok(value) => value,
                Err(error) => {
                    errors_skipped += 1;
                    push_warning(
                        &mut warnings,
                        "filesystem_metadata_error",
                        error.to_string(),
                    );
                    continue;
                }
            };
            if metadata.len() > request.budget.max_file_bytes {
                files_skipped_by_size += 1;
                continue;
            }
            match open_text_file(&entry.path) {
                Ok(Some(file)) => {
                    files_scanned += 1;
                    tracker.record_files(1);
                    state.current_file = Some(FileScanState {
                        path: entry.path.clone(),
                        identity: super::super::FileIdentity::from_metadata(
                            &file.metadata().map_err(super::super::io_error)?,
                        ),
                        reader: Some(BufReader::with_capacity(64 * 1024, file)),
                        line_number: 0,
                        byte_offset: 0,
                        context_before: VecDeque::with_capacity(request.context_before.min(100)),
                        pending: Vec::new(),
                        ready: VecDeque::new(),
                        matches_in_file: 0,
                        invalid_utf8_reported: false,
                    });
                }
                Ok(None) => {
                    binary_files_skipped += 1;
                    continue;
                }
                Err(error) => {
                    errors_skipped += 1;
                    push_warning(&mut warnings, "filesystem_read_error", error.to_string());
                    continue;
                }
            }
        }

        let file = state.current_file.as_mut().expect("file initialized above");
        let mut raw = Vec::new();
        let read = match file
            .reader
            .as_mut()
            .expect("active scan reader")
            .read_until(b'\n', &mut raw)
        {
            Ok(value) => value,
            Err(error) => {
                errors_skipped += 1;
                push_warning(&mut warnings, "filesystem_read_error", error.to_string());
                state.current_file = None;
                continue;
            }
        };
        if read == 0 {
            flush_pending(&mut file.pending, &mut file.ready, true);
            if !drain_ready(
                &mut file.ready,
                &mut matches,
                &mut output_bytes,
                max_output,
                limit,
            )? {
                truncation_reason = Some(if matches.len() >= limit {
                    TruncationReason::ItemLimit
                } else {
                    TruncationReason::OutputLimit
                });
                break;
            }
            state.current_file = None;
            continue;
        }
        bytes_scanned = bytes_scanned.saturating_add(read as u64);
        tracker.record_read_bytes(read as u64);
        file.line_number += 1;
        let line_start = file.byte_offset;
        file.byte_offset = file.byte_offset.saturating_add(read as u64);

        let had_newline = raw.last() == Some(&b'\n');
        if had_newline {
            raw.pop();
        }
        if raw.last() == Some(&b'\r') {
            raw.pop();
        }
        let bom_skip = if file.line_number == 1 && raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
            3
        } else {
            0
        };
        let line_bytes = &raw[bom_skip..];
        let line = match std::str::from_utf8(line_bytes) {
            Ok(value) => value.to_owned(),
            Err(_) => {
                if !file.invalid_utf8_reported {
                    errors_skipped += 1;
                    push_warning(
                        &mut warnings,
                        "invalid_utf8_lossy",
                        format!(
                            "{} contains invalid UTF-8; replacement characters were used",
                            file.path.display()
                        ),
                    );
                    file.invalid_utf8_reported = true;
                }
                String::from_utf8_lossy(line_bytes).into_owned()
            }
        };

        let (context_line, _) = truncate_utf8(&line, request.max_snippet_bytes.max(64));
        append_after_context(&mut file.pending, &context_line);
        flush_pending(&mut file.pending, &mut file.ready, false);
        if !drain_ready(
            &mut file.ready,
            &mut matches,
            &mut output_bytes,
            max_output,
            limit,
        )? {
            truncation_reason = Some(if matches.len() >= limit {
                TruncationReason::ItemLimit
            } else {
                TruncationReason::OutputLimit
            });
            break;
        }

        if file.matches_in_file < request.max_matches_per_file.max(1) {
            for found in compiled.regex.find_iter(&line) {
                if file.matches_in_file >= request.max_matches_per_file.max(1) {
                    break;
                }
                let (line_text, line_truncated) =
                    truncate_utf8(&line, request.max_snippet_bytes.max(64));
                let match_text = line[found.start()..found.end()].to_owned();
                let value = FsSearchMatch {
                    path: file.path.to_string_lossy().into_owned(),
                    line: file.line_number,
                    column: line[..found.start()].chars().count() as u64 + 1,
                    byte_offset: line_start + bom_skip as u64 + found.start() as u64,
                    match_start: found.start() as u64,
                    match_end: found.end() as u64,
                    match_text,
                    line_text,
                    context_before: file.context_before.iter().cloned().collect(),
                    context_after: Vec::new(),
                    line_truncated,
                };
                file.matches_in_file += 1;
                if request.context_after == 0 {
                    file.ready.push_back(value);
                } else {
                    file.pending.push(PendingMatch {
                        value,
                        after_remaining: request.context_after.min(100),
                    });
                }
            }
        }
        if !drain_ready(
            &mut file.ready,
            &mut matches,
            &mut output_bytes,
            max_output,
            limit,
        )? {
            truncation_reason = Some(if matches.len() >= limit {
                TruncationReason::ItemLimit
            } else {
                TruncationReason::OutputLimit
            });
            break;
        }

        if request.context_before > 0 {
            let (snippet, _) = truncate_utf8(&line, request.max_snippet_bytes.max(64));
            file.context_before.push_back(snippet);
            while file.context_before.len() > request.context_before.min(100) {
                file.context_before.pop_front();
            }
        }

        if progress_limiter.should_emit(false) {
            progress(SearchProgress {
                path: file.path.clone(),
                files_scanned,
                bytes_scanned,
                matches_found: matches.len() + file.pending.len(),
            });
        }
    }

    if progress_limiter.should_emit(true) {
        progress(SearchProgress {
            path: state
                .current_file
                .as_ref()
                .map_or_else(|| state.root.clone(), |file| file.path.clone()),
            files_scanned,
            bytes_scanned,
            matches_found: matches.len(),
        });
    }

    let has_more = truncation_reason.is_some()
        || state.current_file.is_some()
        || has_next_file(state, &mut warnings)?;
    Ok(FsSearchScanPage {
        data: FsSearchPageData {
            matches,
            files_skipped_by_size,
            binary_files_skipped,
            errors_skipped,
            index_used: matches!(state.source, SearchSource::Indexed { .. }),
            index_generation: state.index_generation,
            index_freshness: state.index_freshness,
            stale_entries_detected: state.stale_entries_detected,
        },
        has_more,
        files_scanned,
        bytes_scanned,
        truncation_reason,
        warnings,
        root_version: state.root_version.clone(),
    })
}
