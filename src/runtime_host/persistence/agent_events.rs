use super::*;

impl RuntimeHost {
    pub(in crate::runtime_host) async fn save_agent_event(
        &self,
        context: &OperationContext,
        status: &str,
        content: &str,
        title: Option<&str>,
    ) -> RuntimeResult<Value> {
        let task_id = required_task_id(context)?;
        let turn_id = required_turn_id(context)?;
        if status == "completed"
            && !self
                .finish_subagent_for_child(task_id.as_str(), "completed")
                .await?
        {
            return Err(RuntimeError::new(
                "subagent_lease_lost",
                "child completion rejected because another terminal transition won",
            ));
        }
        if status == "completed" && content.trim().is_empty() {
            return Err(RuntimeError::new(
                "final_response_required",
                "agent_turn_complete content must contain the exact final user-facing response",
            ));
        }
        let now = now_ms();
        let session_id = required_session_id(context)?;
        let current = self
            .repository
            .task(&task_id)
            .await
            .map_err(storage_error)?;
        let mut task = current.ok_or_else(|| RuntimeError::new("not_found", "task missing"))?;
        task.status = if status == "completed" {
            TaskStatus::Completed
        } else {
            TaskStatus::Running
        };
        let title_allowed =
            status != "completed" || self.is_first_user_turn(&task_id, &turn_id).await?;
        let applied_title = title
            .filter(|value| !value.trim().is_empty())
            .filter(|_| title_allowed)
            .map(compact_task_title);
        if let Some(value) = applied_title.as_deref() {
            task.title = Some(value.to_owned());
        }
        task.updated_at_ms = now;
        self.repository
            .upsert_task(&task)
            .await
            .map_err(storage_error)?;
        if status == "completed" {
            if let Err(error) = self
                .reconcile_orphaned_tool_calls(
                    task_id.as_str(),
                    turn_id.as_str(),
                    Some(session_id.as_str()),
                    "turn_completed",
                    now,
                )
                .await
            {
                tracing::warn!(
                    task_id = %task_id,
                    turn_id = %turn_id,
                    error = ?error,
                    "failed to reconcile orphaned tool calls while completing turn"
                );
            }
        }
        let key = safe_id(
            "agent-event",
            &context.agent_id,
            &format!("{}\0{status}", context.request_id),
        );
        let event_kind = if status == "completed" {
            EventKind::Status
        } else {
            EventKind::Progress
        };
        let (file_changes, file_change_tracking_incomplete, file_change_events_dropped) =
            if status == "completed" {
                self.finish_turn_file_tracking(context).await
            } else {
                (Vec::new(), false, 0)
            };
        let payload = json!({"tool": context.tool_name, "status": status, "content": content, "title": applied_title.as_deref(),
            "fileChanges": file_changes, "fileChangeTrackingIncomplete": file_change_tracking_incomplete,
            "fileChangeEventsDropped": file_change_events_dropped, "fileChangeSchemaVersion": 2});
        self.repository
            .append_timeline_events(&[TimelineEvent {
                id: EventId::new(key.clone()).map_err(|error| invalid("eventId", error))?,
                task_id: task_id.clone(),
                turn_id: Some(turn_id.clone()),
                session_id: Some(session_id.clone()),
                actor: ActorKind::Assistant,
                kind: event_kind,
                idempotency_key: key.clone(),
                payload_json: payload.to_string(),
                metadata_json: None,
                created_at_ms: now,
            }])
            .await
            .map_err(storage_error)?;
        self.publish_event(
            key,
            event_kind.as_str(),
            Some(task_id.as_str().to_owned()),
            Some(session_id.as_str().to_owned()),
            Some(turn_id.as_str().to_owned()),
            payload,
        );
        Ok(
            json!({"accepted": true, "taskId": task_id.as_str(), "status": status, "titleUpdated": applied_title.is_some(), "title": applied_title}),
        )
    }
}
