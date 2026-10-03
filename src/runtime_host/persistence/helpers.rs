use super::*;

pub(super) fn phase_for_tool(tool: &str) -> ToolPhase {
    if matches!(tool, "fs_search" | "fs_find" | "fs_list" | "fs_list_v2") {
        ToolPhase::Scanning
    } else if matches!(tool, "fs_read_text" | "fs_read_text_v2" | "fs_stat") {
        ToolPhase::Reading
    } else if tool.starts_with("fs_") {
        ToolPhase::Staging
    } else if tool.starts_with("git_") {
        ToolPhase::Committing
    } else if tool.starts_with("shell_") || tool.starts_with("process_") {
        ToolPhase::ProcessRunning
    } else if tool.starts_with("blob_") || tool.starts_with("task_artifact_") {
        ToolPhase::ArtifactWriting
    } else if tool == "agent_plan_question" {
        ToolPhase::WaitingApproval
    } else if tool.starts_with("agent_subagent_") {
        ToolPhase::WaitingSubagent
    } else {
        ToolPhase::Syncing
    }
}

pub(super) fn tool_usage_from_value(value: &Value) -> ToolUsage {
    let mut usage: ToolUsage = value
        .get("usage")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    if usage.output_bytes == 0 {
        usage.output_bytes = serde_json::to_vec(value)
            .map(|bytes| u64::try_from(bytes.len()).unwrap_or(u64::MAX))
            .unwrap_or(0);
    }
    usage
}

pub(super) fn tool_result_is_truncated(value: &Value) -> bool {
    value
        .get("truncation")
        .and_then(|value| value.get("truncated"))
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            value
                .get("truncated")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
}

pub(super) fn tool_result_has_artifact(value: &Value) -> bool {
    ["contentRef", "content_ref", "artifactRef", "artifact_ref"]
        .into_iter()
        .any(|key| value.get(key).is_some_and(|value| !value.is_null()))
}

pub(super) fn should_externalize_tool_output(
    tool: &str,
    projection: &crate::runtime_host::tool_event_projection::ToolEventProjection,
) -> bool {
    projection.received_bytes >= 128 * 1024
        && matches!(
            tool,
            "fs_read_text" | "fs_read_text_v2" | "git_diff" | "git_show"
        )
}

pub(super) fn is_cancel_code(code: &str) -> bool {
    matches!(
        code,
        "operationCancelled" | "cancelled" | "activity_stopped"
    )
}

pub(super) fn is_timeout_code(code: &str) -> bool {
    matches!(code, "timeBudgetExceeded" | "timeout" | "timed_out")
}

pub(super) fn add_projection_metadata(
    payload: &mut Value,
    direction: &str,
    projection: &crate::runtime_host::tool_event_projection::ToolEventProjection,
) {
    payload[format!("{direction}BytesReceived")] = json!(projection.received_bytes);
    payload[format!("{direction}BytesProjected")] = json!(projection.projected_bytes);
    if projection.truncated {
        payload["payloadTruncated"] = Value::Bool(true);
    }
    if !projection.redactions.is_empty() {
        payload["redactions"] = json!(projection.redactions);
    }
}

pub(super) fn enrich_tool_result(value: Value, context: &OperationContext, tool: &str) -> Value {
    let mut object = match value {
        Value::Object(object) => object,
        other => {
            let mut object = serde_json::Map::new();
            object.insert("result".to_owned(), other);
            object
        }
    };
    if let Some(task_id) = context.task_id.as_deref() {
        object.insert("taskId".to_owned(), Value::String(task_id.to_owned()));
    }
    if let Some(turn_id) = context.turn_id.as_deref() {
        object.insert("turnId".to_owned(), Value::String(turn_id.to_owned()));
    }
    if let Some(session_id) = context.mcp_session_id.as_deref() {
        object
            .entry("sessionId".to_owned())
            .or_insert_with(|| Value::String(session_id.to_owned()));
    }
    let completed = tool == "agent_turn_complete";
    object.insert("requiresFinalization".to_owned(), Value::Bool(!completed));
    if completed {
        object.insert("completed".to_owned(), Value::Bool(true));
        object.insert(
            "continuationInstruction".to_owned(),
            Value::String(
                "Reply to the user with the exact content passed to agent_turn_complete. Reuse this taskId on later turns in the same chat."
                    .to_owned(),
            ),
        );
    } else {
        object.insert(
            "finalizer".to_owned(),
            Value::String("agent_turn_complete".to_owned()),
        );
    }
    Value::Object(object)
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::enrich_tool_result;
    use chatcmd_runtime::OperationContext;
    use serde_json::json;

    #[test]
    fn enrichment_preserves_tool_session_but_keeps_parent_task_correlation() {
        let mut context = OperationContext::new("request", "agent", "test_tool");
        context.task_id = Some("parent-task".to_owned());
        context.turn_id = Some("parent-turn".to_owned());
        context.mcp_session_id = Some("logical-mcp-session".to_owned());
        let result = enrich_tool_result(
            json!({
                "taskId": "child-task",
                "turnId": "child-turn",
                "sessionId": "physical-shell-session"
            }),
            &context,
            "test_tool",
        );
        assert_eq!(result["taskId"], "parent-task");
        assert_eq!(result["turnId"], "parent-turn");
        assert_eq!(result["sessionId"], "physical-shell-session");
    }

    #[test]
    fn enrichment_adds_missing_context_correlation_ids() {
        let mut context = OperationContext::new("request", "agent", "test_tool");
        context.task_id = Some("task".to_owned());
        context.turn_id = Some("turn".to_owned());
        context.mcp_session_id = Some("session".to_owned());
        let result = enrich_tool_result(json!({"accepted":true}), &context, "test_tool");
        assert_eq!(result["taskId"], "task");
        assert_eq!(result["turnId"], "turn");
        assert_eq!(result["sessionId"], "session");
    }
}

pub(super) fn required_task_id(context: &OperationContext) -> RuntimeResult<TaskId> {
    TaskId::new(context.task_id.as_deref().unwrap_or_default())
        .map_err(|error| invalid("taskId", error))
}

pub(super) fn required_turn_id(context: &OperationContext) -> RuntimeResult<TurnId> {
    TurnId::new(context.turn_id.as_deref().unwrap_or_default())
        .map_err(|error| invalid("turnId", error))
}

pub(super) fn required_session_id(context: &OperationContext) -> RuntimeResult<SessionId> {
    SessionId::new(context.mcp_session_id.as_deref().unwrap_or_default())
        .map_err(|error| invalid("sessionId", error))
}

pub(super) fn safe_id(prefix: &str, agent_id: &str, scope: &str) -> String {
    let material = format!("{prefix}\0agent:{agent_id}\0scope:{scope}");
    format!(
        "{prefix}-{}",
        Uuid::new_v5(&Uuid::NAMESPACE_OID, material.as_bytes())
    )
}
