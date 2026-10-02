use std::path::{Path, PathBuf};

use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};

use super::{RuntimeHost, virtual_workspace::VirtualWorkspaceView};

impl RuntimeHost {
    /// Read current UI-approved scopes on every operation. Never persist these in a
    /// task or user message: doing so would turn revocable access into a lasting grant.
    pub(super) async fn shared_project_scopes(&self) -> RuntimeResult<Vec<PathBuf>> {
        let paths = sqlx::query_scalar::<_, String>(
            "SELECT global_access_path FROM workspace_projects WHERE allow_all_conversations=1 AND global_access_path IS NOT NULL ORDER BY canonical_path,id",
        )
        .fetch_all(self.repository.pool())
        .await
        .map_err(|_| RuntimeError::new("storage_error", "shared project access unavailable"))?;
        let mut scopes = Vec::new();
        for path in paths {
            let approved = PathBuf::from(path);
            // An unavailable folder or replaced symlink must not redirect the grant.
            if approved.is_absolute()
                && approved.parent().is_some()
                && approved.is_dir()
                && approved
                    .canonicalize()
                    .is_ok_and(|current| current == approved)
            {
                scopes.push(approved);
            }
        }
        scopes.sort();
        scopes.dedup();
        Ok(scopes)
    }

    pub(super) async fn effective_task_path_scopes(
        &self,
        context: &OperationContext,
    ) -> RuntimeResult<Vec<PathBuf>> {
        let mut scopes = self.task_user_path_scopes(context).await?;
        if context.task_id.as_deref().is_some_and(|id| !id.is_empty()) {
            scopes.extend(self.shared_project_scopes().await?);
        }
        scopes.sort();
        scopes.dedup();
        Ok(scopes)
    }

    /// A PTY retains its original cwd. Revalidate it before accepting new input or
    /// exposing output; a revoked shared scope must not survive via an old session.
    pub(super) async fn check_shell_path_access(
        &self,
        _context: &OperationContext,
        tool: &str,
        arguments: &serde_json::Value,
        scopes: &[PathBuf],
    ) -> RuntimeResult<()> {
        if !matches!(
            tool,
            "shell_write" | "shell_read" | "shell_wait" | "shell_resize" | "shell_inspect"
        ) {
            return Ok(());
        }
        let Some(id) = arguments
            .get("sessionId")
            .and_then(serde_json::Value::as_str)
        else {
            return Ok(()); // Normal input validation reports missing session IDs.
        };
        let session = self.shell.inspect(id).await?;
        self.workspace
            .with_additional_scopes(scopes)?
            .stat(&session.initial_working_directory)
            .await
            .map_err(|_| {
                RuntimeError::new(
                    "policy_denied",
                    "terminal working directory is no longer allowed",
                )
            })?;
        Ok(())
    }

    pub(super) async fn visible_workspace_roots(
        &self,
        context: &OperationContext,
        project_folder: Option<&Path>,
        workspace: &VirtualWorkspaceView,
    ) -> RuntimeResult<Vec<String>> {
        if context.task_id.is_none() {
            return Ok(Vec::new());
        }
        let mut aliases = if project_folder.is_some() {
            vec!["@project".to_owned()]
        } else {
            workspace.aliases()
        };
        for alias in workspace.shared_aliases() {
            if !aliases.contains(alias) {
                aliases.push(alias.clone());
            }
        }
        Ok(aliases)
    }
}

impl RuntimeHost {
    pub(super) async fn virtual_workspace_view(
        &self,
        context: &OperationContext,
    ) -> VirtualWorkspaceView {
        let project_folder =
            <Self as chatcmd_mcp::RuntimeApi>::project_folder(self, context.task_id.as_deref())
                .await
                .ok()
                .flatten()
                .map(PathBuf::from);

        let mut roots = self.workspace.roots().to_vec();
        match self.task_user_path_scopes(context).await {
            Ok(scopes) => roots.extend(scopes),
            Err(error) => {
                tracing::warn!(
                    code = %error.code,
                    "virtual workspace path scopes unavailable; configured roots remain masked"
                );
            }
        }
        if let Some(project) = project_folder.as_ref()
            && !roots.iter().any(|root| root == project)
        {
            roots.push(project.clone());
        }

        let shared = if context.task_id.is_some() {
            self.shared_project_scopes().await.unwrap_or_default()
        } else {
            Vec::new()
        };
        VirtualWorkspaceView::new(project_folder, roots).with_shared_roots(shared)
    }
}

#[cfg(test)]
#[path = "shared_project_access_tests.rs"]
mod tests;
