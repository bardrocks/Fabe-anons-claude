//! Adding sound files to the app's own `sesler` folder.

use crate::player::Clip;
use anyhow::{Context, Result};
use std::path::Path;

/// Keeps letters (Turkish ones too), digits and dashes, so the name is safe on every PC.
fn slug(s: &str) -> String {
    let mut out = String::new();
    for ch in s.trim().chars() {
        if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "ses".into() } else { out.chars().take(40).collect() }
}

/// Copies `src` into `sounds` under a new unique name and returns that name.
/// The file is decoded first, so a file that can't be played is refused right away
/// instead of failing later during an announcement.
pub fn import(src: &Path, sounds: &Path, name_hint: &str) -> Result<String> {
    Clip::load(src).map_err(|e| {
        anyhow::anyhow!("Bu ses dosyası okunamadı. Desteklenen biçimler: MP3, WAV, OGG, FLAC, M4A, AAC. ({e})")
    })?;
    let ext = src
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| "mp3".into());
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let base = format!("{}-{stamp}", slug(name_hint));
    let mut name = format!("{base}.{ext}");
    let mut n = 2;
    while sounds.join(&name).exists() {
        name = format!("{base}-{n}.{ext}");
        n += 1;
    }
    std::fs::create_dir_all(sounds)?;
    std::fs::copy(src, sounds.join(&name)).with_context(|| format!("{} kopyalanamadı", src.display()))?;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_keeps_turkish_letters() {
        assert_eq!(slug("Münevver Şahin"), "münevver-şahin");
        assert_eq!(slug("  ** "), "ses");
        assert_eq!(slug("Mola 12:30"), "mola-12-30");
    }

    #[test]
    fn refuses_files_that_are_not_audio() {
        let dir = std::env::temp_dir().join(format!("fabe-snd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bad = dir.join("not-audio.mp3");
        std::fs::write(&bad, b"hello").unwrap();
        assert!(import(&bad, &dir.join("out"), "x").is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
