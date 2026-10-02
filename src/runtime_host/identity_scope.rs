use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};

use super::{RuntimeHost, now_ms};

const BINDING_LOOKUP: &str = "SELECT b.task_id,b.generation,t.generation FROM chatgpt_mcp_scope_bindings b JOIN tasks t ON t.id=b.task_id WHERE b.agent_id=? AND b.device_id=? AND b.scope_hash=? AND t.agent_id=b.agent_id AND t.device_id=b.device_id";

impl RuntimeHost {
    pub(super) async fn bound_task_for_provider_scope(
        &self,
        context: &OperationContext,
    ) -> RuntimeResult<Option<String>> {
        let Some(scope) = provider_scope(context) else {
            return Ok(None);
        };
        let row = sqlx::query_as::<_, (String, i64, i64)>(BINDING_LOOKUP)
            .bind(&context.agent_id)
            .bind(self.device.id.as_str())
            .bind(scope)
            .fetch_optional(self.repository.pool())
            .await
            .map_err(|_| identity_storage_error("conversation binding lookup failed"))?;
        let Some((task_id, binding_generation, task_generation)) = row else {
            return Ok(None);
        };

        if let Some(explicit) = context
            .task_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            && explicit != task_id
        {
            return Err(RuntimeError::new(
                "conversation_identity_conflict",
                "the provider conversation scope is already attached to another task; rebinding is not allowed",
            ));
        }
        if binding_generation != task_generation {
            return Err(RuntimeError::new(
                "conversation_compacting_or_archived",
                "this provider conversation scope belongs to an earlier conversation generation; use the confirmed replacement conversation",
            ));
        }
        if self.task_has_active_compaction(&task_id).await? {
            return Err(RuntimeError::new(
                "conversation_compacting_or_archived",
                "the resolved conversation is compacting; wait for its confirmed replacement binding",
            ));
        }
        Ok(Some(task_id))
    }

    pub(super) async fn validate_provider_scope_task(
        &self,
        context: &OperationContext,
        task_id: &str,
    ) -> RuntimeResult<()> {
        if provider_scope(context).is_none() {
            return Ok(());
        }
        let generation = sqlx::query_scalar::<_, i64>(
            "SELECT generation FROM tasks WHERE id=? AND agent_id=? AND device_id=? AND source IN ('chatgpt_web','mcp') AND allow_execute=1",
        )
        .bind(task_id)
        .bind(&context.agent_id)
        .bind(self.device.id.as_str())
        .fetch_optional(self.repository.pool())
        .await
        .map_err(|_| identity_storage_error("conversation task validation failed"))?;
        if generation.is_none() {
            let owner = sqlx::query_as::<_, (Option<String>, String)>(
                "SELECT agent_id,device_id FROM tasks WHERE id=?",
            )
            .bind(task_id)
            .fetch_optional(self.repository.pool())
            .await
            .map_err(|_| identity_storage_error("conversation task ownership lookup failed"))?;
            return Err(match owner {
                Some((agent, device))
                    if agent.as_deref() != Some(context.agent_id.as_str())
                        || device != self.device.id.as_str() =>
                {
                    RuntimeError::new(
                        "conversation_identity_conflict",
                        "the task does not belong to the authenticated execution context",
                    )
                }
                _ => provider_scope_unbound(),
            });
        }
        if self.task_has_active_compaction(task_id).await? {
            return Err(RuntimeError::new(
                "conversation_compacting_or_archived",
                "the resolved conversation is compacting; wait for its confirmed replacement binding",
            ));
        }
        Ok(())
    }

    pub(super) async fn bind_provider_scope_to_task(
        &self,
        context: &OperationContext,
        task_id: &str,
    ) -> RuntimeResult<()> {
        let Some(scope) = provider_scope(context) else {
            return Ok(());
        };
        self.validate_provider_scope_task(context, task_id).await?;
        let generation = sqlx::query_scalar::<_, i64>(
            "SELECT generation FROM tasks WHERE id=? AND agent_id=? AND device_id=? AND source IN ('chatgpt_web','mcp') AND allow_execute=1",
        )
        .bind(task_id)
        .bind(&context.agent_id)
        .bind(self.device.id.as_str())
        .fetch_one(self.repository.pool())
        .await
        .map_err(|_| identity_storage_error("conversation generation lookup failed"))?;
        let now = now_ms();
        let updated = sqlx::query(
            "INSERT INTO chatgpt_mcp_scope_bindings(agent_id,device_id,scope_hash,task_id,generation,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,?,?) ON CONFLICT(agent_id,device_id,scope_hash) DO UPDATE SET generation=excluded.generation,updated_at_ms=excluded.updated_at_ms WHERE chatgpt_mcp_scope_bindings.task_id=excluded.task_id",
        )
        .bind(&context.agent_id)
        .bind(self.device.id.as_str())
        .bind(scope)
        .bind(task_id)
        .bind(generation)
        .bind(now)
        .bind(now)
        .execute(self.repository.pool())
        .await
        .map_err(|_| identity_storage_error("conversation binding update failed"))?;
        if updated.rows_affected() == 0 {
            return Err(RuntimeError::new(
                "conversation_identity_conflict",
                "the provider conversation scope is already attached to another task; rebinding is not allowed",
            ));
        }
        Ok(())
    }

    async fn task_has_active_compaction(&self, task_id: &str) -> RuntimeResult<bool> {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM chatgpt_compact_jobs WHERE task_id=? AND phase NOT IN ('completed','cancelled'))",
        )
        .bind(task_id)
        .fetch_one(self.repository.pool())
        .await
        .map_err(|_| identity_storage_error("conversation compaction state lookup failed"))
    }
}

fn provider_scope(context: &OperationContext) -> Option<&str> {
    context
        .conversation_scope_id
        .as_deref()
        .map(str::trim)
        .filter(|value| value.starts_with("openai:"))
}

fn provider_scope_unbound() -> RuntimeError {
    RuntimeError::new(
        "conversation_identity_unbound",
        "this conversation scope is not bound to an executable task; omit a stale taskId or retry with the current authenticated conversation context",
    )
}

fn identity_storage_error(message: &'static str) -> RuntimeError {
    RuntimeError::new("storage_error", message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_host::user_message_tests::test_host;
    use chatcmd_runtime::OperationContext;

    async fn insert_chatgpt_task(
        host: &RuntimeHost,
        agent_id: &str,
        task_id: &str,
        generation: i64,
    ) {
        let now = now_ms();
        sqlx::query(
            "INSERT INTO tasks(id,agent_id,device_id,conversation_scope_hash,title,source,allow_execute,status,active_session_id,generation,stopped_at_ms,created_at_ms,updated_at_ms) VALUES(?,?,?,?,'Conversation','chatgpt_web',1,'running',NULL,?,NULL,?,?)",
        )
        .bind(task_id)
        .bind(agent_id)
        .bind(host.device.id.as_str())
        .bind(format!("browser-{task_id}"))
        .bind(generation)
        .bind(now)
        .bind(now)
        .execute(host.repository.pool())
        .await
        .expect("insert ChatGPT task");
    }

    async fn insert_mcp_task(
        host: &RuntimeHost,
        agent_id: &str,
        task_id: &str,
        allow_execute: bool,
    ) {
        let now = now_ms();
        sqlx::query(
            "INSERT INTO tasks(id,agent_id,device_id,title,source,allow_execute,status,active_session_id,generation,stopped_at_ms,created_at_ms,updated_at_ms) VALUES(?,?,?,'Conversation','mcp',?,'running',NULL,1,NULL,?,?)",
        )
        .bind(task_id)
        .bind(agent_id)
        .bind(host.device.id.as_str())
        .bind(allow_execute)
        .bind(now)
        .bind(now)
        .execute(host.repository.pool())
        .await
        .expect("insert MCP task");
    }

    fn context(agent_id: &str, task_id: Option<&str>, scope: &str) -> OperationContext {
        let mut context = OperationContext::new("request", agent_id, "workspace_roots");
        context.task_id = task_id.map(str::to_owned);
        context.conversation_scope_id = Some(scope.to_owned());
        context
    }

    #[tokio::test]
    async fn binding_reuses_task_without_overwriting_browser_identity() {
        let (host, agent_id, _directory) = test_host().await;
        insert_chatgpt_task(&host, &agent_id, "task-bound", 1).await;
        let first = context(&agent_id, Some("task-bound"), "openai:provider-scope");
        host.bind_provider_scope_to_task(&first, "task-bound")
            .await
            .expect("bind provider scope");

        let second = context(&agent_id, None, "openai:provider-scope");
        assert_eq!(
            host.bound_task_for_provider_scope(&second)
                .await
                .expect("resolve binding")
                .as_deref(),
            Some("task-bound")
        );
        let browser_scope: String =
            sqlx::query_scalar("SELECT conversation_scope_hash FROM tasks WHERE id='task-bound'")
                .fetch_one(host.repository.pool())
                .await
                .expect("read browser identity");
        assert_eq!(browser_scope, "browser-task-bound");
    }

    #[tokio::test]
    async fn binding_accepts_owned_executable_mcp_task() {
        let (host, agent_id, _directory) = test_host().await;
        insert_mcp_task(&host, &agent_id, "task-mcp", true).await;
        let context = context(&agent_id, Some("task-mcp"), "openai:provider-mcp");

        host.bind_provider_scope_to_task(&context, "task-mcp")
            .await
            .expect("bind provider scope to existing MCP task");
        assert_eq!(
            host.bound_task_for_provider_scope(&context)
                .await
                .expect("resolve MCP binding")
                .as_deref(),
            Some("task-mcp")
        );
    }

    #[tokio::test]
    async fn binding_rejects_non_executable_mcp_task() {
        let (host, agent_id, _directory) = test_host().await;
        insert_mcp_task(&host, &agent_id, "task-mcp-readonly", false).await;
        let context = context(
            &agent_id,
            Some("task-mcp-readonly"),
            "openai:provider-readonly",
        );

        let error = host
            .bind_provider_scope_to_task(&context, "task-mcp-readonly")
            .await
            .expect_err("non-executable task must not gain provider binding");
        assert_eq!(error.code, "conversation_identity_unbound");
    }

    #[tokio::test]
    async fn binding_refuses_cross_task_reassignment() {
        let (host, agent_id, _directory) = test_host().await;
        insert_chatgpt_task(&host, &agent_id, "task-a", 1).await;
        insert_chatgpt_task(&host, &agent_id, "task-b", 1).await;
        let first = context(&agent_id, Some("task-a"), "openai:provider-scope");
        host.bind_provider_scope_to_task(&first, "task-a")
            .await
            .expect("bind first task");

        let second = context(&agent_id, Some("task-b"), "openai:provider-scope");
        let error = host
            .bound_task_for_provider_scope(&second)
            .await
            .expect_err("must reject rebinding");
        assert_eq!(error.code, "conversation_identity_conflict");
    }

    #[tokio::test]
    async fn stale_generation_binding_is_rejected() {
        let (host, agent_id, _directory) = test_host().await;
        insert_chatgpt_task(&host, &agent_id, "task-bound", 1).await;
        let context = context(&agent_id, Some("task-bound"), "openai:provider-scope");
        host.bind_provider_scope_to_task(&context, "task-bound")
            .await
            .expect("bind task");
        sqlx::query("UPDATE tasks SET generation=2 WHERE id='task-bound'")
            .execute(host.repository.pool())
            .await
            .expect("advance generation");

        let error = host
            .bound_task_for_provider_scope(&context)
            .await
            .expect_err("stale binding must fail");
        assert_eq!(error.code, "conversation_compacting_or_archived");
    }
}
