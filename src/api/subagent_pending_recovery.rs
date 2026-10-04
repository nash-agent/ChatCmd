use super::*;

pub(in crate::api) async fn pending_subagent_fallbacks(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<Value>>, Problem> {
    if !heartbeat::creation_enabled(&state).await? {
        return Ok(Json(Vec::new()));
    }
    let rows = sqlx::query(
        "SELECT r.id,r.parent_task_id,r.parent_turn_id,r.child_task_id,r.name,r.request,r.fallback_attempts,r.fallback_state,r.created_at_ms,r.max_runtime_ms,r.fallback_conversation_id,r.fallback_conversation_url,a.name AS agent_name,p.project_folder AS parent_project_folder FROM subagent_runs r LEFT JOIN tasks t ON t.id=r.child_task_id LEFT JOIN mcp_agents a ON a.id=t.agent_id LEFT JOIN tasks p ON p.id=r.parent_task_id WHERE r.status='pending' AND p.status IN ('pending','running') AND r.fallback_state IN ('requested','started') AND r.fallback_attempts BETWEEN 1 AND ? ORDER BY r.updated_at_ms,r.id",
    )
    .bind(MAX_EXTENSION_FALLBACK_ATTEMPTS)
    .fetch_all(state.repository.pool())
    .await
    .map_err(db_problem)?;
    Ok(Json(
        rows.iter()
            .map(|row| {
                let mut value = fallback_request_value(row, row.get::<i64, _>("fallback_attempts"));
                // Only a current reservation may recover a missed creation event.
                // A known conversation is resumed separately; stale requests cannot spawn chats.
                let deadline = row
                    .get::<i64, _>("created_at_ms")
                    .saturating_add(row.get::<i64, _>("max_runtime_ms").min(180_000));
                value["canStartNewConversation"] = json!(
                    row.get::<String, _>("fallback_state") == "requested" && now_ms() < deadline
                );
                value
            })
            .collect(),
    ))
}
