use std::collections::BTreeSet;

use chatcmd_runtime::RuntimeError;
use uuid::Uuid;

pub(super) fn unique_bridge_task_for_message(
    rows: &[(String, String)],
    preferred_task_id: Option<&str>,
    message: &str,
) -> Option<String> {
    let exact = rows
        .iter()
        .filter(|(_, submitted)| submitted == message)
        .map(|(task_id, _)| task_id.as_str())
        .collect::<BTreeSet<_>>();
    if !exact.is_empty() {
        return preferred_or_unique_task(exact, preferred_task_id);
    }
    let equivalent = rows
        .iter()
        .filter(|(_, submitted)| crate::chatgpt_message::equivalent(submitted, message))
        .map(|(task_id, _)| task_id.as_str())
        .collect::<BTreeSet<_>>();
    preferred_or_unique_task(equivalent, preferred_task_id)
}

fn preferred_or_unique_task(
    candidates: BTreeSet<&str>,
    preferred_task_id: Option<&str>,
) -> Option<String> {
    if let Some(preferred) = preferred_task_id
        && candidates.contains(preferred)
    {
        return Some(preferred.to_owned());
    }
    (candidates.len() == 1)
        .then(|| candidates.into_iter().next())
        .flatten()
        .map(str::to_owned)
}

pub(super) fn conversation_approval_denied() -> RuntimeError {
    RuntimeError::new(
        "conversation_approval_denied",
        "Anti-hack verification mode is enabled. This conversation was not approved or the 60-second approval window expired, so it cannot execute.",
    )
}

pub(super) fn select_task_identity(
    agent_id: &str,
    conversation_scope: Option<&str>,
    explicit_task_id: Option<&str>,
    bound_task_id: Option<&str>,
    request_id: &str,
) -> String {
    if let Some(scope) = conversation_scope.filter(|value| !value.trim().is_empty()) {
        return safe_id("task-chat", agent_id, scope);
    }
    if let Some(task_id) = explicit_task_id.filter(|value| !value.trim().is_empty()) {
        return task_id.trim().to_owned();
    }
    if let Some(task_id) = bound_task_id.filter(|value| !value.trim().is_empty()) {
        return task_id.trim().to_owned();
    }
    safe_id("task", agent_id, request_id)
}

pub(super) fn task_identity_from_first_message(
    agent_id: &str,
    conversation_scope: &str,
    message: &str,
) -> String {
    safe_id(
        "task-chat",
        agent_id,
        &format!("{conversation_scope}\0first-user-message:{message}"),
    )
}

pub(super) fn safe_id(prefix: &str, agent_id: &str, scope: &str) -> String {
    let material = format!("{prefix}\0agent:{agent_id}\0scope:{scope}");
    format!(
        "{prefix}-{}",
        Uuid::new_v5(&Uuid::NAMESPACE_OID, material.as_bytes())
    )
}

#[cfg(test)]
mod tests {
    use super::{
        select_task_identity, task_identity_from_first_message, unique_bridge_task_for_message,
    };

    #[test]
    fn conversation_scope_overrides_stale_explicit_task() {
        let first = select_task_identity(
            "agent",
            Some("conversation-a"),
            Some("old-task"),
            None,
            "r1",
        );
        let second = select_task_identity(
            "agent",
            Some("conversation-b"),
            Some("old-task"),
            None,
            "r2",
        );
        assert_ne!(first, "old-task");
        assert_ne!(second, "old-task");
        assert_ne!(first, second);
    }

    #[test]
    fn first_user_message_participates_in_new_chat_identity() {
        let first = task_identity_from_first_message("agent", "conversation", "hello");
        assert_eq!(
            first,
            task_identity_from_first_message("agent", "conversation", "hello")
        );
        assert_ne!(
            first,
            task_identity_from_first_message("agent", "conversation", "another message")
        );
    }

    #[test]
    fn explicit_task_and_turn_binding_are_safe_fallbacks_without_private_scope() {
        assert_eq!(
            select_task_identity("agent", None, Some("task-known"), Some("task-bound"), "r1"),
            "task-known"
        );
        assert_eq!(
            select_task_identity("agent", None, None, Some("task-bound"), "r1"),
            "task-bound"
        );
        assert_ne!(
            select_task_identity("agent", None, None, None, "r1"),
            select_task_identity("agent", None, None, None, "r2")
        );
    }

    #[test]
    fn unicode_space_bridge_match_requires_one_unambiguous_task() {
        let rows = vec![("task-a".to_owned(), "Example abcd ".to_owned())];
        assert_eq!(
            unique_bridge_task_for_message(&rows, None, "Example abcd\u{00a0}"),
            Some("task-a".to_owned())
        );

        let ambiguous = vec![
            ("task-a".to_owned(), "Example abcd ".to_owned()),
            ("task-b".to_owned(), "Example abcd\u{202f}".to_owned()),
        ];
        assert_eq!(
            unique_bridge_task_for_message(&ambiguous, None, "Example abcd\u{00a0}"),
            None
        );
        assert_eq!(
            unique_bridge_task_for_message(&ambiguous, Some("task-b"), "Example abcd\u{00a0}"),
            Some("task-b".to_owned())
        );
        assert_eq!(
            unique_bridge_task_for_message(
                &ambiguous,
                Some("task-unrelated"),
                "Example abcd\u{00a0}"
            ),
            None
        );
    }
}
