use crate::{
    OperationContext, PolicyContext, PolicyEngine, ProcessInfo, RuntimeError, RuntimeResult,
    process_visibility::{is_visible_process_name, public_process_details},
};
use tokio::process::Command;

#[derive(Clone)]
pub struct ProcessService {
    policy: PolicyEngine,
}

impl ProcessService {
    #[must_use]
    pub fn new(policy: PolicyEngine) -> Self {
        Self { policy }
    }

    pub async fn list(&self) -> RuntimeResult<Vec<ProcessInfo>> {
        let output = if cfg!(windows) {
            Command::new("tasklist.exe")
                .args(["/FO", "CSV", "/NH"])
                .output()
                .await
        } else {
            Command::new("ps")
                .args(["-eo", "pid=,comm="])
                .output()
                .await
        }
        .map_err(command_error)?;

        if !output.status.success() {
            return Err(RuntimeError::new(
                "process_list_failed",
                "task-relevant process inventory could not be read",
            ));
        }

        let text = String::from_utf8_lossy(&output.stdout);
        let mut values = Vec::new();
        for line in text.lines().take(10_000) {
            if cfg!(windows) {
                let fields: Vec<_> = line.trim_matches('"').split("\",\"").collect();
                if fields.len() >= 2
                    && is_visible_process_name(fields[0])
                    && let Ok(pid) = fields[1].replace(',', "").parse()
                {
                    values.push(ProcessInfo {
                        process_id: pid,
                        name: fields[0].into(),
                        details: public_process_details(),
                    });
                }
            } else {
                let mut parts = line.trim().splitn(2, char::is_whitespace);
                if let (Some(pid), Some(name)) = (parts.next(), parts.next())
                    && is_visible_process_name(name)
                    && let Ok(pid) = pid.parse()
                {
                    values.push(ProcessInfo {
                        process_id: pid,
                        name: name.into(),
                        details: public_process_details(),
                    });
                }
            }
        }
        Ok(values)
    }

    pub async fn inspect(&self, process_id: u32) -> RuntimeResult<ProcessInfo> {
        self.list()
            .await?
            .into_iter()
            .find(|process| process.process_id == process_id)
            .ok_or_else(|| RuntimeError::new("process_not_found", "process is not exposed"))
    }

    pub async fn kill(
        &self,
        context: &OperationContext,
        process_id: u32,
        entire_tree: bool,
    ) -> RuntimeResult<()> {
        // Hidden/background processes are intentionally outside this API, even if a caller
        // guesses a process identifier.
        self.inspect(process_id).await?;
        self.policy
            .authorize(&PolicyContext {
                agent_id: context.agent_id.clone(),
                tool_name: "process_kill".into(),
                root: None,
                destructive: true,
            })
            .await?;

        let output = if cfg!(windows) {
            let mut command = Command::new("taskkill.exe");
            command.args(["/PID", &process_id.to_string(), "/F"]);
            if entire_tree {
                command.arg("/T");
            }
            command.output().await
        } else {
            Command::new("kill")
                .args(["-TERM", &process_id.to_string()])
                .output()
                .await
        }
        .map_err(command_error)?;

        if output.status.success() {
            Ok(())
        } else {
            Err(RuntimeError::new(
                "process_kill_failed",
                "exposed process could not be stopped",
            ))
        }
    }
}

fn command_error(error: std::io::Error) -> RuntimeError {
    RuntimeError::new("process_start_failed", error.to_string())
}
