//! In-memory coordination for `fs_write_chatgpt_image`.
//!
//! The MCP tool registers a job and waits on its status. The local web UI
//! claims the job and asks the Chrome extension to open a new chatgpt.com
//! chat, where the prompt is sent and the generated images are downloaded
//! with the signed-in browser session. The extension posts the image bytes
//! back to the local API, which hands them to the saver task spawned by the
//! tool. Files are written by the runtime (never by the HTTP handler) so the
//! normal workspace scope and policy checks of the originating call apply.

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};

use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::{oneshot, watch};

/// Maximum number of images accepted from one ChatGPT answer.
pub(crate) const MAX_IMAGES: usize = 8;
/// Maximum decoded size of one image.
pub(crate) const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
/// HTTP body limit for the result callback (Base64 inflates by 4/3).
pub(crate) const MAX_RESULT_BODY_BYTES: usize = 96 * 1024 * 1024;
/// How long the browser side may take before the job fails.
pub(crate) const BROWSER_DEADLINE_MS: u64 = 10 * 60 * 1000;
/// Terminal jobs are kept this long so a caller can still resume with jobId.
const TERMINAL_TTL_MS: i64 = 60 * 60 * 1000;
/// A claimed job that never reports `started` becomes claimable again.
const CLAIM_STALE_MS: i64 = 90 * 1000;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImageJobRequest {
    pub job_id: String,
    pub prompt: String,
    pub model: String,
    pub target_path: String,
    pub created_at_ms: i64,
    pub deadline_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SavedImage {
    pub path: String,
    pub mime_type: String,
    pub bytes_written: u64,
    #[serde(skip)]
    pub source_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImageJobStatus {
    /// requested | dispatched | started | saving | completed | failed
    pub phase: String,
    pub files: Vec<SavedImage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assistant_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub updated_at_ms: i64,
}

impl ImageJobStatus {
    fn phase(phase: &str) -> Self {
        Self {
            phase: phase.to_owned(),
            files: Vec::new(),
            conversation_url: None,
            assistant_text: None,
            error_code: None,
            error_message: None,
            updated_at_ms: now_ms(),
        }
    }

    pub(crate) fn is_terminal(&self) -> bool {
        matches!(self.phase.as_str(), "completed" | "failed")
    }
}

#[derive(Debug, Clone)]
pub(crate) struct GeneratedImage {
    pub mime_type: String,
    pub bytes: Arc<Vec<u8>>,
    pub source_url: Option<String>,
}

#[derive(Debug)]
pub(crate) enum ImageDelivery {
    Images {
        images: Vec<GeneratedImage>,
        conversation_url: Option<String>,
        assistant_text: Option<String>,
    },
    Failed {
        message: String,
        conversation_url: Option<String>,
        assistant_text: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JobError {
    NotFound,
    Forbidden,
    AlreadyClaimed,
    AlreadyFinished,
}

struct Entry {
    request: ImageJobRequest,
    owner_agent_id: String,
    status: watch::Sender<ImageJobStatus>,
    delivery: Option<oneshot::Sender<ImageDelivery>>,
    claimed_at_ms: Option<i64>,
}

#[derive(Default)]
pub(crate) struct ImageJobRegistry {
    jobs: Mutex<HashMap<String, Entry>>,
}

pub(crate) static IMAGE_JOBS: LazyLock<ImageJobRegistry> =
    LazyLock::new(ImageJobRegistry::default);

impl ImageJobRegistry {
    /// Register a new job and return its status stream and delivery channel.
    pub(crate) fn create(
        &self,
        request: ImageJobRequest,
        owner_agent_id: &str,
    ) -> (watch::Receiver<ImageJobStatus>, oneshot::Receiver<ImageDelivery>) {
        let (status, receiver) = watch::channel(ImageJobStatus::phase("requested"));
        let (delivery, delivery_receiver) = oneshot::channel();
        let mut jobs = self.lock();
        purge_expired(&mut jobs);
        jobs.insert(
            request.job_id.clone(),
            Entry {
                request,
                owner_agent_id: owner_agent_id.to_owned(),
                status,
                delivery: Some(delivery),
                claimed_at_ms: None,
            },
        );
        (receiver, delivery_receiver)
    }

    /// Resume waiting on an existing job owned by the same agent and target.
    pub(crate) fn subscribe(
        &self,
        job_id: &str,
        owner_agent_id: &str,
        target_path: &str,
    ) -> Result<watch::Receiver<ImageJobStatus>, JobError> {
        let jobs = self.lock();
        let entry = jobs.get(job_id).ok_or(JobError::NotFound)?;
        if entry.owner_agent_id != owner_agent_id || entry.request.target_path != target_path {
            return Err(JobError::Forbidden);
        }
        Ok(entry.status.subscribe())
    }

    pub(crate) fn request(&self, job_id: &str) -> Option<ImageJobRequest> {
        self.lock().get(job_id).map(|entry| entry.request.clone())
    }

    pub(crate) fn status(&self, job_id: &str) -> Option<ImageJobStatus> {
        self.lock().get(job_id).map(|entry| entry.status.borrow().clone())
    }

    /// Jobs that still need a browser dispatch (used by UI recovery).
    pub(crate) fn pending(&self) -> Vec<ImageJobRequest> {
        let now = now_ms();
        let mut jobs = self.lock();
        purge_expired(&mut jobs);
        let mut pending = jobs
            .values()
            .filter(|entry| claimable(entry, now))
            .map(|entry| entry.request.clone())
            .collect::<Vec<_>>();
        pending.sort_by_key(|request| request.created_at_ms);
        pending
    }

    /// Atomically reserve a job for one browser dispatch.
    pub(crate) fn claim(&self, job_id: &str) -> Result<ImageJobRequest, JobError> {
        let now = now_ms();
        let mut jobs = self.lock();
        let entry = jobs.get_mut(job_id).ok_or(JobError::NotFound)?;
        if entry.status.borrow().is_terminal() || entry.delivery.is_none() {
            return Err(JobError::AlreadyFinished);
        }
        if !claimable(entry, now) {
            return Err(JobError::AlreadyClaimed);
        }
        entry.claimed_at_ms = Some(now);
        entry.status.send_modify(|status| {
            status.phase = "dispatched".to_owned();
            status.updated_at_ms = now;
        });
        Ok(entry.request.clone())
    }

    pub(crate) fn mark_started(
        &self,
        job_id: &str,
        conversation_url: Option<String>,
    ) -> Result<(), JobError> {
        let jobs = self.lock();
        let entry = jobs.get(job_id).ok_or(JobError::NotFound)?;
        if entry.status.borrow().is_terminal() || entry.delivery.is_none() {
            return Err(JobError::AlreadyFinished);
        }
        entry.status.send_modify(|status| {
            status.phase = "started".to_owned();
            if conversation_url.is_some() {
                status.conversation_url = conversation_url;
            }
            status.updated_at_ms = now_ms();
        });
        Ok(())
    }

    /// Hand the browser result to the saver task. Only the first delivery wins.
    pub(crate) fn deliver(&self, job_id: &str, delivery: ImageDelivery) -> Result<(), JobError> {
        let sender = {
            let mut jobs = self.lock();
            let entry = jobs.get_mut(job_id).ok_or(JobError::NotFound)?;
            entry.delivery.take().ok_or(JobError::AlreadyFinished)?
        };
        sender.send(delivery).map_err(|_| JobError::AlreadyFinished)
    }

    pub(crate) fn update(&self, job_id: &str, apply: impl FnOnce(&mut ImageJobStatus)) {
        let jobs = self.lock();
        if let Some(entry) = jobs.get(job_id) {
            entry.status.send_modify(|status| {
                apply(status);
                status.updated_at_ms = now_ms();
            });
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn claimable(entry: &Entry, now: i64) -> bool {
    if entry.delivery.is_none() {
        return false;
    }
    let status = entry.status.borrow();
    match status.phase.as_str() {
        "requested" => true,
        "dispatched" => entry
            .claimed_at_ms
            .is_none_or(|claimed| now.saturating_sub(claimed) > CLAIM_STALE_MS),
        _ => false,
    }
}

fn purge_expired(jobs: &mut HashMap<String, Entry>) {
    let now = now_ms();
    jobs.retain(|_, entry| {
        let status = entry.status.borrow();
        !(status.is_terminal() && now.saturating_sub(status.updated_at_ms) > TERMINAL_TTL_MS)
    });
}

/// Event/pending payload consumed by the web UI bridge.
pub(crate) fn request_value(request: &ImageJobRequest) -> Value {
    json!({
        "jobId": request.job_id,
        "prompt": request.prompt,
        "model": request.model,
        "createdAtMs": request.created_at_ms,
        "deadlineAtMs": request.deadline_at_ms,
    })
}

/// Validate a decoded image and return its canonical MIME type.
pub(crate) fn sniff_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else {
        None
    }
}

pub(crate) fn extension_for_mime(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "png",
    }
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(id: &str) -> ImageJobRequest {
        ImageJobRequest {
            job_id: id.to_owned(),
            prompt: "Create an image of a cat".to_owned(),
            model: "Auto".to_owned(),
            target_path: "D:/game/assets/cat.png".to_owned(),
            created_at_ms: now_ms(),
            deadline_at_ms: now_ms() + 1_000,
        }
    }

    #[tokio::test]
    async fn claim_is_exclusive_and_delivery_reaches_saver_once() {
        let registry = ImageJobRegistry::default();
        let (status, delivery) = registry.create(request("job-a"), "agent-1");
        assert_eq!(registry.pending().len(), 1);
        assert!(registry.claim("job-a").is_ok());
        assert_eq!(registry.claim("job-a"), Err(JobError::AlreadyClaimed));
        assert!(registry.pending().is_empty());
        assert_eq!(status.borrow().phase, "dispatched");

        registry.mark_started("job-a", Some("https://chatgpt.com/c/x".to_owned())).unwrap();
        assert_eq!(status.borrow().phase, "started");

        registry
            .deliver(
                "job-a",
                ImageDelivery::Failed { message: "no image".to_owned(), conversation_url: None, assistant_text: None },
            )
            .unwrap();
        assert!(matches!(delivery.await.unwrap(), ImageDelivery::Failed { .. }));
        assert!(matches!(
            registry.deliver(
                "job-a",
                ImageDelivery::Failed { message: "again".to_owned(), conversation_url: None, assistant_text: None },
            ),
            Err(JobError::AlreadyFinished)
        ));
        assert_eq!(registry.claim("job-a"), Err(JobError::AlreadyFinished));
    }

    #[test]
    fn subscribe_requires_same_owner_and_target() {
        let registry = ImageJobRegistry::default();
        let _channels = registry.create(request("job-b"), "agent-1");
        assert!(registry.subscribe("job-b", "agent-1", "D:/game/assets/cat.png").is_ok());
        assert_eq!(
            registry.subscribe("job-b", "agent-2", "D:/game/assets/cat.png").err(),
            Some(JobError::Forbidden)
        );
        assert_eq!(
            registry.subscribe("job-b", "agent-1", "D:/other.png").err(),
            Some(JobError::Forbidden)
        );
        assert_eq!(registry.subscribe("missing", "agent-1", "x").err(), Some(JobError::NotFound));
    }

    #[test]
    fn sniffs_supported_image_signatures() {
        assert_eq!(sniff_image_mime(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0]), Some("image/png"));
        assert_eq!(sniff_image_mime(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(sniff_image_mime(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff_image_mime(b"<html>"), None);
    }
}
