//! Streaming MD5 of a demo file.
//!
//! Separate from `api.rs` because we also hash locally during filesystem
//! watcher debounce (before calling lookup-by-hash) - keeping it pure
//! makes it trivial to unit-test.

use anyhow::{Context, Result};
use md5::{Digest, Md5};
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

const CHUNK: usize = 64 * 1024;

/// Stream-hash a file, checking `cancel` between 64 KB chunks. Returns
/// `Ok(None)` if cancellation was requested mid-stream so the caller can
/// distinguish "user paused" from "IO error". CPU usage stops within one
/// chunk (~milliseconds even on a slow disk).
pub fn md5_hex_cancellable(path: &Path, cancel: &AtomicBool) -> Result<Option<String>> {
    let mut file = std::fs::File::open(path).with_context(|| format!("open {:?}", path))?;
    let mut hasher = Md5::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        if cancel.load(Ordering::Acquire) {
            return Ok(None);
        }
        let n = file.read(&mut buf).context("read chunk")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(Some(hex::encode(hasher.finalize())))
}
