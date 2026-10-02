//! `fs_write_chatgpt_image`: generate images in the ChatGPT web app and save
//! them to any local path.
//!
//! The call returns immediately by default; a detached saver waits for the
//! browser result and writes the files automatically. Writes are not limited
//! to workspace roots (the user wants free destinations), so this module keeps
//! its own guardrails: only verified image bytes with a matching image
//! extension, no silent overwrite, no symlink targets, no OS/program folders.

use std::{
    io::Write as _,
    path::{Component, PathBuf},
};

use serde::Deserialize;
use tokio::sync::watch;

use super::*;
use crate::chatgpt_image_jobs::{
    self as jobs, BROWSER_DEADLINE_MS, GeneratedImage, IMAGE_JOBS, ImageDelivery,
    ImageJobRequest, ImageJobStatus, JobError, SavedImage,
};

const DEFAULT_WAIT_MS: u64 = 0;
const MAX_WAIT_MS: u64 = 280_000;
const MAX_PROMPT_CHARS: usize = 32_000;
const MAX_MISSING_PARENTS: usize = 16;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChatGptImageInput {
    path: String,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    job_id: Option<String>,
    #[serde(default)]
    overwrite: bool,
    #[serde(default)]
    wait_ms: Option<u64>,
    #[serde(default)]
    model: Option<String>,
}

/// Where generated files go: a directory (auto names) or a file stem.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ImageTarget {
    Directory(PathBuf),
    File { parent: PathBuf, stem: String, extension: Option<String> },
}

impl RuntimeHost {
    pub(super) async fn dispatch_chatgpt_image(
        &self,
        context: &OperationContext,
        arguments: Value,
    ) -> RuntimeResult<Value> {
        let input: ChatGptImageInput = parse(arguments)?;
        let wait = Duration::from_millis(input.wait_ms.unwrap_or(DEFAULT_WAIT_MS).min(MAX_WAIT_MS));
        let target_key = input.path.clone();

        // Optional status check / wait for an earlier job.
        if let Some(job_id) = input.job_id.as_deref().map(str::trim).filter(|id| !id.is_empty()) {
            let receiver = IMAGE_JOBS
                .subscribe(job_id, &context.agent_id, &target_key)
                .map_err(|error| job_error(error, job_id))?;
            return wait_for_job(context, job_id, receiver, wait).await;
        }

        let prompt = input
            .prompt
            .as_deref()
            .map(str::trim)
            .filter(|prompt| !prompt.is_empty())
            .ok_or_else(|| invalid("prompt", "is required unless jobId is provided"))?
            .to_owned();
        if prompt.chars().count() > MAX_PROMPT_CHARS {
            return Err(invalid("prompt", "is too long"));
        }
        let path = PathBuf::from(&input.path);
        let target = image_target(&input.path, &path)?;
        // Fail before spending a ChatGPT generation when the destination would
        // be refused later.
        preflight_target(&target, &path, input.overwrite)?;

        let now = jobs::now_ms();
        let job_id = format!("img-{}", uuid::Uuid::new_v4());
        let model = input
            .model
            .as_deref()
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .unwrap_or("Auto")
            .to_owned();
        let request = ImageJobRequest {
            job_id: job_id.clone(),
            prompt,
            model,
            target_path: target_key,
            created_at_ms: now,
            deadline_at_ms: now.saturating_add(i64::try_from(BROWSER_DEADLINE_MS).unwrap_or(i64::MAX)),
        };
        let (receiver, delivery) = IMAGE_JOBS.create(request.clone(), &context.agent_id);

        // The saver outlives this MCP call: the caller does not need to wait
        // or poll for the file to be written.
        let saver_host = self.clone();
        let saver_context = detached_context(context);
        let saver_job = job_id.clone();
        let overwrite = input.overwrite;
        tokio::spawn(async move {
            let delivery = tokio::select! {
                delivery = delivery => delivery.ok(),
                () = tokio::time::sleep(Duration::from_millis(BROWSER_DEADLINE_MS)) => None,
            };
            saver_host
                .save_image_delivery(&saver_context, &saver_job, &target, overwrite, delivery)
                .await;
        });

        self.publish_event(
            format!("image-generation-requested-{job_id}"),
            "image.generation_requested",
            context.task_id.clone(),
            None,
            context.turn_id.clone(),
            jobs::request_value(&request),
        );
        wait_for_job(context, &job_id, receiver, wait).await
    }

    async fn save_image_delivery(
        &self,
        context: &OperationContext,
        job_id: &str,
        target: &ImageTarget,
        overwrite: bool,
        delivery: Option<ImageDelivery>,
    ) {
        let (images, conversation_url, assistant_text) = match delivery {
            None => {
                fail_job(job_id, "image_generation_timeout", "Image generation did not complete in time.", None, None);
                self.publish_image_outcome(context, job_id);
                return;
            }
            Some(ImageDelivery::Failed { message, conversation_url, assistant_text }) => {
                fail_job(job_id, "image_generation_failed", &message, conversation_url, assistant_text);
                self.publish_image_outcome(context, job_id);
                return;
            }
            Some(ImageDelivery::Images { images, conversation_url, assistant_text }) => {
                (images, conversation_url, assistant_text)
            }
        };
        IMAGE_JOBS.update(job_id, |status| {
            status.phase = "saving".to_owned();
            status.conversation_url.clone_from(&conversation_url);
            status.assistant_text.clone_from(&assistant_text);
        });

        let stamp = file_stamp();
        let mut saved = Vec::with_capacity(images.len());
        for (index, image) in images.iter().enumerate() {
            let path = image_path(target, index, &image.mime_type, &stamp);
            match self.write_generated_image(context, &path, image, overwrite).await {
                Ok(bytes_written) => saved.push(SavedImage {
                    path: path.to_string_lossy().into_owned(),
                    mime_type: image.mime_type.clone(),
                    bytes_written,
                    source_url: image.source_url.clone(),
                }),
                Err(error) => {
                    let message = format!(
                        "saving {} failed: {} ({}){}",
                        path.display(),
                        error.message,
                        error.code,
                        if saved.is_empty() { String::new() } else { format!("; {} earlier image(s) were saved", saved.len()) }
                    );
                    IMAGE_JOBS.update(job_id, |status| {
                        status.phase = "failed".to_owned();
                        status.files = saved;
                        status.error_code = Some("image_save_failed".to_owned());
                        status.error_message = Some(message);
                    });
                    self.publish_image_outcome(context, job_id);
                    return;
                }
            }
        }
        IMAGE_JOBS.update(job_id, |status| {
            status.phase = "completed".to_owned();
            status.files = saved;
        });
        self.publish_image_outcome(context, job_id);
    }

    async fn write_generated_image(
        &self,
        context: &OperationContext,
        path: &Path,
        image: &GeneratedImage,
        overwrite: bool,
    ) -> RuntimeResult<u64> {
        let before = capture_snapshot(path);
        let kind = if path.exists() { FileChangeKind::Modified } else { FileChangeKind::Added };
        let owned_path = path.to_path_buf();
        let bytes = image.bytes.clone();
        let written = tokio::task::spawn_blocking(move || write_image_file(&owned_path, &bytes, overwrite))
            .await
            .map_err(|_| RuntimeError::new("io_error", "image writer task failed"))??;
        // Shows up in the turn's file changes when the path is inside the project.
        self.record_committed_change(context, path, None, kind, before, capture_snapshot(path), None, None);
        Ok(written)
    }

    /// Terminal notification for the UI/log; the caller never has to poll.
    fn publish_image_outcome(&self, context: &OperationContext, job_id: &str) {
        let Some(status) = IMAGE_JOBS.status(job_id) else { return };
        let event_type = if status.phase == "completed" { "image.generation_completed" } else { "image.generation_failed" };
        match status.phase.as_str() {
            "completed" => tracing::info!(job_id, files = status.files.len(), "Image generation job saved"),
            _ => tracing::warn!(job_id, error = status.error_message.as_deref().unwrap_or_default(), "Image generation job failed"),
        }
        self.publish_event(
            format!("{event_type}-{job_id}"),
            event_type,
            context.task_id.clone(),
            None,
            context.turn_id.clone(),
            json!({
                "jobId": job_id,
                "files": status.files,
                "errorCode": status.error_code,
                "errorMessage": status.error_message,
            }),
        );
    }
}

async fn wait_for_job(
    context: &OperationContext,
    job_id: &str,
    mut receiver: watch::Receiver<ImageJobStatus>,
    wait: Duration,
) -> RuntimeResult<Value> {
    if !wait.is_zero() {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            if receiver.borrow().is_terminal() {
                break;
            }
            tokio::select! {
                changed = receiver.changed() => if changed.is_err() { break },
                () = tokio::time::sleep_until(deadline) => break,
                () = context.cancellation.cancelled() => break,
            }
        }
    }
    let status = receiver.borrow().clone();
    let target_path = IMAGE_JOBS.request(job_id).map(|request| request.target_path);
    job_result(job_id, target_path.as_deref(), &status)
}

fn job_result(job_id: &str, target_path: Option<&str>, status: &ImageJobStatus) -> RuntimeResult<Value> {
    if status.phase == "failed" {
        let mut message = status
            .error_message
            .clone()
            .unwrap_or_else(|| "Image generation failed".to_owned());
        if let Some(text) = status.assistant_text.as_deref().filter(|text| !text.trim().is_empty()) {
            let text = text.chars().take(4_000).collect::<String>();
            message.push_str(&format!("\nGeneration response: {text}"));
        }
        if !status.files.is_empty() {
            let files = status.files.iter().map(|file| file.path.as_str()).collect::<Vec<_>>().join(", ");
            message.push_str(&format!("\nSaved before the failure: {files}"));
        }
        return Err(RuntimeError::new(
            status.error_code.clone().unwrap_or_else(|| "image_generation_failed".to_owned()),
            message,
        ));
    }
    let completed = status.phase == "completed";
    let mut result = json!({
        "jobId": job_id,
        "status": if completed { "completed" } else { "running" },
        "phase": status.phase,
        "path": target_path,
        "files": status.files,
    });
    if completed {
        if let Some(text) = status.assistant_text.as_deref().filter(|text| !text.trim().is_empty()) {
            result["generationMessage"] = Value::String(text.chars().take(2_000).collect());
        }
    } else {
        result["message"] = Value::String(format!(
            "Image generation is running and output will be saved automatically to {} when it finishes. To check later, call again with jobId={job_id} and the same path. Do not resend the prompt.",
            target_path.unwrap_or("the requested path")
        ));
    }
    Ok(result)
}

fn fail_job(
    job_id: &str,
    code: &str,
    message: &str,
    conversation_url: Option<String>,
    assistant_text: Option<String>,
) {
    IMAGE_JOBS.update(job_id, |status| {
        status.phase = "failed".to_owned();
        status.error_code = Some(code.to_owned());
        status.error_message = Some(message.to_owned());
        if conversation_url.is_some() {
            status.conversation_url = conversation_url;
        }
        if assistant_text.is_some() {
            status.assistant_text = assistant_text;
        }
    });
}

fn job_error(error: JobError, job_id: &str) -> RuntimeError {
    match error {
        JobError::NotFound => RuntimeError::new(
            "not_found",
            format!("image job {job_id} was not found (it may have expired or ChatCMD restarted)"),
        ),
        JobError::Forbidden => RuntimeError::new(
            "policy_denied",
            "image job belongs to another agent or a different path",
        ),
        JobError::AlreadyClaimed | JobError::AlreadyFinished => {
            RuntimeError::new("conflict", "image job state changed")
        }
    }
}

/// A fresh context for the background saver: same identity for bookkeeping,
/// but not cancelled when the MCP request ends.
fn detached_context(context: &OperationContext) -> OperationContext {
    let mut detached = OperationContext::new(
        format!("{}-save", context.request_id),
        context.agent_id.clone(),
        context.tool_name.clone(),
    );
    detached.task_id.clone_from(&context.task_id);
    detached.turn_id.clone_from(&context.turn_id);
    detached.mcp_session_id.clone_from(&context.mcp_session_id);
    detached.conversation_scope_id.clone_from(&context.conversation_scope_id);
    detached
}

fn image_target(raw: &str, path: &Path) -> RuntimeResult<ImageTarget> {
    if !path.is_absolute() {
        return Err(RuntimeError::new(
            "absolute_path_required",
            "path must be absolute (or relative to the task project folder when one is set)",
        ));
    }
    if path.components().any(|component| matches!(component, Component::ParentDir)) {
        return Err(invalid("path", "must not contain '..' segments"));
    }
    if raw.ends_with('/') || raw.ends_with('\\') || path.is_dir() {
        return Ok(ImageTarget::Directory(path.to_path_buf()));
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| invalid("path", "must include a parent directory"))?
        .to_path_buf();
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .ok_or_else(|| invalid("path", "must name a file or directory"))?;
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .filter(|extension| !extension.is_empty());
    Ok(ImageTarget::File { parent, stem, extension })
}

fn image_path(target: &ImageTarget, index: usize, mime: &str, stamp: &str) -> PathBuf {
    let extension = jobs::extension_for_mime(mime);
    match target {
        ImageTarget::Directory(directory) => {
            let suffix = if index == 0 { String::new() } else { format!("_{}", index + 1) };
            directory.join(format!("generated-image-{stamp}{suffix}.{extension}"))
        }
        ImageTarget::File { parent, stem, extension: requested } => {
            // Keep the caller's spelling when it matches the real type (jpg/jpeg).
            let extension = match requested.as_deref() {
                Some(requested) if requested == extension || (extension == "jpg" && requested == "jpeg") => requested,
                _ => extension,
            };
            let suffix = if index == 0 { String::new() } else { format!("_{}", index + 1) };
            parent.join(format!("{stem}{suffix}.{extension}"))
        }
    }
}

fn target_directory(target: &ImageTarget) -> &Path {
    match target {
        ImageTarget::Directory(directory) => directory,
        ImageTarget::File { parent, .. } => parent,
    }
}

fn preflight_target(target: &ImageTarget, path: &Path, overwrite: bool) -> RuntimeResult<()> {
    let directory = target_directory(target);
    guard_location(directory)?;
    if directory.exists() && !directory.is_dir() {
        return Err(invalid("path", "parent is not a directory"));
    }
    let missing = directory.ancestors().take_while(|ancestor| !ancestor.exists()).count();
    if missing > MAX_MISSING_PARENTS {
        return Err(invalid("path", "too many missing parent directories"));
    }
    if matches!(target, ImageTarget::File { .. }) && path.is_file() && !overwrite {
        return Err(RuntimeError::new(
            "targetExists",
            "target image already exists; pass overwrite=true or choose another path",
        ));
    }
    Ok(())
}

/// Refuse OS and installed-program folders even though other paths are free.
fn guard_location(path: &Path) -> RuntimeResult<()> {
    let normalized = normalize_for_compare(path);
    let protected = ["SystemRoot", "windir", "ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"]
        .iter()
        .filter_map(|name| std::env::var_os(name))
        .map(|value| normalize_for_compare(Path::new(&value)))
        .filter(|value| !value.is_empty());
    for root in protected {
        if normalized == root || normalized.starts_with(&format!("{root}/")) {
            return Err(RuntimeError::new(
                "protected_location",
                format!("refusing to write generated images under {}", path.display()),
            ));
        }
    }
    Ok(())
}

fn normalize_for_compare(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let text = text.trim_end_matches('/');
    if cfg!(windows) { text.to_ascii_lowercase() } else { text.to_owned() }
}

/// Write via a sibling temp file then rename, so a partially written image
/// never appears at the destination.
fn write_image_file(path: &Path, bytes: &[u8], overwrite: bool) -> RuntimeResult<u64> {
    let io = |error: std::io::Error| RuntimeError::new("io_error", error.to_string());
    let parent = path
        .parent()
        .ok_or_else(|| invalid("path", "must include a parent directory"))?;
    guard_location(parent)?;
    std::fs::create_dir_all(parent).map_err(io)?;
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || metadata.is_dir() => {
            return Err(RuntimeError::new(
                "target_not_regular_file",
                "refusing to write through a symlink or over a directory",
            ));
        }
        Ok(_) if !overwrite => {
            return Err(RuntimeError::new(
                "targetExists",
                "target image already exists; pass overwrite=true or choose another path",
            ));
        }
        _ => {}
    }
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "image".to_owned());
    let temp = parent.join(format!(".{file_name}.{}.chatcmd-tmp", uuid::Uuid::new_v4().simple()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temp);
        return Err(io(error));
    }
    Ok(u64::try_from(bytes.len()).unwrap_or(u64::MAX))
}

fn file_stamp() -> String {
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn absolute(path: &str) -> PathBuf {
        std::env::temp_dir().join(path)
    }

    #[test]
    fn file_target_keeps_stem_and_follows_real_image_type() {
        let path = absolute("game/assets/hero.png");
        let target = image_target(&path.to_string_lossy(), &path).unwrap();
        assert_eq!(image_path(&target, 0, "image/png", "s"), absolute("game/assets/hero.png"));
        assert_eq!(image_path(&target, 1, "image/png", "s"), absolute("game/assets/hero_2.png"));
        assert_eq!(image_path(&target, 0, "image/webp", "s"), absolute("game/assets/hero.webp"));

        let jpeg_path = absolute("game/a.jpeg");
        let jpeg = image_target(&jpeg_path.to_string_lossy(), &jpeg_path).unwrap();
        assert_eq!(image_path(&jpeg, 0, "image/jpeg", "s"), absolute("game/a.jpeg"));

        let bare_path = absolute("game/icon");
        let bare = image_target(&bare_path.to_string_lossy(), &bare_path).unwrap();
        assert_eq!(image_path(&bare, 0, "image/png", "s"), absolute("game/icon.png"));
    }

    #[test]
    fn trailing_separator_selects_directory_mode() {
        let directory = absolute("game/generated");
        let raw = format!("{}/", directory.to_string_lossy());
        let target = image_target(&raw, Path::new(&raw)).unwrap();
        assert!(matches!(target, ImageTarget::Directory(_)));
        assert_eq!(
            image_path(&target, 2, "image/png", "20260930-101500").file_name().unwrap(),
            "generated-image-20260930-101500_3.png"
        );
    }

    #[test]
    fn relative_and_parent_segments_are_rejected() {
        assert_eq!(image_target("assets/x.png", Path::new("assets/x.png")).unwrap_err().code, "absolute_path_required");
        let sneaky = absolute("game/../x.png");
        assert!(image_target(&sneaky.to_string_lossy(), &sneaky).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn os_folders_are_protected() {
        let windows = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()));
        assert_eq!(guard_location(&windows.join("System32")).unwrap_err().code, "protected_location");
        assert!(guard_location(&std::env::temp_dir()).is_ok());
    }

    #[test]
    fn writer_creates_parents_and_refuses_silent_overwrite() {
        let root = std::env::temp_dir().join(format!("chatcmd-image-test-{}", uuid::Uuid::new_v4().simple()));
        let path = root.join("nested/deeper/hero.png");
        assert_eq!(write_image_file(&path, b"first", false).unwrap(), 5);
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
        assert_eq!(write_image_file(&path, b"second", false).unwrap_err().code, "targetExists");
        write_image_file(&path, b"second", true).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        let leftovers = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".chatcmd-tmp"))
            .count();
        assert_eq!(leftovers, 0);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn call_returns_immediately_and_saver_writes_the_delivered_image() {
        let (host, agent_id, _directory) = crate::runtime_host::user_message_tests::test_host().await;
        let root = std::env::temp_dir().join(format!("chatcmd-image-e2e-{}", uuid::Uuid::new_v4().simple()));
        let target = root.join("assets/ui/potion.png");
        let started = std::time::Instant::now();
        let value = host
            .dispatch_chatgpt_image(
                &OperationContext::new("image-e2e", agent_id.clone(), "fs_write_chatgpt_image"),
                json!({ "path": target.to_string_lossy(), "prompt": "Create an image of a red potion" }),
            )
            .await
            .expect("image job accepted");
        assert!(started.elapsed() < Duration::from_secs(2), "default call must not block");
        assert_eq!(value["status"], "running");
        let job_id = value["jobId"].as_str().unwrap().to_owned();

        IMAGE_JOBS.claim(&job_id).expect("claim");
        let png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 7, 7];
        IMAGE_JOBS
            .deliver(&job_id, ImageDelivery::Images {
                images: vec![GeneratedImage { mime_type: "image/png".to_owned(), bytes: std::sync::Arc::new(png.clone()), source_url: Some("https://files.oaiusercontent.com/internal.png".to_owned()) }],
                conversation_url: Some("https://chatgpt.com/c/e2e".to_owned()),
                assistant_text: None,
            })
            .expect("deliver");

        let done = host
            .dispatch_chatgpt_image(
                &OperationContext::new("image-e2e-check", agent_id, "fs_write_chatgpt_image"),
                json!({ "path": target.to_string_lossy(), "jobId": job_id, "waitMs": 5_000 }),
            )
            .await
            .expect("status");
        assert_eq!(done["status"], "completed");
        assert!(done.get("conversationUrl").is_none());
        assert!(serde_json::to_string(&done).unwrap().find("oaiusercontent").is_none());
        assert_eq!(std::fs::read(&target).unwrap(), png);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn failed_status_becomes_generic_structured_error() {
        let status = ImageJobStatus {
            phase: "failed".to_owned(),
            files: Vec::new(),
            conversation_url: Some("https://chatgpt.com/c/abc".to_owned()),
            assistant_text: Some("What style do you want?".to_owned()),
            error_code: Some("image_generation_failed".to_owned()),
            error_message: Some("no generated image was found".to_owned()),
            updated_at_ms: 0,
        };
        let error = job_result("img-1", Some("D:/x.png"), &status).unwrap_err();
        assert_eq!(error.code, "image_generation_failed");
        assert!(error.message.contains("Generation response: What style do you want?"));
        assert!(!error.message.contains("chatgpt.com"));
        assert!(!error.message.contains("ChatGPT"));
    }

    #[test]
    fn running_status_says_the_file_is_saved_automatically() {
        let status = ImageJobStatus {
            phase: "requested".to_owned(),
            files: Vec::new(),
            conversation_url: None,
            assistant_text: None,
            error_code: None,
            error_message: None,
            updated_at_ms: 0,
        };
        let value = job_result("img-2", Some("D:/x.png"), &status).unwrap();
        assert_eq!(value["status"], "running");
        let message = value["message"].as_str().unwrap();
        assert!(message.contains("saved automatically to D:/x.png"));
        assert!(message.contains("To check later"));
        assert!(!message.contains("ChatGPT"));
        assert!(!message.contains("browser"));
    }
}
