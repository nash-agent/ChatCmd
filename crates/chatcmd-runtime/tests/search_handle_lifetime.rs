use chatcmd_runtime::{
    ApprovalDecision, BoxFuture, ExecutionPolicy, FsSearchBudget, FsSearchRequest,
    OperationContext, PolicyDecision, PolicyEngine, RuntimeResult, SearchMode, WorkspaceService,
};
use std::{collections::BTreeMap, fs, sync::Arc};

struct Approve;
impl ApprovalDecision for Approve {
    fn request<'a>(
        &'a self,
        _: &'a chatcmd_runtime::PolicyContext,
    ) -> BoxFuture<'a, RuntimeResult<bool>> {
        Box::pin(async { Ok(true) })
    }
}

fn fixture() -> (
    tempfile::TempDir,
    WorkspaceService,
    FsSearchRequest,
    OperationContext,
) {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("sample.txt"),
        "needle 1\nneedle 2\nneedle 3\n",
    )
    .unwrap();
    let policy = PolicyEngine::new(
        Some(ExecutionPolicy {
            default: PolicyDecision::Allow,
            per_agent_tool: BTreeMap::new(),
            per_root: BTreeMap::new(),
        }),
        Arc::new(Approve),
    );
    let workspace = WorkspaceService::new(&[directory.path().to_owned()], policy).unwrap();
    let request = FsSearchRequest {
        path: directory.path().to_owned(),
        query: "needle".to_owned(),
        mode: SearchMode::Literal,
        case_sensitive: true,
        word_boundary: false,
        include: Vec::new(),
        exclude: Vec::new(),
        include_ignored: true,
        context_before: 0,
        context_after: 0,
        max_matches_per_file: 10,
        limit: 1,
        max_snippet_bytes: 1024,
        budget: FsSearchBudget::default(),
    };
    let context = OperationContext::new("search-handle-test", "agent", "fs_search");
    (directory, workspace, request, context)
}

#[tokio::test]
async fn paginated_search_reopens_at_consumed_offset_without_losing_matches() {
    let (_directory, workspace, request, context) = fixture();
    let mut cursor = None;
    let mut version = None;
    let mut lines = Vec::new();
    loop {
        let (page, next) = workspace
            .search_v2(
                &context,
                &request,
                cursor.as_deref(),
                version.as_deref(),
                |_| {},
            )
            .await
            .unwrap();
        lines.extend(page.data.matches.iter().map(|entry| entry.line));
        version = Some(page.root_version);
        cursor = next;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(lines, vec![1, 2, 3]);
}

#[cfg(windows)]
#[tokio::test]
async fn paginated_search_releases_the_file_for_exclusive_save_between_pages() {
    use std::os::windows::fs::OpenOptionsExt;
    let (directory, workspace, request, context) = fixture();
    let (page, cursor) = workspace
        .search_v2(&context, &request, None, None, |_| {})
        .await
        .unwrap();
    assert!(cursor.is_some());
    // A real Windows share-mode=0 open fails if the cursor still owns its reader.
    let exclusive = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .open(directory.path().join("sample.txt"))
        .expect("search page must close its file handle");
    drop(exclusive);
    let (next, _) = workspace
        .search_v2(
            &context,
            &request,
            cursor.as_deref(),
            Some(&page.root_version),
            |_| {},
        )
        .await
        .unwrap();
    assert_eq!(next.data.matches[0].line, 2);
}

#[tokio::test]
async fn changed_file_cannot_continue_from_buffered_search_results() {
    let (directory, workspace, request, context) = fixture();
    let (page, cursor) = workspace
        .search_v2(&context, &request, None, None, |_| {})
        .await
        .unwrap();
    fs::write(
        directory.path().join("sample.txt"),
        "needle changed substantially\n",
    )
    .unwrap();
    let error = workspace
        .search_v2(
            &context,
            &request,
            cursor.as_deref(),
            Some(&page.root_version),
            |_| {},
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "cursor_stale");
}
