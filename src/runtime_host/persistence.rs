use chatcmd_core::{
    ActorKind, Artifact, ArtifactId, ArtifactStore as _, EventId, EventKind, SessionId, TaskId,
    TaskStatus, TaskStore as _, TerminalEventChunk, TerminalEventStore as _, TerminalSession,
    TerminalSessionStatus, TimelineEvent, TurnId,
};
use chatcmd_runtime::{
    OperationContext, RuntimeError, RuntimeResult, ToolPhase, ToolStatus, ToolUsage,
};
use serde_json::{Value, json};
use sqlx::Row as _;
use tracing::Instrument as _;
use uuid::Uuid;

use super::{
    RuntimeHost, invalid, now_ms, storage_error,
    tool_event_projection::{EventLimits, bounded_error_message, project},
    user_message::compact_task_title,
};

impl RuntimeHost {
    pub(super) async fn call_persisted(
        &self,
        tool: &str,
        context: OperationContext,
        arguments: Value,
    ) -> RuntimeResult<Value> {
        let telemetry = self.telemetry.start(&context, tool);
        telemetry.set_phase(ToolPhase::Authorizing);
        let span = telemetry.span();
        let result = self
            .call_persisted_inner(tool, context, arguments, &telemetry)
            .instrument(span)
            .await;
        if tool.starts_with("blob_") {
            self.telemetry.set_blob_bytes(self.blob_store.usage_bytes());
        }
        let (status, error_code) = match &result {
            Ok(_) => (ToolStatus::Success, None),
            Err(error) if is_timeout_code(&error.code) => {
                (ToolStatus::Timeout, Some(error.code.as_str()))
            }
            Err(error) if is_cancel_code(&error.code) => {
                (ToolStatus::Cancelled, Some(error.code.as_str()))
            }
            Err(error) => (ToolStatus::Failure, Some(error.code.as_str())),
        };
        let usage = result
            .as_ref()
            .ok()
            .map(tool_usage_from_value)
            .unwrap_or_default();
        if result.as_ref().ok().is_some_and(tool_result_has_artifact) {
            telemetry.mark_artifact_created();
        }
        let truncated = result.as_ref().ok().is_some_and(tool_result_is_truncated);
        telemetry.finish(status, usage, error_code, truncated);
        result
    }

    async fn call_persisted_inner(
        &self,
        tool: &str,
        mut context: OperationContext,
        arguments: Value,
        telemetry: &chatcmd_runtime::ToolCallTelemetry,
    ) -> RuntimeResult<Value> {
        self.authorize_tool(&context.agent_id, tool).await?;
        let first_user_message = (tool == "agent_user_message")
            .then(|| arguments.get("content").and_then(Value::as_str))
            .flatten();
        let selected_subagent_id = (tool == "agent_subagent_wait")
            .then(|| arguments.get("subagentId").and_then(Value::as_str))
            .flatten();
        self.ensure_call_identity_for_call(&mut context, first_user_message, selected_subagent_id)
            .await?;
        telemetry.update_context(&context);
        if let Some(task_id) = context.task_id.as_deref()
            && let Err(error) = self.heartbeat_subagent(task_id).await
        {
            tracing::warn!(code = %error.code, "sub-agent activity heartbeat failed");
        }
        let request_workspace = self.virtual_workspace_view(&context).await;
        let arguments = request_workspace.resolve_alias_arguments(tool, arguments)?;
        if let Err(mut error) = self.authorize_execution(&context, tool, &arguments).await {
            self.virtual_workspace_view(&context)
                .await
                .project_error(&mut error);
            self.append_call_event(&context, tool, "failed", None, None, Some(&error))
                .await?;
            return Err(error);
        }
        telemetry.set_phase(phase_for_tool(tool));
        let _activity_guard = self.activities.register(&context, tool, &arguments);
        let public_arguments = request_workspace.project_tool_output(tool, arguments.clone());
        self.append_call_event(
            &context,
            tool,
            "started",
            Some(&public_arguments),
            None,
            None,
        )
        .await?;

        let dispatch = self.dispatch(tool, context.clone(), arguments);
        tokio::pin!(dispatch);
        let mut heartbeat = tokio::time::interval_at(
            tokio::time::Instant::now() + std::time::Duration::from_secs(10),
            std::time::Duration::from_secs(10),
        );
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let result = loop {
            tokio::select! {
            result = &mut dispatch => break result,
            () = context.cancellation.cancelled() => break {
                let reason = self.activities.stop_reason(&context.request_id);
                // Dropping a dispatch future does not stop an active blocking worker. Continue
                // polling until it reaches a cooperative checkpoint and completes cleanup. If
                // it committed before observing cancellation, preserve the committed result.
                match dispatch.await {
                    Ok(output) => Ok(output),
                    Err(error) if error.code != "operationCancelled" && error.code != "cancelled" => Err(error),
                    Err(_) => Err(RuntimeError::new(
                        "activity_stopped",
                        reason.map_or_else(
                            || "the user stopped this activity after worker cleanup".to_owned(),
                            |value| format!("the user stopped this activity after worker cleanup. Reason: {value}"),
                        ),
                    )),
                }
            },
            _ = heartbeat.tick() => {
                if let Some(task_id) = context.task_id.as_deref()
                    && let Err(error) = self.heartbeat_subagent(task_id).await
                {
                    tracing::warn!(code = %error.code, "sub-agent timer heartbeat failed");
                }
            },
            }
        };
        match result {
            Ok(output) => {
                let virtual_workspace = self.virtual_workspace_view(&context).await;
                let output = virtual_workspace.project_tool_output(tool, output);
                telemetry.update_usage(tool_usage_from_value(&output));
                telemetry.set_phase(ToolPhase::CleaningUp);
                if let Err(error) = self
                    .append_call_event(&context, tool, "succeeded", None, Some(&output), None)
                    .await
                {
                    tracing::warn!(
                        tool,
                        error_code = error.code,
                        "tool succeeded but its bounded timeline event could not be persisted"
                    );
                }
                let output = self
                    .attach_immediate_messages(&context, tool, output)
                    .await?;
                let output = virtual_workspace.project_tool_output(tool, output);
                Ok(enrich_tool_result(output, &context, tool))
            }
            Err(mut error) => {
                self.virtual_workspace_view(&context)
                    .await
                    .project_error(&mut error);
                telemetry.set_phase(if is_cancel_code(&error.code) {
                    ToolPhase::RollingBack
                } else {
                    ToolPhase::CleaningUp
                });
                let status = if error.code == "activity_stopped" {
                    "stopped"
                } else {
                    "failed"
                };
                self.append_call_event(&context, tool, status, None, None, Some(&error))
                    .await?;
                Err(error)
            }
        }
    }

    pub(super) async fn append_call_event(
        &self,
        context: &OperationContext,
        tool: &str,
        status: &str,
        input: Option<&Value>,
        output: Option<&Value>,
        error: Option<&RuntimeError>,
    ) -> RuntimeResult<()> {
        let task_id = required_task_id(context)?;
        let turn_id = required_turn_id(context)?;
        let session_id = required_session_id(context)?;
        let key = safe_id(
            "mcp-event",
            &context.agent_id,
            &format!("{}\0{status}", context.request_id),
        );
        let limits = EventLimits::default();
        let mut received_bytes = 0_u64;
        let mut redactions = 0_u64;
        let mut truncated = false;
        let mut externalized_bytes = 0_u64;
        let mut externalization_failed = false;
        let mut payload = json!({
            "activityId": context.request_id,
            "tool": tool,
            "status": status,
            "schemaVersion": 2
        });
        if let Some(value) = input {
            let projection = project(tool, value, limits);
            received_bytes = received_bytes
                .saturating_add(u64::try_from(projection.received_bytes).unwrap_or(u64::MAX));
            redactions = redactions
                .saturating_add(u64::try_from(projection.redactions.len()).unwrap_or(u64::MAX));
            truncated |= projection.truncated;
            add_projection_metadata(&mut payload, "input", &projection);
            payload["input"] = projection.public_summary;
        }
        if let Some(value) = output {
            let projection = project(tool, value, limits);
            received_bytes = received_bytes
                .saturating_add(u64::try_from(projection.received_bytes).unwrap_or(u64::MAX));
            redactions = redactions
                .saturating_add(u64::try_from(projection.redactions.len()).unwrap_or(u64::MAX));
            truncated |= projection.truncated;
            add_projection_metadata(&mut payload, "output", &projection);
            if should_externalize_tool_output(tool, &projection) {
                match self
                    .externalize_tool_output(context, value, projection.received_bytes)
                    .await
                {
                    Ok(Some((artifact_id, size_bytes))) => {
                        externalized_bytes = externalized_bytes.saturating_add(size_bytes);
                        payload["payloadExternalized"] = Value::Bool(true);
                        payload["artifactRef"] = Value::String(artifact_id);
                        payload["artifactSizeBytes"] = json!(size_bytes);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        externalization_failed = true;
                        payload["externalizationFailed"] = Value::Bool(true);
                        payload["externalizationErrorCode"] = Value::String(error.code);
                    }
                }
            }
            payload["output"] = projection.public_summary;
        }
        if let Some(value) = error {
            payload["errorCode"] = Value::String(value.code.clone());
            let (message, error_truncated) = bounded_error_message(&value.message, limits);
            payload["errorMessage"] = Value::String(message);
            if error_truncated {
                payload["errorTruncated"] = Value::Bool(true);
                truncated = true;
            }
        }
        let event_kind = if status == "started" || status == "pending_approval" {
            EventKind::ToolCall
        } else {
            EventKind::ToolResult
        };
        let task_value = task_id.as_str().to_owned();
        let turn_value = turn_id.as_str().to_owned();
        let session_value = session_id.as_str().to_owned();
        let payload_json = payload.to_string();
        let payload_bytes = u64::try_from(payload_json.len()).unwrap_or(u64::MAX);
        let event = TimelineEvent {
            id: EventId::new(key.clone()).map_err(|error| invalid("eventId", error))?,
            task_id,
            turn_id: Some(turn_id),
            session_id: Some(session_id),
            actor: ActorKind::Tool,
            kind: event_kind,
            idempotency_key: key.clone(),
            payload_json,
            metadata_json: None,
            created_at_ms: now_ms(),
        };
        self.repository
            .append_timeline_events(&[event])
            .await
            .map_err(storage_error)?;
        self.telemetry.record_event_projection(
            received_bytes,
            payload_bytes,
            payload_bytes,
            externalized_bytes,
            redactions,
            truncated,
            externalization_failed,
        );
        self.publish_event(
            key,
            event_kind.as_str(),
            Some(task_value),
            Some(session_value),
            Some(turn_value),
            payload,
        );
        Ok(())
    }

    async fn externalize_tool_output(
        &self,
        context: &OperationContext,
        value: &Value,
        received_bytes: usize,
    ) -> RuntimeResult<Option<(String, u64)>> {
        if received_bytes < 128 * 1024 {
            return Ok(None);
        }
        let managed = self
            .blob_store
            .store_artifact_json(context, value, 24 * 60 * 60)?;
        self.telemetry.set_blob_bytes(self.blob_store.usage_bytes());
        let managed_size_bytes = managed.size_bytes;
        let artifact_id = ArtifactId::new(format!("artifact-{}", Uuid::new_v4()))
            .map_err(|error| invalid("artifactId", error))?;
        let size_bytes = i64::try_from(managed_size_bytes).map_err(|_| {
            RuntimeError::new("artifactTooLarge", "artifact size cannot be represented")
        })?;
        let timestamp = now_ms();
        let artifact = Artifact {
            id: artifact_id.clone(),
            task_id: required_task_id(context)?,
            // artifact_registry.session_id references terminal_sessions, not MCP sessions.
            session_id: None,
            relative_path: format!("{}{}", super::MANAGED_ARTIFACT_PREFIX, managed.content_ref),
            media_type: Some("application/vnd.chatcmd.tool-output+json".to_owned()),
            size_bytes,
            sha256_hex: Some(managed.sha256),
            created_at_ms: timestamp,
            updated_at_ms: timestamp,
        };
        self.repository
            .register_artifact(&artifact)
            .await
            .map_err(storage_error)?;
        Ok(Some((artifact_id.into_string(), managed_size_bytes)))
    }
}

mod agent_events;
mod helpers;
mod sessions;

use helpers::*;
