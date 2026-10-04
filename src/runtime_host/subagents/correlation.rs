use std::collections::BTreeSet;

use super::*;

impl RuntimeHost {
    /// An authenticated active child has one delegated user turn. Keep optional
    /// tool correlation on that turn so evidence and the final report agree.
    /// Explicit turn IDs and new user messages never use this inference.
    pub(in crate::runtime_host) async fn inferred_subagent_child_turn(
        &self,
        agent_id: &str,
        child_task_id: &str,
    ) -> RuntimeResult<Option<String>> {
        let turns = sqlx::query_scalar::<_, String>(
            "SELECT u.turn_id FROM subagent_runs r JOIN tasks t ON t.id=r.child_task_id
             JOIN timeline_events u ON u.task_id=t.id
             WHERE r.child_task_id=? AND t.agent_id=? AND r.status='running'
               AND u.actor='user' AND u.kind='message' AND u.turn_id IS NOT NULL
               AND u.created_at_ms>=COALESCE(r.started_at_ms,r.created_at_ms)
               AND COALESCE(json_extract(u.payload_json,'$.provider'),'')<>'chatgpt_web'
             GROUP BY u.turn_id ORDER BY MIN(u.created_at_ms),u.turn_id LIMIT 2",
        )
        .bind(child_task_id)
        .bind(agent_id)
        .fetch_all(self.repository.pool())
        .await
        .map_err(|_| RuntimeError::new("storage_error", "sub-agent child turn lookup failed"))?;
        if turns.len() > 1 {
            return Err(RuntimeError::new(
                "invalid_context",
                "multiple child user turns exist; reuse the delegated user message's turnId",
            ));
        }
        Ok(turns.into_iter().next())
    }

    /// Recover omitted lifecycle correlation only within the resolved parent task.
    /// Explicit caller turns never use this fallback, including unrelated turns.
    pub(in crate::runtime_host) async fn inferred_subagent_parent_turn(
        &self,
        agent_id: &str,
        parent_task_id: &str,
        selected_subagent_id: Option<&str>,
        completing: bool,
    ) -> RuntimeResult<Option<String>> {
        let owned: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tasks WHERE id=? AND agent_id=?)")
                .bind(parent_task_id)
                .bind(agent_id)
                .fetch_one(self.repository.pool())
                .await
                .map_err(|_| {
                    RuntimeError::new("storage_error", "sub-agent parent ownership lookup failed")
                })?;
        if !owned {
            return if selected_subagent_id.is_some() {
                Err(RuntimeError::new(
                    "subagent_not_found",
                    "report does not belong to this agent",
                ))
            } else {
                Ok(None)
            };
        }
        let runs = self.subagent_values(parent_task_id, None).await?;
        let turns = runs
            .iter()
            .filter(|run| {
                selected_subagent_id.is_none_or(|id| run["id"].as_str() == Some(id))
                    && (!completing || is_pending_status(run["status"].as_str()))
            })
            .filter_map(|run| run["rootTurnId"].as_str())
            .collect::<BTreeSet<_>>();
        if turns.is_empty() && selected_subagent_id.is_some() {
            return Err(RuntimeError::new(
                "subagent_not_found",
                "report is not a descendant of the current parent task",
            ));
        }
        if turns.len() > 1 {
            return Err(RuntimeError::new(
                "invalid_context",
                "multiple parent turns have delegated children; reuse the registration's parentTurnId as turnId or select a subagentId when waiting",
            ));
        }
        Ok(turns.into_iter().next().map(str::to_owned))
    }
}
