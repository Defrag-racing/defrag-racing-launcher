//! HTTP client for the defrag.racing launcher API.
//!
//! Two endpoints only:
//!   - POST /api/launcher/lookup-by-hash — pre-upload dedupe check
//!   - POST /api/launcher/upload-demo    — actual multipart upload
//!
//! The token is injected at call time (keyring → memory → header) so a
//! token rotation takes effect on the next upload without restart.

use anyhow::{anyhow, Context, Result};
use reqwest::multipart::{Form, Part};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LookupResponse {
    pub exists: bool,
    pub demo_id: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UploadResponse {
    pub demo_id: u64,
    pub status: String,
}

/// Errors we care about differentiating in the UI. Anything unexpected
/// collapses into `Other`.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("authentication failed — token invalid or revoked")]
    Unauthorized,
    #[error("account is restricted from uploading demos")]
    Forbidden,
    #[error("rate limit exceeded — backing off")]
    RateLimited,
    #[error("duplicate (demo_id = {demo_id})")]
    Duplicate { demo_id: u64 },
    #[error("server error ({status}): {body}")]
    ServerError { status: u16, body: String },
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

pub type ApiResult<T> = Result<T, ApiError>;

pub struct Client {
    http: reqwest::Client,
    base_url: String,
    token: String,
}

impl Client {
    pub fn new(base_url: String, token: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(format!("defrag-racing-launcher/{}", env!("CARGO_PKG_VERSION")))
            // Upload of a 50 MB demo over a slow link can easily exceed the
            // default 30s. Give it a generous ceiling but keep the
            // connection timeout tight so a dead server fails fast.
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(300))
            .build()
            .context("build reqwest client")?;
        Ok(Self { http, base_url, token })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }

    /// Ask the server if a demo with this MD5 already exists. Called before
    /// every upload so we skip files the user already has on defrag.racing
    /// (e.g. after a reinstall of Defrag that dropped old demos into a
    /// freshly-watched folder).
    pub async fn lookup_by_hash(&self, md5_hex: &str) -> ApiResult<LookupResponse> {
        let resp = self
            .http
            .post(self.url("/api/launcher/lookup-by-hash"))
            .bearer_auth(&self.token)
            .header("Accept", "application/json")
            .json(&serde_json::json!({ "hash": md5_hex }))
            .send()
            .await?;
        self.check_status(&resp).await?;
        let body = resp.json::<LookupResponse>().await?;
        Ok(body)
    }

    /// Upload a single demo file. `md5_hex` is the precomputed hash — we
    /// send it so the server can skip hashing again. If the server already
    /// has this hash (race with another device) we get 409 which surfaces
    /// as `ApiError::Duplicate`.
    pub async fn upload_demo(&self, path: &Path, md5_hex: &str) -> ApiResult<UploadResponse> {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow!("invalid filename"))?
            .to_string();

        let bytes = tokio::fs::read(path).await.context("read demo file")?;
        let part = Part::bytes(bytes)
            .file_name(file_name.clone())
            .mime_str("application/octet-stream")
            .map_err(|e| anyhow!(e))?;

        let form = Form::new()
            .text("hash", md5_hex.to_string())
            .part("demo", part);

        let resp = self
            .http
            .post(self.url("/api/launcher/upload-demo"))
            .bearer_auth(&self.token)
            .header("Accept", "application/json")
            .multipart(form)
            .send()
            .await?;

        // 409 duplicate — parse the body so we can report the existing id back.
        if resp.status() == reqwest::StatusCode::CONFLICT {
            #[derive(Deserialize)]
            struct Dup {
                demo_id: u64,
            }
            let dup: Dup = resp.json().await?;
            return Err(ApiError::Duplicate { demo_id: dup.demo_id });
        }

        self.check_status(&resp).await?;
        let body = resp.json::<UploadResponse>().await?;
        Ok(body)
    }

    async fn check_status(&self, resp: &reqwest::Response) -> ApiResult<()> {
        // `Response::json()` consumes the body — we only read it here when
        // the status is already known to be an error, so the success path
        // stays efficient.
        match resp.status().as_u16() {
            200..=299 => Ok(()),
            401 => Err(ApiError::Unauthorized),
            403 => Err(ApiError::Forbidden),
            429 => Err(ApiError::RateLimited),
            status => {
                // Clone the status first so we can still read the body. reqwest's
                // Response doesn't allow reading an error body without consuming
                // the response, so we surface just the status code + a brief hint.
                Err(ApiError::ServerError {
                    status,
                    body: format!("http {}", status),
                })
            }
        }
    }
}
