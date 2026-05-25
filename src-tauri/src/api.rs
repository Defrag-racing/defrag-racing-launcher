//! HTTP client for the defrag.racing launcher API.
//!
//! Two endpoints only:
//!   - POST /api/launcher/lookup-by-hash - pre-upload dedupe check
//!   - POST /api/launcher/upload-demo    - actual multipart upload
//!
//! The token is injected at call time (keyring → memory → header) so a
//! token rotation takes effect on the next upload without restart.

use anyhow::{anyhow, Context, Result};
use reqwest::multipart::{Form, Part};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Number of automatic retries on HTTP 429 before giving up and surfacing
/// the error to the caller. Three retries with the server's own
/// Retry-After honored each time is enough to absorb the bursts a rescan
/// of a few thousand demos can produce against the per-token throttle.
const MAX_RETRIES_ON_429: u32 = 3;

/// Fallback wait between retries when the server didn't send a
/// `Retry-After` header. Scales with the retry attempt (1s, 2s, 4s).
const RETRY_BASE_SECS: u64 = 1;

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
    #[error("authentication failed - token invalid or revoked")]
    Unauthorized,
    #[error("account is restricted from uploading demos")]
    Forbidden,
    #[error("rate limit exceeded - backing off")]
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

/// Callback invoked when the client starts/stops waiting on a 429
/// backoff. Lets the worker surface a "resuming in Xs" countdown in
/// the UI - we don't pull AppHandle into this module to keep api.rs
/// Tauri-free.
///
/// First call: `Some(epoch_ms_when_resuming)` when wait begins.
/// Second call: `None` when wait ends (success or final failure).
pub type RateLimitObserver = Arc<dyn Fn(Option<u64>) + Send + Sync>;

pub struct Client {
    http: reqwest::Client,
    base_url: String,
    token: String,
    rate_limit_observer: Option<RateLimitObserver>,
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
        Ok(Self { http, base_url, token, rate_limit_observer: None })
    }

    /// Install a callback that gets pinged whenever the client starts
    /// or stops waiting on a 429 backoff. Used by the worker to push
    /// a live countdown into UploadState so the Dashboard can show
    /// "Rate limited, resuming in Xs".
    pub fn set_rate_limit_observer(&mut self, observer: RateLimitObserver) {
        self.rate_limit_observer = Some(observer);
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }

    /// Ask the server if a demo with this MD5 already exists. Called before
    /// every upload so we skip files the user already has on defrag.racing
    /// (e.g. after a reinstall of Defrag that dropped old demos into a
    /// freshly-watched folder).
    ///
    /// Wrapped in retry-on-429 so a rescan that briefly outruns the
    /// per-token rate limit gets paced down by the server's own
    /// Retry-After header instead of marking the file as Error and
    /// forcing the user to redrive manually.
    pub async fn lookup_by_hash(&self, md5_hex: &str) -> ApiResult<LookupResponse> {
        let resp = self
            .with_429_retry(|| async {
                self.http
                    .post(self.url("/api/launcher/lookup-by-hash"))
                    .bearer_auth(&self.token)
                    .header("Accept", "application/json")
                    .json(&serde_json::json!({ "hash": md5_hex }))
                    .send()
                    .await
            })
            .await?;
        self.check_status(&resp).await?;
        Ok(resp.json::<LookupResponse>().await?)
    }

    /// Walks the closure's response: if it's HTTP 429, honors the
    /// server's `Retry-After` header (or falls back to exponential
    /// backoff) and retries up to MAX_RETRIES_ON_429 times. Any other
    /// status passes through to the caller for normal handling.
    ///
    /// While sleeping on a 429, pokes the rate_limit_observer with the
    /// Unix-epoch milliseconds when the wait ends so the UI can render
    /// a live countdown. Pokes with None on the way out (success or
    /// final failure) so the countdown banner clears.
    async fn with_429_retry<F, Fut>(&self, mut send: F) -> ApiResult<reqwest::Response>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<reqwest::Response, reqwest::Error>>,
    {
        let mut attempt: u32 = 0;
        loop {
            let resp = send().await?;
            if resp.status() != reqwest::StatusCode::TOO_MANY_REQUESTS
                || attempt >= MAX_RETRIES_ON_429
            {
                if attempt > 0 {
                    if let Some(obs) = &self.rate_limit_observer {
                        obs(None);
                    }
                }
                return Ok(resp);
            }
            let wait = retry_after_from(&resp).unwrap_or_else(|| {
                Duration::from_secs(RETRY_BASE_SECS << attempt)
            });
            let resume_at_ms = now_ms().saturating_add(wait.as_millis() as u64);
            if let Some(obs) = &self.rate_limit_observer {
                obs(Some(resume_at_ms));
            }
            log::warn!(
                "rate limited (429), attempt {} of {}, waiting {:?}",
                attempt + 1,
                MAX_RETRIES_ON_429,
                wait
            );
            tokio::time::sleep(wait).await;
            attempt += 1;
        }
    }

    /// Upload a single demo file. `md5_hex` is the precomputed hash - we
    /// send it so the server can skip hashing again. If the server already
    /// has this hash (race with another device) we get 409 which surfaces
    /// as `ApiError::Duplicate`.
    ///
    /// Same retry-on-429 wrapper as lookup_by_hash. The multipart Form
    /// can't be cloned (consuming the byte buffer), so the entire form
    /// construction is re-done inside the retry closure - cheap for the
    /// typical small demo, and uploads aren't tight enough to spin much.
    pub async fn upload_demo(&self, path: &Path, md5_hex: &str) -> ApiResult<UploadResponse> {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow!("invalid filename"))?
            .to_string();

        let bytes = tokio::fs::read(path).await.context("read demo file")?;

        let resp = self
            .with_429_retry(|| async {
                // Form is consumed by .send() so we rebuild it per attempt.
                // bytes.clone() is bounded by the 512MB upload cap and only
                // matters on the rare 429-retry path.
                let part = match Part::bytes(bytes.clone())
                    .file_name(file_name.clone())
                    .mime_str("application/octet-stream")
                {
                    Ok(p) => p,
                    // mime_str only fails on a malformed mime literal,
                    // which is a constant here - treat as unreachable but
                    // don't panic from the retry closure.
                    Err(_) => return self.http.get("about:blank").send().await,
                };
                let form = Form::new()
                    .text("hash", md5_hex.to_string())
                    .part("demo", part);
                self.http
                    .post(self.url("/api/launcher/upload-demo"))
                    .bearer_auth(&self.token)
                    .header("Accept", "application/json")
                    .multipart(form)
                    .send()
                    .await
            })
            .await?;

        // 409 duplicate - parse the body so we can report the existing id back.
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

    /// Pull the server-browser feed from /api/launcher/servers. Returns
    /// the raw JSON value the Laravel endpoint produced - we don't define
    /// a Rust struct mirror because the shape comes straight from the
    /// existing web /api/servers/live (same ServerListService) and will
    /// grow new columns over time. The Vue layer types it loosely and
    /// renders directly, so a backend addition doesn't need a launcher
    /// release.
    pub async fn fetch_servers(&self) -> ApiResult<serde_json::Value> {
        let resp = self
            .http
            .get(self.url("/api/launcher/servers"))
            .bearer_auth(&self.token)
            .header("Accept", "application/json")
            .send()
            .await?;
        self.check_status(&resp).await?;
        let body = resp.json::<serde_json::Value>().await?;
        Ok(body)
    }

    async fn check_status(&self, resp: &reqwest::Response) -> ApiResult<()> {
        // `Response::json()` consumes the body - we only read it here when
        // the status is already known to be an error, so the success path
        // stays efficient.
        match resp.status().as_u16() {
            200..=299 => Ok(()),
            401 => Err(ApiError::Unauthorized),
            403 => Err(ApiError::Forbidden),
            // Reaching this branch means with_429_retry exhausted its
            // retries and still got 429 - propagate so the worker
            // marks the file Error and the user can redrive when the
            // limit window passes.
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

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Parse the `Retry-After` HTTP header into a Duration. The spec allows
/// either an integer seconds value or an HTTP-date; we handle only the
/// integer case (Laravel's default throttle uses it). Returns None if
/// the header is missing or malformed, in which case the caller falls
/// back to exponential backoff.
fn retry_after_from(resp: &reqwest::Response) -> Option<Duration> {
    resp.headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}
