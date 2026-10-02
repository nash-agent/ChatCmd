use super::*;

#[tokio::test]
async fn browser_child_scope_remains_bound_and_cannot_spawn_without_task_id() {
    let (host, parent, registration, id, _dir) = fallback_fixture().await;
    host.request_subagent_extension_fallback(&parent, &registration, &delegated_prompt(&id))
        .await
        .unwrap();
    let mut child = OperationContext::new("browser-claim", &parent.agent_id, "agent_user_message");
    child.conversation_scope_id = Some("openai:browser-child".to_owned());
    let claimed = host
        .call_persisted(
            "agent_user_message",
            child.clone(),
            json!({"content": delegated_prompt(&id)}),
        )
        .await
        .unwrap();
    assert_eq!(claimed["taskId"], registration["childTaskId"]);
    assert_eq!(claimed["subagentPolicy"]["enabled"], false);

    child.request_id = "browser-next-tool".to_owned();
    child.tool_name = "agent_subagent_start".to_owned();
    let error = host
        .call_persisted(
            "agent_subagent_start",
            child,
            json!({"name":"Recursive child","request":"Repeat this job"}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "subagent_delegation_forbidden");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM subagent_runs")
        .fetch_one(host.repository.pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
    let roots: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE id LIKE 'task-chat-%'")
        .fetch_one(host.repository.pool())
        .await
        .unwrap();
    assert_eq!(roots, 0);
}

#[tokio::test]
async fn copied_child_envelope_is_rejected_before_registration() {
    let (host, parent, _dir) = parent_fixture().await;
    let error = host
        .register_subagent(
            &parent,
            "Copied child",
            "Do work\nCMDGPT_SUBAGENT_ID=subagent-old",
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid_subagent_request");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM subagent_runs")
        .fetch_one(host.repository.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn conflicting_child_markers_cannot_claim_either_task() {
    let (host, mut context, registration, id, _dir) = fallback_fixture().await;
    let prompt = format!(
        "{}\nCMDGPT_SUBAGENT_ID=subagent-other",
        delegated_prompt(&id)
    );
    let error = host
        .ensure_call_identity(&mut context, Some(&prompt))
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid_subagent_marker");
    let status: String =
        sqlx::query_scalar("SELECT status FROM subagent_runs WHERE child_task_id=?")
            .bind(registration["childTaskId"].as_str().unwrap())
            .fetch_one(host.repository.pool())
            .await
            .unwrap();
    assert_eq!(status, "pending");
}
