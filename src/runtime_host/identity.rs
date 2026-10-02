use super::{
    RuntimeHost,
    identity_support::{
        conversation_approval_denied, safe_id, select_task_identity,
        task_identity_from_first_message, unique_bridge_task_for_message,
    },
    invalid, now_ms, storage_error,
};
use chatcmd_core::{
    AgentId, SessionId, SettingsStore as _, Task, TaskId, TaskSession, TaskStatus, TaskStore as _,
    TerminalSessionStatus, TurnBinding, TurnId,
};
use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};

impl RuntimeHost {
    pub(super) async fn ensure_call_identity(
        &self,
        context: &mut OperationContext,
        first_user_message: Option<&str>,
    ) -> RuntimeResult<()> {
        let conversation_scope = context.conversation_scope_id.clone();
        let provider_scope = conversation_scope
            .as_deref()
            .map(str::trim)
            .filter(|value| value.starts_with("openai:"))
            .map(str::to_owned);
        let legacy_scope = if provider_scope.is_none() {
            conversation_scope
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        } else {
            None
        };
        let delegated_task = self
            .delegated_subagent_task_id(context, first_user_message)
            .await?;
        let durable_scope_task = if delegated_task.is_none() && provider_scope.is_some() {
            self.bound_task_for_provider_scope(context).await?
        } else {
            None
        };
        let bound_task = if delegated_task.is_none()
            && durable_scope_task.is_none()
            && conversation_scope.is_none()
            && context.task_id.is_none()
        {
            self.bound_task_for_turn(context).await?
        } else {
            None
        };
        let chatgpt_bridge_task = if delegated_task.is_none() && durable_scope_task.is_none() {
            if let Some(message) = first_user_message {
                self.chatgpt_bridge_task_for_message(
                    &context.agent_id,
                    context.task_id.as_deref(),
                    message,
                )
                .await?
            } else {
                None
            }
        } else {
            None
        };
        let mapped_scope_task = if delegated_task.is_none()
            && durable_scope_task.is_none()
            && chatgpt_bridge_task.is_none()
        {
            if let Some(scope) = legacy_scope.as_deref() {
                self.bound_task_for_conversation_scope(&context.agent_id, scope)
                    .await?
            } else {
                None
            }
        } else {
            None
        };
        let pending_chatgpt_bridge_task = if delegated_task.is_none()
            && durable_scope_task.is_none()
            && chatgpt_bridge_task.is_none()
            && mapped_scope_task.is_none()
        {
            let bound = if first_user_message.is_none() {
                self.unique_recent_chatgpt_bridge_task(&context.agent_id)
                    .await?
            } else {
                None
            };
            match bound {
                Some(task_id) => Some(task_id),
                None => {
                    self.claim_unbound_chatgpt_bridge_task(
                        &context.agent_id,
                        legacy_scope.as_deref(),
                        first_user_message,
                    )
                    .await?
                }
            }
        } else {
            None
        };
        let routed_task = durable_scope_task
            .clone()
            .or_else(|| chatgpt_bridge_task.clone())
            .or_else(|| mapped_scope_task.clone())
            .or_else(|| pending_chatgpt_bridge_task.clone())
            .or_else(|| context.task_id.clone())
            .or_else(|| bound_task.clone());
        let provider_bootstrap =
            delegated_task.is_none() && provider_scope.is_some() && routed_task.is_none();
        let task = delegated_task.clone().unwrap_or_else(|| {
            if provider_bootstrap {
                safe_id(
                    "task-chat",
                    &context.agent_id,
                    provider_scope.as_deref().unwrap_or_default(),
                )
            } else {
                routed_task.unwrap_or_else(|| {
                    if let (Some(scope), Some(message)) =
                        (legacy_scope.as_deref(), first_user_message)
                    {
                        task_identity_from_first_message(&context.agent_id, scope, message)
                    } else {
                        select_task_identity(
                            &context.agent_id,
                            legacy_scope.as_deref(),
                            context.task_id.as_deref(),
                            bound_task.as_deref(),
                            &context.request_id,
                        )
                    }
                })
            }
        });
        if delegated_task.is_none()
            && provider_scope.is_some()
            && durable_scope_task.is_none()
            && !provider_bootstrap
        {
            self.validate_provider_scope_task(context, &task).await?;
        }
        let turn = context.turn_id.clone().unwrap_or_else(|| {
            safe_id(
                "turn",
                &context.agent_id,
                &format!("{task}\0{}", context.request_id),
            )
        });
        context.task_id = Some(task.clone());
        context.turn_id = Some(turn);

        let task_id = TaskId::new(&task).map_err(|error| invalid("taskId", error))?;
        let current = self
            .repository
            .task(&task_id)
            .await
            .map_err(storage_error)?;
        let reopening_stopped = current
            .as_ref()
            .is_some_and(|task| task.status == TaskStatus::Stopped);
        if reopening_stopped && first_user_message.is_none() {
            return Err(RuntimeError::new(
                "conversation_stopped",
                "this conversation was stopped by the user; only a new user message can reopen it",
            ));
        }
        let generation = current.as_ref().map_or(1, |task| {
            if reopening_stopped {
                task.generation.saturating_add(1)
            } else {
                task.generation
            }
        });
        let logical_session_scope = if generation > 1 {
            format!("{task}\0generation:{generation}")
        } else {
            task
        };
        let logical_session = safe_id("mcp-session", &context.agent_id, &logical_session_scope);
        context.mcp_session_id = Some(logical_session.clone());
        let session_id =
            SessionId::new(logical_session).map_err(|error| invalid("sessionId", error))?;
        if current
            .as_ref()
            .is_some_and(|task| task.allow_execute == Some(false))
        {
            return Err(conversation_approval_denied());
        }
        let now = now_ms();
        let allow_execute = if let Some(task) = current.as_ref() {
            task.allow_execute
        } else if self.approve_new_conversations_enabled().await? {
            None
        } else {
            Some(true)
        };
        self.repository
            .upsert_task(&Task {
                id: task_id.clone(),
                agent_id: AgentId::new(&context.agent_id).ok(),
                device_id: self.device.id.clone(),
                conversation_scope_hash: if provider_scope.is_some() {
                    current
                        .as_ref()
                        .and_then(|task| task.conversation_scope_hash.clone())
                } else {
                    legacy_scope.clone().or_else(|| {
                        current
                            .as_ref()
                            .and_then(|task| task.conversation_scope_hash.clone())
                    })
                },
                title: current.as_ref().and_then(|task| task.title.clone()),
                source: current
                    .as_ref()
                    .and_then(|task| task.source.clone())
                    .or_else(|| Some("mcp".to_owned())),
                project_folder: current
                    .as_ref()
                    .and_then(|task| task.project_folder.clone()),
                allow_execute,
                status: if allow_execute.is_none() {
                    TaskStatus::Pending
                } else {
                    TaskStatus::Running
                },
                active_session_id: allow_execute.map(|_| session_id.clone()),
                generation,
                stopped_at_ms: None,
                created_at_ms: current.as_ref().map_or(now, |task| task.created_at_ms),
                updated_at_ms: now,
            })
            .await
            .map_err(storage_error)?;
        if allow_execute.is_none() {
            self.publish_event(
                format!("conversation-approval-pending-{}", task_id.as_str()),
                "conversation.approval_pending",
                Some(task_id.as_str().to_owned()),
                None,
                context.turn_id.clone(),
                serde_json::json!({
                    "allowExecute": serde_json::Value::Null,
                    "approvalDeadlineUtc": time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(now.saturating_add(60_000)) * 1_000_000)
                        .ok()
                        .and_then(|value| value.format(&time::format_description::well_known::Rfc3339).ok())
                }),
            );
            self.wait_for_conversation_approval(&task_id).await?;
            let Some(mut approved_task) = self
                .repository
                .task(&task_id)
                .await
                .map_err(storage_error)?
            else {
                return Err(RuntimeError::new(
                    "not_found",
                    "task missing after approval",
                ));
            };
            approved_task.status = TaskStatus::Running;
            approved_task.active_session_id = Some(session_id.clone());
            approved_task.updated_at_ms = now_ms();
            self.repository
                .upsert_task(&approved_task)
                .await
                .map_err(storage_error)?;
        }
        // A claimed browser child must retain its task identity on subsequent
        // calls that omit taskId. Otherwise they bootstrap an unrelated root
        // task and evade the server's child policy.
        self.claim_subagent_from_message(context, task_id.as_str(), first_user_message)
            .await?;
        if provider_scope.is_some() {
            self.bind_provider_scope_to_task(context, task_id.as_str())
                .await?;
        }
        self.repository
            .upsert_task_session(&TaskSession {
                task_id: task_id.clone(),
                session_id,
                generation,
                replaced_session_id: None,
                status: TerminalSessionStatus::Running,
                created_at_ms: now,
                updated_at_ms: now,
            })
            .await
            .map_err(storage_error)?;
        let agent_id =
            AgentId::new(&context.agent_id).map_err(|error| invalid("agentId", error))?;
        let turn_id = TurnId::new(context.turn_id.as_deref().unwrap_or_default())
            .map_err(|error| invalid("turnId", error))?;
        self.repository
            .bind_turn(&TurnBinding {
                agent_id,
                device_id: self.device.id.clone(),
                turn_id,
                task_id,
                last_used_at_ms: now,
            })
            .await
            .map_err(storage_error)
    }

    async fn approve_new_conversations_enabled(&self) -> RuntimeResult<bool> {
        let setting = self
            .repository
            .setting("ui_approveNewConversations")
            .await
            .map_err(storage_error)?;
        Ok(setting
            .and_then(|value| serde_json::from_str::<bool>(&value.value_json).ok())
            .unwrap_or(true))
    }

    async fn wait_for_conversation_approval(&self, task_id: &TaskId) -> RuntimeResult<()> {
        const APPROVAL_WINDOW_MS: i64 = 60_000;
        loop {
            let Some(task) = self.repository.task(task_id).await.map_err(storage_error)? else {
                return Err(RuntimeError::new(
                    "not_found",
                    "task missing while waiting for approval",
                ));
            };
            match task.allow_execute {
                Some(true) => return Ok(()),
                Some(false) => return Err(conversation_approval_denied()),
                None if now_ms().saturating_sub(task.created_at_ms) >= APPROVAL_WINDOW_MS => {
                    sqlx::query("UPDATE tasks SET allow_execute=0,status='failed',updated_at_ms=? WHERE id=? AND allow_execute IS NULL")
                        .bind(now_ms())
                        .bind(task_id.as_str())
                        .execute(self.repository.pool())
                        .await
                        .map_err(|_| RuntimeError::new("storage_error", "conversation approval timeout could not be persisted"))?;
                    return Err(conversation_approval_denied());
                }
                None => tokio::time::sleep(std::time::Duration::from_millis(250)).await,
            }
        }
    }

    async fn bound_task_for_conversation_scope(
        &self,
        agent_id: &str,
        conversation_scope: &str,
    ) -> RuntimeResult<Option<String>> {
        sqlx::query_scalar::<_, String>(
            "SELECT id FROM tasks WHERE agent_id=? AND conversation_scope_hash=? ORDER BY created_at_ms,id LIMIT 1",
        )
        .bind(agent_id)
        .bind(conversation_scope)
        .fetch_optional(self.repository.pool())
        .await
        .map_err(|_| RuntimeError::new("storage_error", "conversation task binding lookup failed"))
    }

    async fn chatgpt_bridge_task_for_message(
        &self,
        agent_id: &str,
        preferred_task_id: Option<&str>,
        message: &str,
    ) -> RuntimeResult<Option<String>> {
        let cutoff = now_ms().saturating_sub(5 * 60 * 1000);
        let rows = sqlx::query_as::<_, (String, String)>(
            r#"SELECT r.task_id,r.submitted_content
               FROM chatgpt_bridge_requests r
               JOIN tasks t ON t.id=r.task_id
               WHERE r.task_id IS NOT NULL
                 AND t.agent_id=? AND t.source='chatgpt_web'
                 AND r.status IN ('queued','running','stop_requested')
                 AND r.updated_at_ms>=?
               ORDER BY r.updated_at_ms DESC,r.id DESC
               LIMIT 32"#,
        )
        .bind(agent_id)
        .bind(cutoff)
        .fetch_all(self.repository.pool())
        .await
        .map_err(|_| {
            RuntimeError::new("storage_error", "ChatGPT bridge task binding lookup failed")
        })?;
        Ok(unique_bridge_task_for_message(
            &rows,
            preferred_task_id,
            message,
        ))
    }

    async fn unique_recent_chatgpt_bridge_task(
        &self,
        agent_id: &str,
    ) -> RuntimeResult<Option<String>> {
        let cutoff = now_ms().saturating_sub(90_000);
        let rows = sqlx::query_scalar::<_, String>(
            r#"SELECT r.task_id
               FROM chatgpt_bridge_requests r
               JOIN tasks t ON t.id=r.task_id
               WHERE r.task_id IS NOT NULL
                 AND t.agent_id=? AND t.source='chatgpt_web'
                 AND r.status IN ('queued','running','stop_requested')
                 AND r.updated_at_ms>=?
               GROUP BY r.task_id
               ORDER BY MAX(r.updated_at_ms) DESC
               LIMIT 2"#,
        )
        .bind(agent_id)
        .bind(cutoff)
        .fetch_all(self.repository.pool())
        .await
        .map_err(|_| {
            RuntimeError::new("storage_error", "pending ChatGPT bridge task lookup failed")
        })?;
        Ok((rows.len() == 1).then(|| rows[0].clone()))
    }

    async fn bound_task_for_turn(
        &self,
        context: &OperationContext,
    ) -> RuntimeResult<Option<String>> {
        let Some(turn_id) = context
            .turn_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(None);
        };
        let cutoff = now_ms().saturating_sub(2 * 60 * 60 * 1000);
        sqlx::query_scalar::<_, String>(
            "SELECT task_id FROM turn_bindings WHERE agent_id=? AND device_id=? AND turn_id=? AND last_used_at_ms>=? LIMIT 1",
        )
        .bind(&context.agent_id)
        .bind(self.device.id.as_str())
        .bind(turn_id)
        .bind(cutoff)
        .fetch_optional(self.repository.pool())
        .await
        .map_err(|_| RuntimeError::new("storage_error", "turn binding lookup failed"))
    }
}
