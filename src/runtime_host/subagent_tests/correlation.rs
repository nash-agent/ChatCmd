use super::reports::{begin_child, finish};
use super::*;

fn implicit_context(agent: &str, tool: &str) -> OperationContext {
    let mut context = OperationContext::new(format!("implicit-{tool}"), agent, tool);
    context.conversation_scope_id = Some("openai:correlation-test".to_owned());
    context
}

#[tokio::test]
async fn browser_child_omitted_turn_preserves_the_delegated_report_turn() {
    let (host, parent, registration, id, _dir) = fallback_fixture().await;
    host.request_subagent_extension_fallback(&parent, &registration, &delegated_prompt(&id))
        .await
        .unwrap();
    let mut child = implicit_context(&parent.agent_id, "agent_user_message");
    child.conversation_scope_id = Some("openai:implicit-browser-child".to_owned());
    let started = Box::pin(host.call_persisted(
        "agent_user_message",
        child.clone(),
        json!({"content":delegated_prompt(&id)}),
    ))
    .await
    .unwrap();
    child.request_id = "implicit-browser-progress".to_owned();
    child.tool_name = "agent_progress".to_owned();
    let progress = Box::pin(host.call_persisted(
        "agent_progress",
        child.clone(),
        json!({"message":"Read-only lookup finished"}),
    ))
    .await
    .unwrap();
    assert_eq!(progress["turnId"], started["turnId"]);
    child.request_id = "implicit-browser-complete".to_owned();
    child.tool_name = "agent_turn_complete".to_owned();
    let completed = Box::pin(host.call_persisted(
        "agent_turn_complete",
        child,
        json!({"content":"size=142503, readonly=false", "workOutcome":"completed"}),
    ))
    .await
    .unwrap();
    assert_eq!(completed["turnId"], started["turnId"]);
    let report = host.wait_for_subagents(&parent, 250).await.unwrap();
    assert_eq!(
        report["subagents"][0]["report"]["content"],
        "size=142503, readonly=false"
    );
    assert_eq!(
        report["subagents"][0]["taskId"],
        registration["childTaskId"]
    );
}

#[tokio::test]
async fn child_turn_inference_rejects_ambiguity_and_preserves_ownership() {
    let (host, parent, registration, _id, _dir) = fallback_fixture().await;
    let child = begin_child(&host, &parent, &registration).await;
    let task = child.task_id.as_deref().unwrap();
    assert!(
        host.inferred_subagent_child_turn("another-agent", task)
            .await
            .unwrap()
            .is_none()
    );
    let mut explicit = child.clone();
    explicit.turn_id = Some("explicit-child-turn".to_owned());
    host.ensure_call_identity(&mut explicit, None)
        .await
        .unwrap();
    assert_eq!(explicit.turn_id.as_deref(), Some("explicit-child-turn"));
    sqlx::query("INSERT INTO timeline_events(event_id,task_id,turn_id,actor,kind,idempotency_key,payload_json,created_at_ms) VALUES('extra-child-user',?,'other-child-turn','user','message','extra-child-user','{}',?)")
        .bind(task).bind(now_ms()+10).execute(host.repository.pool()).await.unwrap();
    assert_eq!(
        host.inferred_subagent_child_turn(&parent.agent_id, task)
            .await
            .unwrap_err()
            .code,
        "invalid_context"
    );
}

#[tokio::test]
async fn omitted_wait_ids_recover_report_in_the_authenticated_parent_conversation() {
    let (host, mut parent, _dir) = parent_fixture().await;
    parent.conversation_scope_id = Some("openai:correlation-test".to_owned());
    let registration = Box::pin(host.call_persisted(
        "agent_subagent_start",
        parent.clone(),
        json!({"name":"Reader", "request":"Read the delegated source"}),
    ))
    .await
    .expect("register parent child");
    assert_eq!(registration["taskId"], PARENT_TASK_ID);
    assert_eq!(registration["parentTaskId"], PARENT_TASK_ID);
    assert_eq!(registration["turnId"], PARENT_TURN_ID);
    assert_eq!(registration["parentTurnId"], PARENT_TURN_ID);
    assert_ne!(registration["childTaskId"], registration["taskId"]);

    let child = begin_child(&host, &parent, &registration).await;
    finish(&host, &child, "Durable child result", "completed").await;
    let args = json!({"subagentId":registration["subagentId"], "timeoutMs":250});
    let result = Box::pin(host.call_persisted(
        "agent_subagent_wait",
        implicit_context(&parent.agent_id, "agent_subagent_wait"),
        args.clone(),
    ))
    .await
    .expect("recover omitted parent correlation");
    assert_eq!(result["taskId"], PARENT_TASK_ID);
    assert_eq!(result["turnId"], PARENT_TURN_ID);
    assert_eq!(
        result["subagents"][0]["report"]["content"],
        "Durable child result"
    );

    let mut wrong_turn = implicit_context(&parent.agent_id, "agent_subagent_wait");
    wrong_turn.request_id = "wrong-turn-request".to_owned();
    wrong_turn.turn_id = Some("explicit-unrelated-turn".to_owned());
    let denied = Box::pin(host.call_persisted("agent_subagent_wait", wrong_turn, args.clone()))
        .await
        .expect_err("explicit unrelated turn must remain isolated");
    assert_eq!(denied.code, "subagent_not_found");

    let mut other_chat = implicit_context(&parent.agent_id, "agent_subagent_wait");
    other_chat.request_id = "other-chat-request".to_owned();
    other_chat.conversation_scope_id = Some("openai:unrelated-chat".to_owned());
    let denied = Box::pin(host.call_persisted("agent_subagent_wait", other_chat, args))
        .await
        .expect_err("another authenticated conversation must not recover this report");
    assert_eq!(denied.code, "subagent_not_found");
}

#[tokio::test]
async fn omitted_completion_turn_cannot_bypass_pending_child_gate() {
    let (host, mut parent, _dir) = parent_fixture().await;
    parent.conversation_scope_id = Some("openai:correlation-test".to_owned());
    Box::pin(host.call_persisted(
        "agent_subagent_start",
        parent.clone(),
        json!({"name":"Pending reader", "request":"Read the delegated source"}),
    ))
    .await
    .expect("register pending child");
    let denied = Box::pin(host.call_persisted(
        "agent_turn_complete",
        implicit_context(&parent.agent_id, "agent_turn_complete"),
        json!({"content":"Premature final answer"}),
    ))
    .await
    .expect_err("pending child must prevent finalization when turnId is omitted");
    assert_eq!(denied.code, "subagents_still_running");
}

#[tokio::test]
async fn implicit_wait_requires_selection_when_parent_turns_are_ambiguous() {
    let (host, parent, first, id, _dir) = fallback_fixture().await;
    let mut second_parent = parent.clone();
    second_parent.turn_id = Some("another-parent-turn".to_owned());
    host.register_subagent(&second_parent, "Second reader", "Another request", None)
        .await
        .expect("register other turn");
    let task = parent.task_id.as_deref().unwrap();
    let denied = host
        .inferred_subagent_parent_turn(&parent.agent_id, task, None, false)
        .await
        .expect_err("do not guess among multiple parent turns");
    assert_eq!(denied.code, "invalid_context");
    assert_eq!(
        host.inferred_subagent_parent_turn(&parent.agent_id, task, Some(&id), false)
            .await
            .unwrap()
            .as_deref(),
        Some(PARENT_TURN_ID)
    );
    assert_eq!(first["parentTurnId"], PARENT_TURN_ID);
    assert_eq!(
        host.inferred_subagent_parent_turn("another-agent", task, Some(&id), false)
            .await
            .expect_err("another agent cannot infer the parent's turn")
            .code,
        "subagent_not_found"
    );
}
