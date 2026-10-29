//! Where the daemon listens and where its token lives. The CLI reads the same values.

use std::fs;
use std::path::PathBuf;

pub const DEFAULT_PORT: u16 = 7878;

pub fn port() -> u16 {
    std::env::var("NOTCHD_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_PORT)
}

fn token_path() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("notchd").join("token")
}

/// The bearer token, created on first use.
pub fn token() -> anyhow::Result<String> {
    let path = token_path();
    if let Ok(existing) = fs::read_to_string(&path) {
        let existing = existing.trim().to_string();
        if !existing.is_empty() {
            return Ok(existing);
        }
    }
    let mut bytes = [0u8; 24];
    getrandom::fill(&mut bytes).map_err(|err| anyhow::anyhow!("no random bytes: {err}"))?;
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&path, &token)?;
    Ok(token)
}
