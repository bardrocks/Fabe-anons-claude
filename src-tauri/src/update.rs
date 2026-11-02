//! "Yeni sürüm var" check and one-click update.
//!
//! Each release publishes `latest.json` next to the installer:
//! `{"version":"4.1.0","notes":"...","url":"https://.../FABE.Anons_4.1.0_x64-setup.exe","sha256":"..."}`.
//! Updating first makes a full backup, then downloads the installer, checks its SHA-256
//! and starts it; the installer replaces the app and opens it again. User data lives in
//! %APPDATA%\FABE Anons and is never touched by the installer.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

pub const LATEST_URL: &str = "https://github.com/bardrocks/Fabe-anons-claude/releases/latest/download/latest.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    pub url: String,
    pub sha256: String,
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(60))
        .user_agent(concat!("FABE-Anons/", env!("CARGO_PKG_VERSION")))
        .build()
}

fn parse_version(v: &str) -> Vec<u64> {
    v.trim_start_matches(['v', 'V']).split(['.', '-']).map_while(|p| p.parse().ok()).collect()
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    parse_version(candidate) > parse_version(current)
}

/// Returns the newer release, or None when this is already the latest version.
pub fn check() -> Result<Option<Release>> {
    let rel: Release = agent()
        .get(LATEST_URL)
        .call()
        .map_err(|e| anyhow::anyhow!("Güncelleme sunucusuna ulaşılamadı: {e}"))?
        .into_json()
        .context("Güncelleme bilgisi okunamadı")?;
    Ok(is_newer(&rel.version, env!("CARGO_PKG_VERSION")).then_some(rel))
}

/// Downloads the installer and verifies it. Returns the path of the verified file.
pub fn download(rel: &Release, mut progress: impl FnMut(u64, u64)) -> Result<PathBuf> {
    let resp = agent()
        .get(&rel.url)
        .timeout(Duration::from_secs(600))
        .call()
        .map_err(|e| anyhow::anyhow!("Yeni sürüm indirilemedi: {e}"))?;
    let total: u64 = resp.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let dest = std::env::temp_dir().join(format!("FABE-Anons-{}-kurulum.exe", rel.version));
    let mut reader = resp.into_reader();
    let mut file = std::fs::File::create(&dest)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        std::io::Write::write_all(&mut file, &buf[..n])?;
        done += n as u64;
        progress(done, total);
    }
    drop(file);
    let got = hex::encode(hasher.finalize());
    if !got.eq_ignore_ascii_case(rel.sha256.trim()) {
        let _ = std::fs::remove_file(&dest);
        anyhow::bail!("İndirilen dosya bozuk (doğrulama tutmadı). Güncelleme yapılmadı, lütfen tekrar deneyin.");
    }
    Ok(dest)
}

/// Starts the installer in "passive" mode (progress bar, no questions) and asks it to
/// reopen the app when finished. The caller must exit the app right after.
pub fn launch_installer(installer: &std::path::Path) -> Result<()> {
    std::process::Command::new(installer)
        .args(["/P", "/R"])
        .spawn()
        .context("Kurulum başlatılamadı")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_numerically() {
        assert!(is_newer("4.1.0", "4.0.9"));
        assert!(is_newer("v4.10.0", "4.9.3"));
        assert!(!is_newer("4.0.0", "4.0.0"));
        assert!(!is_newer("3.2.0", "4.0.0"));
    }
}
