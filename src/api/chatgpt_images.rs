//! Local callbacks for `fs_write_chatgpt_image`.
//!
//! * `GET  /chatgpt/images/pending`       UI recovery after reconnect.
//! * `POST /chatgpt/images/{id}/claim`    UI reserves one browser dispatch.
//! * `POST /chatgpt/images/{id}/started`  extension: prompt was submitted.
//! * `POST /chatgpt/images/{id}/result`   extension/UI: images or failure.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::chatgpt_image_jobs::{
    GeneratedImage, IMAGE_JOBS, ImageDelivery, JobError, MAX_IMAGE_BYTES, MAX_IMAGES,
    request_value, sniff_image_mime,
};
use crate::websocket::AppState;

use super::Problem;

const MAX_TEXT_CHARS: usize = 20_000;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ImageStarted {
    conversation_url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ImageResultItem {
    mime_type: Option<String>,
    data_base64: String,
    source_url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ImageResult {
    status: String,
    #[serde(default)]
    images: Vec<ImageResultItem>,
    conversation_url: Option<String>,
    assistant_text: Option<String>,
    error_message: Option<String>,
}

pub(super) async fn pending_chatgpt_images(
    State(_state): State<Arc<AppState>>,
) -> Json<Vec<Value>> {
    Json(IMAGE_JOBS.pending().iter().map(request_value).collect())
}

pub(super) async fn claim_chatgpt_image(
    State(_state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Result<Json<Value>, Problem> {
    let request = IMAGE_JOBS.claim(job_id.trim()).map_err(job_problem)?;
    Ok(Json(request_value(&request)))
}

pub(super) async fn chatgpt_image_started(
    State(_state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
    Json(input): Json<ImageStarted>,
) -> Result<Json<Value>, Problem> {
    let conversation_url = chatgpt_url(input.conversation_url.as_deref());
    IMAGE_JOBS
        .mark_started(job_id.trim(), conversation_url)
        .map_err(job_problem)?;
    Ok(Json(json!({ "accepted": true })))
}

pub(super) async fn chatgpt_image_result(
    State(_state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
    Json(input): Json<ImageResult>,
) -> Result<Json<Value>, Problem> {
    let job_id = job_id.trim();
    let conversation_url = chatgpt_url(input.conversation_url.as_deref());
    let assistant_text = bounded_text(input.assistant_text.as_deref());
    let delivery = match input.status.as_str() {
        "completed" => {
            let images = decode_images(input.images)?;
            if images.is_empty() {
                ImageDelivery::Failed {
                    message: "Image generation finished without a generated image.".to_owned(),
                    conversation_url,
                    assistant_text,
                }
            } else {
                ImageDelivery::Images { images, conversation_url, assistant_text }
            }
        }
        "failed" | "stopped" => ImageDelivery::Failed {
            message: bounded_text(input.error_message.as_deref())
                .unwrap_or_else(|| "The ChatGPT browser bridge reported a failure.".to_owned()),
            conversation_url,
            assistant_text,
        },
        _ => {
            return Err(Problem::new(
                StatusCode::BAD_REQUEST,
                "Invalid image result",
                "status must be completed, failed, or stopped",
            ));
        }
    };
    let count = match &delivery {
        ImageDelivery::Images { images, .. } => images.len(),
        ImageDelivery::Failed { .. } => 0,
    };
    IMAGE_JOBS.deliver(job_id, delivery).map_err(job_problem)?;
    Ok(Json(json!({ "accepted": true, "images": count })))
}

fn decode_images(items: Vec<ImageResultItem>) -> Result<Vec<GeneratedImage>, Problem> {
    if items.len() > MAX_IMAGES {
        return Err(bad_image(format!("at most {MAX_IMAGES} images are accepted")));
    }
    items
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            if item.data_base64.len() > MAX_IMAGE_BYTES / 3 * 4 + 8 {
                return Err(bad_image(format!("image {} is larger than 32 MiB", index + 1)));
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(item.data_base64.trim())
                .map_err(|_| bad_image(format!("image {} is not valid Base64", index + 1)))?;
            if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
                return Err(bad_image(format!("image {} has an invalid size", index + 1)));
            }
            // Trust the bytes, not the declared MIME type.
            let mime_type = sniff_image_mime(&bytes).ok_or_else(|| {
                bad_image(format!(
                    "image {} is not PNG/JPEG/WebP/GIF (declared {})",
                    index + 1,
                    item.mime_type.as_deref().unwrap_or("unknown")
                ))
            })?;
            Ok(GeneratedImage {
                mime_type: mime_type.to_owned(),
                bytes: Arc::new(bytes),
                source_url: item
                    .source_url
                    .filter(|url| url.starts_with("https://"))
                    .map(|url| url.chars().take(2_048).collect()),
            })
        })
        .collect()
}

fn chatgpt_url(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|url| url.starts_with("https://chatgpt.com/") && url.len() <= 2_048)
        .map(ToOwned::to_owned)
}

fn bounded_text(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| text.chars().take(MAX_TEXT_CHARS).collect())
}

fn bad_image(detail: String) -> Problem {
    Problem::new(StatusCode::BAD_REQUEST, "Invalid image", detail)
}

fn job_problem(error: JobError) -> Problem {
    match error {
        JobError::NotFound => Problem::new(
            StatusCode::NOT_FOUND,
            "Not found",
            "image job was not found or has expired",
        ),
        JobError::Forbidden => Problem::new(StatusCode::FORBIDDEN, "Forbidden", "image job access denied"),
        JobError::AlreadyClaimed => Problem::new(
            StatusCode::CONFLICT,
            "Already claimed",
            "another ChatCMD window is already dispatching this image job",
        ),
        JobError::AlreadyFinished => Problem::new(
            StatusCode::CONFLICT,
            "Already finished",
            "image job already received a result",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_image_payloads_and_sniffs_real_type() {
        let html = base64::engine::general_purpose::STANDARD.encode(b"<html>login</html>");
        assert!(decode_images(vec![ImageResultItem { mime_type: Some("image/png".into()), data_base64: html, source_url: None }]).is_err());

        let png = base64::engine::general_purpose::STANDARD.encode([0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2]);
        let decoded = decode_images(vec![ImageResultItem {
            mime_type: Some("image/webp".into()),
            data_base64: png,
            source_url: Some("javascript:alert(1)".into()),
        }])
        .unwrap();
        assert_eq!(decoded[0].mime_type, "image/png");
        assert_eq!(decoded[0].source_url, None);
    }

    #[test]
    fn only_chatgpt_conversation_urls_are_kept() {
        assert_eq!(chatgpt_url(Some("https://chatgpt.com/c/1")), Some("https://chatgpt.com/c/1".into()));
        assert_eq!(chatgpt_url(Some("https://evil.example/c/1")), None);
    }
}
