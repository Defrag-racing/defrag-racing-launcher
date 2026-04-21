//! Streaming MD5 of a demo file.
//!
//! Separate from `api.rs` because we also hash locally during filesystem
//! watcher debounce (before calling lookup-by-hash) — keeping it pure
//! makes it trivial to unit-test.

use anyhow::{Context, Result};
use md5::{Digest, Md5};
use std::io::Read;
use std::path::Path;

const CHUNK: usize = 64 * 1024;

pub fn md5_hex(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).with_context(|| format!("open {:?}", path))?;
    let mut hasher = Md5::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = file.read(&mut buf).context("read chunk")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}
