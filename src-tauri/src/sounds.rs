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

fn unique_name(sounds: &Path, name_hint: &str, ext: &str) -> String {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let base = format!("{}-{stamp}", slug(name_hint));
    let mut name = format!("{base}.{ext}");
    let mut n = 2;
    while sounds.join(&name).exists() {
        name = format!("{base}-{n}.{ext}");
        n += 1;
    }
    name
}

/// Adds a sound the user picked to the app's `sesler` folder and returns its new name.
///
/// With FFmpeg (shipped with the Windows app) every format is accepted — WhatsApp .opus,
/// .webm, .amr, .wma, .m4a... — and converted to MP3 with even loudness. Without it, the
/// file is copied as-is if it can be played. Either way the result is test-decoded, so a
/// broken file is refused now rather than failing during an announcement.
pub fn import(src: &Path, sounds: &Path, name_hint: &str) -> Result<String> {
    std::fs::create_dir_all(sounds)?;
    if crate::media::ffmpeg().is_some() {
        let name = unique_name(sounds, name_hint, "mp3");
        let dest = sounds.join(&name);
        crate::media::to_normalized_mp3(src, &dest)?;
        if let Err(e) = Clip::load(&dest) {
            let _ = std::fs::remove_file(&dest);
            anyhow::bail!("Dönüştürülen ses çalınamadı: {e}");
        }
        return Ok(name);
    }
    Clip::load(src).map_err(|e| {
        anyhow::anyhow!("Bu ses dosyası okunamadı. Desteklenen biçimler: MP3, WAV, OGG, FLAC, M4A, AAC. ({e})")
    })?;
    let ext = src
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| "mp3".into());
    let name = unique_name(sounds, name_hint, &ext);
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
