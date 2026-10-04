use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    response::Response,
};
use serde_json::{Value, json};
use tower::ServiceExt;

use super::chatgpt_router_tests::{expect_json, fixture};

const PATH: &str = "/api/local/workspaces/projects";

fn display_path(path: &std::path::Path) -> String {
    let value = path.canonicalize().unwrap().to_string_lossy().into_owned();
    #[cfg(windows)]
    {
        return value.strip_prefix(r"\\?\").unwrap_or(&value).to_owned();
    }
    #[cfg(not(windows))]
    value
}

async fn request(
    app: &Router,
    method: &str,
    path: &str,
    body: Value,
    cookie: Option<&str>,
) -> Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("X-ChatCmdClient", "local-ui")
        .header("Content-Type", "application/json");
    if let Some(cookie) = cookie {
        request = request.header("Cookie", cookie);
    }
    app.clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn shared_project_access_is_default_off_authenticated_and_round_trips() {
    let (state, app, _directory) = fixture("running").await;
    let folder = tempfile::tempdir().unwrap();
    let body = json!({"name":"Shared", "path":folder.path()});
    let token = state
        .gui_auth
        .setup_password("project-access-test-password".to_owned())
        .await
        .unwrap();
    let cookie = format!("chatcmd_gui_session={token}");
    let unauthorized = request(&app, "POST", PATH, body.clone(), None).await;
    assert!(!unauthorized.status().is_success());
    let saved = expect_json(
        request(&app, "POST", PATH, body.clone(), Some(&cookie)).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(saved["allowAllConversations"], false);
    let id = saved["id"].as_str().unwrap();
    let path = format!("{PATH}/{id}");
    let shared = json!({"allowAllConversations":true,"name":"Shared", "path":folder.path()});
    let saved = expect_json(
        request(&app, "PUT", &path, shared, Some(&cookie)).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(saved["allowAllConversations"], true);
    let approved: Option<String> =
        sqlx::query_scalar("SELECT global_access_path FROM workspace_projects WHERE id=?")
            .bind(id)
            .fetch_one(state.repository.pool())
            .await
            .unwrap();
    assert_eq!(
        approved.as_deref(),
        Some(folder.path().canonicalize().unwrap().to_str().unwrap())
    );
    let listed = expect_json(
        request(&app, "GET", PATH, json!(null), Some(&cookie)).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(listed[0]["allowAllConversations"], true);
    assert_eq!(listed[0]["globalAccessPath"], display_path(folder.path()));
    let saved = expect_json(
        request(&app, "PUT", &path, body, Some(&cookie)).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(saved["allowAllConversations"], false);
    let approved: Option<String> =
        sqlx::query_scalar("SELECT global_access_path FROM workspace_projects WHERE id=?")
            .bind(id)
            .fetch_one(state.repository.pool())
            .await
            .unwrap();
    assert_eq!(approved, None);
}

#[tokio::test]
async fn shared_project_can_use_a_different_global_root() {
    let (state, app, _directory) = fixture("running").await;
    let project = tempfile::tempdir().unwrap();
    let shared = tempfile::tempdir().unwrap();
    let token = state
        .gui_auth
        .setup_password("project-global-root-test-password".to_owned())
        .await
        .unwrap();
    let cookie = format!("chatcmd_gui_session={token}");
    let saved = expect_json(
        request(
            &app,
            "POST",
            PATH,
            json!({
                "name":"Shared",
                "path":project.path(),
                "allowAllConversations":true,
                "globalAccessPath":shared.path()
            }),
            Some(&cookie),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(saved["globalAccessPath"], display_path(shared.path()));
    let approved: Option<String> =
        sqlx::query_scalar("SELECT global_access_path FROM workspace_projects WHERE id=?")
            .bind(saved["id"].as_str().unwrap())
            .fetch_one(state.repository.pool())
            .await
            .unwrap();
    assert_eq!(
        approved.as_deref(),
        Some(shared.path().canonicalize().unwrap().to_str().unwrap())
    );
}

#[tokio::test]
async fn shared_project_changes_and_delete_revoke_cached_approval_grants() {
    let (state, app, _directory) = fixture("running").await;
    let folder = tempfile::tempdir().unwrap();
    let token = state
        .gui_auth
        .setup_password("project-grant-test-password".to_owned())
        .await
        .unwrap();
    let cookie = format!("chatcmd_gui_session={token}");
    let body = json!({"name":"Shared", "path":folder.path(),"allowAllConversations":true});
    let saved = expect_json(
        request(&app, "POST", PATH, body.clone(), Some(&cookie)).await,
        StatusCode::OK,
    )
    .await;
    let path = format!("{PATH}/{}", saved["id"].as_str().unwrap());
    for action in ["PUT", "DELETE"] {
        sqlx::query("INSERT OR REPLACE INTO approval_grants(id,owner_agent_id,task_id,allowed_tools_json,path_scopes_json,option_constraints_json,max_calls,expires_at_ms,catalog_hash,state,created_at_ms,updated_at_ms) SELECT 'cached',agent_id,id,'[]','[]','{}',2,9999999999999,'catalog','active',0,0 FROM tasks WHERE id='task-a'")
            .execute(state.repository.pool()).await.unwrap();
        expect_json(
            request(
                &app,
                action,
                &path,
                json!({"name":"Shared", "path":folder.path(),"allowAllConversations":false}),
                Some(&cookie),
            )
            .await,
            StatusCode::OK,
        )
        .await;
        let status: String =
            sqlx::query_scalar("SELECT state FROM approval_grants WHERE id='cached'")
                .fetch_one(state.repository.pool())
                .await
                .unwrap();
        assert_eq!(status, "revoked");
    }
}

#[tokio::test]
async fn shared_project_rejects_missing_relative_and_file_global_roots() {
    let (state, app, _directory) = fixture("running").await;
    let token = state
        .gui_auth
        .setup_password("project-path-test-password".to_owned())
        .await
        .unwrap();
    let cookie = format!("chatcmd_gui_session={token}");
    let project = tempfile::tempdir().unwrap();
    let file = project.path().join("not-a-root.txt");
    std::fs::write(&file, "data").unwrap();
    for global_access_path in [
        "relative".to_owned(),
        file.to_string_lossy().into_owned(),
    ] {
        expect_json(
            request(
                &app,
                "POST",
                PATH,
                json!({
                    "name":"Unsafe",
                    "path":project.path(),
                    "allowAllConversations":true,
                    "globalAccessPath":global_access_path
                }),
                Some(&cookie),
            )
            .await,
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workspace_projects")
        .fetch_one(state.repository.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
}
