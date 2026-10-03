use super::*;

impl RuntimeHost {
    pub(in crate::runtime_host) async fn persist_shell_session(
        &self,
        context: &OperationContext,
        info: &chatcmd_runtime::ShellSessionInfo,
    ) -> RuntimeResult<()> {
        self.repository
            .upsert_terminal_session(&TerminalSession {
                id: SessionId::new(&info.session_id)
                    .map_err(|error| invalid("sessionId", error))?,
                task_id: Some(required_task_id(context)?),
                turn_id: Some(required_turn_id(context)?),
                executable: info.executable.clone(),
                working_directory: info.initial_working_directory.display().to_string(),
                columns: i32::from(info.columns),
                rows: i32::from(info.rows),
                process_id: info.process_id.map(i64::from),
                status: TerminalSessionStatus::Running,
                exit_code: info.exit_code,
                created_at_ms: i64::try_from(info.created_at_unix_ms).unwrap_or(i64::MAX),
                updated_at_ms: now_ms(),
                closed_at_ms: None,
            })
            .await
            .map_err(storage_error)
    }

    pub(in crate::runtime_host) async fn persist_shell_events(
        &self,
        context: &OperationContext,
        result: &chatcmd_runtime::ShellReadResult,
    ) -> RuntimeResult<()> {
        let session_id =
            SessionId::new(&result.session_id).map_err(|error| invalid("sessionId", error))?;
        let task_id = required_task_id(context)?;
        let turn_id = required_turn_id(context)?;
        let chunks = result
            .events
            .iter()
            .map(|event| {
                Ok(TerminalEventChunk {
                    session_id: session_id.clone(),
                    sequence: i64::try_from(event.sequence)
                        .map_err(|error| invalid("sequence", error))?,
                    event_id: EventId::new(format!("{}:{}", result.session_id, event.sequence))
                        .map_err(|error| invalid("eventId", error))?,
                    task_id: Some(task_id.clone()),
                    turn_id: Some(turn_id.clone()),
                    kind: EventKind::TerminalOutput,
                    stream: Some(event.stream.clone()),
                    payload: if event.encoding == "base64" {
                        use base64::Engine as _;
                        base64::engine::general_purpose::STANDARD
                            .decode(&event.data)
                            .map_err(|error| invalid("terminal event data", error))?
                    } else {
                        event.data.as_bytes().to_vec()
                    },
                    payload_encoding: event.encoding.clone(),
                    created_at_ms: i64::try_from(event.timestamp_unix_ms).unwrap_or(i64::MAX),
                })
            })
            .collect::<RuntimeResult<Vec<_>>>()?;
        self.repository
            .append_terminal_chunks(&chunks)
            .await
            .map_err(storage_error)?;
        for event in &result.events {
            self.publish_event(
                format!("{}:{}", result.session_id, event.sequence),
                EventKind::TerminalOutput.as_str(),
                Some(task_id.as_str().to_owned()),
                Some(session_id.as_str().to_owned()),
                Some(turn_id.as_str().to_owned()),
                json!({ "text": event.data, "stream": event.stream, "encoding": event.encoding }),
            );
        }
        Ok(())
    }

    pub(in crate::runtime_host) async fn update_session_status(
        &self,
        session_id: &str,
        status: &str,
        exit_code: Option<i32>,
    ) -> RuntimeResult<()> {
        let now = now_ms();
        sqlx::query("UPDATE terminal_sessions SET status=?,exit_code=?,updated_at_ms=?,closed_at_ms=CASE WHEN ? IN ('closed','exited') THEN ? ELSE closed_at_ms END WHERE id=?")
            .bind(status).bind(exit_code).bind(now).bind(status).bind(now).bind(session_id)
            .execute(self.repository.pool()).await
            .map_err(|_| RuntimeError::new("storage_error", "session state could not be persisted"))?;
        Ok(())
    }

    pub(in crate::runtime_host) async fn reconcile_orphaned_tool_calls(
        &self,
        task_id: &str,
        turn_id: &str,
        fallback_session_id: Option<&str>,
        reason: &str,
        created_at_ms: i64,
    ) -> RuntimeResult<usize> {
        let rows = sqlx::query(
            "SELECT json_extract(start.payload_json,'$.activityId') AS activity_id, COALESCE(MAX(json_extract(start.payload_json,'$.tool')),'tool') AS tool, MAX(start.session_id) AS session_id FROM timeline_events start WHERE start.task_id=? AND start.turn_id=? AND start.kind='tool_call' AND COALESCE(json_extract(start.payload_json,'$.activityId'),'')<>'' AND COALESCE(json_extract(start.payload_json,'$.status'),'') IN ('started','pending_approval','stop_requested') AND COALESCE(json_extract(start.payload_json,'$.tool'),'') NOT IN ('agent_user_message','agent_progress','agent_subagent_start','agent_subagent_wait','agent_turn_complete') AND NOT EXISTS (SELECT 1 FROM timeline_events terminal WHERE terminal.task_id=start.task_id AND terminal.turn_id=start.turn_id AND terminal.kind='tool_result' AND json_extract(terminal.payload_json,'$.activityId')=json_extract(start.payload_json,'$.activityId')) GROUP BY json_extract(start.payload_json,'$.activityId')",
        )
        .bind(task_id)
        .bind(turn_id)
        .fetch_all(self.repository.pool())
        .await
        .map_err(|_| RuntimeError::new("storage_error", "orphaned tool activity lookup failed"))?;
        let task = TaskId::new(task_id).map_err(|error| invalid("taskId", error))?;
        let turn = TurnId::new(turn_id).map_err(|error| invalid("turnId", error))?;
        let mut reconciled = 0_usize;
        for row in rows {
            let activity_id = row.get::<String, _>("activity_id");
            let tool = row.get::<String, _>("tool");
            let session_value = row
                .get::<Option<String>, _>("session_id")
                .or_else(|| fallback_session_id.map(str::to_owned));
            let session = session_value
                .as_deref()
                .map(SessionId::new)
                .transpose()
                .map_err(|error| invalid("sessionId", error))?;
            let key = safe_id(
                "tool-reconcile",
                "runtime",
                &format!("{task_id}\0{turn_id}\0{activity_id}"),
            );
            let payload = json!({
                "activityId": activity_id,
                "tool": tool,
                "status": "interrupted",
                "errorCode": "tool_result_missing",
                "errorMessage": "tool execution ended without a terminal result before the turn completed",
                "reconciled": true,
                "reconcileReason": reason
            });
            let inserted = self
                .repository
                .append_timeline_events(&[TimelineEvent {
                    id: EventId::new(key.clone()).map_err(|error| invalid("eventId", error))?,
                    task_id: task.clone(),
                    turn_id: Some(turn.clone()),
                    session_id: session,
                    actor: ActorKind::Tool,
                    kind: EventKind::ToolResult,
                    idempotency_key: key.clone(),
                    payload_json: payload.to_string(),
                    metadata_json: None,
                    created_at_ms,
                }])
                .await
                .map_err(storage_error)?;
            if inserted == 0 {
                continue;
            }
            reconciled += inserted;
            self.publish_event(
                key,
                EventKind::ToolResult.as_str(),
                Some(task_id.to_owned()),
                session_value,
                Some(turn_id.to_owned()),
                payload,
            );
        }
        Ok(reconciled)
    }
}
