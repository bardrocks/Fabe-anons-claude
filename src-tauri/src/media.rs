//! Converting any audio file (WhatsApp .opus, .webm, .amr, .wma, .m4a...) to a normalized MP3
//! with the FFmpeg that ships with the app. FFmpeg looks at the file's content, not its name,
//! so a voice note saved with the wrong extension still works.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

static FFMPEG: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Called once at start with the app's resource folder.
pub fn init(resource_dir: Option<PathBuf>) {
    let found = resource_dir
        .into_iter()
        .flat_map(|d| [d.join("resources").join(exe()), d.join(exe())])
        .find(|p| p.is_file())
        .or_else(|| which(exe()));
    match &found {
        Some(p) => log::info!("FFmpeg: {}", p.display()),
        None => log::warn!("FFmpeg bulunamadı; yalnızca MP3/WAV/OGG/FLAC/M4A eklenebilir"),
    }
    let _ = FFMPEG.set(found);
}

fn exe() -> &'static str {
    if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" }
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|d| d.join(name)).find(|p| p.is_file()))
}

pub fn ffmpeg() -> Option<&'static Path> {
    FFMPEG.get_or_init(|| which(exe())).as_deref()
}

fn command(ffmpeg: &Path) -> Command {
    #[allow(unused_mut)] // mutated on Windows only
    let mut c = Command::new(ffmpeg);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

/// Converts `src` to an MP3 at `dest`, evening out the loudness so every
/// announcement plays at about the same volume (EBU R128, -16 LUFS).
pub fn to_normalized_mp3(src: &Path, dest: &Path) -> Result<()> {
    let Some(ff) = ffmpeg() else { bail!("Ses dönüştürücü (FFmpeg) bulunamadı") };
    let out = command(ff)
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(src)
        .args(["-vn", "-af", "loudnorm=I=-16:TP=-1.5:LRA=11", "-ar", "44100", "-ac", "2", "-c:a", "libmp3lame", "-b:a", "192k"])
        .arg(dest)
        .output()
        .context("FFmpeg çalıştırılamadı")?;
    if !out.status.success() || !dest.is_file() {
        let _ = std::fs::remove_file(dest);
        let msg = String::from_utf8_lossy(&out.stderr);
        bail!("Bu dosya ses olarak okunamadı ({})", msg.lines().last().unwrap_or("bilinmeyen hata").trim());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_when_ffmpeg_is_installed() {
        let Some(ff) = ffmpeg() else { return }; // FFmpeg isn't on every dev machine
        let dir = std::env::temp_dir().join(format!("fabe-media-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A 1 s tone saved as Opus in an .ogg with a misleading ".mp3" name, like WhatsApp files.
        let src = dir.join("whatsapp.mp3");
        let ok = command(ff)
            .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=1", "-c:a", "libopus", "-f", "ogg"])
            .arg(&src)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            return; // this FFmpeg build has no Opus encoder; nothing to test
        }
        let dest = dir.join("out.mp3");
        to_normalized_mp3(&src, &dest).unwrap();
        let clip = crate::player::Clip::load(&dest).unwrap();
        assert!(clip.duration().as_secs_f32() > 0.8);
        std::fs::remove_dir_all(&dir).ok();
    }
}
