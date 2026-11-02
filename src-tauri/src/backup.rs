//! Full backups ("Tüm verileri yedekle") and restoring them.
//!
//! A backup is a ZIP with `fabe.db` (people, categories, colors, history, breaks, settings),
//! the `sesler` folder and a small `fabe-yedek.json` describing it. Restoring also accepts
//! backups made by the old app (v1-v3).

use crate::db::Store;
use crate::{migrate, paths};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const MANIFEST: &str = "fabe-yedek.json";
/// How many automatic backups to keep.
const KEEP_AUTO: usize = 15;

#[derive(Serialize, Deserialize)]
struct Manifest {
    app: String,
    version: String,
    created: String,
    computer: String,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreReport {
    /// "v4" for backups of this app, "eski" for the old app's backups.
    pub kind: String,
    pub people: usize,
    pub sounds: usize,
    pub breaks: usize,
    pub missing_sounds: Vec<String>,
    /// Where the data was saved just before restoring, in case the user wants it back.
    pub safety_backup: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupFile {
    pub path: String,
    pub name: String,
    pub created: String,
    pub size: u64,
}

fn computer_name() -> String {
    std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_default()
}

fn temp_dir(tag: &str) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!(
        "fabe-{tag}-{}-{}",
        std::process::id(),
        chrono::Local::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Writes a full backup to `dest`.
pub fn create(store: &Store, sounds: &Path, dest: &Path) -> Result<()> {
    let tmp = temp_dir("yedek")?;
    let result = (|| {
        let db = tmp.join("fabe.db");
        store.backup_to(&db)?;
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Write to a side file first so a half-written ZIP never looks like a good backup.
        let partial = dest.with_extension("zip.part");
        let mut z = zip::ZipWriter::new(std::fs::File::create(&partial)?);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let manifest = Manifest {
            app: "FABE Anons".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            created: chrono::Local::now().to_rfc3339(),
            computer: computer_name(),
        };
        z.start_file(MANIFEST, opts)?;
        z.write_all(&serde_json::to_vec_pretty(&manifest)?)?;
        z.start_file("fabe.db", opts)?;
        std::io::copy(&mut std::fs::File::open(&db)?, &mut z)?;
        if let Ok(entries) = std::fs::read_dir(sounds) {
            for e in entries.flatten().filter(|e| e.path().is_file()) {
                // Audio is already compressed; storing it is faster and just as small.
                let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
                z.start_file(format!("sesler/{}", e.file_name().to_string_lossy()), stored)?;
                std::io::copy(&mut std::fs::File::open(e.path())?, &mut z)?;
            }
        }
        z.finish()?;
        std::fs::rename(&partial, dest).with_context(|| format!("{} yazılamadı", dest.display()))?;
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

/// Makes a backup in the app's own `yedekler` folder (before restoring or updating)
/// and keeps only the newest few.
pub fn auto(store: &Store, sounds: &Path, reason: &str) -> Result<PathBuf> {
    let dir = paths::backups_dir();
    let name = format!(
        "FABE_Anons_Yedek_{}_{}_{reason}.zip",
        env!("CARGO_PKG_VERSION"),
        chrono::Local::now().format("%Y%m%d_%H%M%S")
    );
    let dest = dir.join(name);
    create(store, sounds, &dest)?;
    let mut all = list()?;
    all.sort_by(|a, b| b.created.cmp(&a.created));
    for old in all.into_iter().skip(KEEP_AUTO) {
        let _ = std::fs::remove_file(old.path);
    }
    Ok(dest)
}

/// Automatic backups, newest first.
pub fn list() -> Result<Vec<BackupFile>> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(paths::backups_dir()) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "zip") {
                let meta = e.metadata()?;
                let created: chrono::DateTime<chrono::Local> = meta.modified()?.into();
                out.push(BackupFile {
                    path: p.to_string_lossy().into(),
                    name: e.file_name().to_string_lossy().into(),
                    created: created.format("%Y-%m-%d %H:%M").to_string(),
                    size: meta.len(),
                });
            }
        }
    }
    out.sort_by(|a, b| b.created.cmp(&a.created).then(b.name.cmp(&a.name)));
    Ok(out)
}

/// Restores a backup ZIP made by this app or by the old app.
/// The current data is backed up first, so nothing is ever lost by restoring.
pub fn restore(zip_path: &Path, store: &Store, sounds: &Path) -> Result<RestoreReport> {
    let file = std::fs::File::open(zip_path).with_context(|| format!("{} açılamadı", zip_path.display()))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| anyhow::anyhow!("Yedek dosyası okunamadı: {e}"))?;
    let is_v4 = zip.by_name(MANIFEST).is_ok() && zip.by_name("fabe.db").is_ok();

    let safety = auto(store, sounds, "geri-yukleme-oncesi").context("Geri yüklemeden önce güvenlik yedeği alınamadı")?;
    let safety_backup = safety.to_string_lossy().to_string();

    if !is_v4 {
        let r = migrate::import_zip(zip_path, store, sounds)?;
        return Ok(RestoreReport {
            kind: "eski".into(),
            people: r.people,
            sounds: r.sounds,
            breaks: r.breaks,
            missing_sounds: r.missing_sounds,
            safety_backup,
        });
    }

    let tmp = temp_dir("geri")?;
    let result = (|| {
        let db = tmp.join("fabe.db");
        std::io::copy(&mut zip.by_name("fabe.db")?, &mut std::fs::File::create(&db)?)?;
        let mut copied = 0;
        std::fs::create_dir_all(sounds)?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            let Some(rel) = entry.enclosed_name() else { continue };
            let Ok(name) = rel.strip_prefix("sesler") else { continue };
            if entry.is_dir() || name.as_os_str().is_empty() {
                continue;
            }
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            std::fs::write(sounds.join(name), buf)?;
            copied += 1;
        }
        store.restore_from(&db)?;
        Ok(RestoreReport {
            kind: "v4".into(),
            people: store.people()?.len(),
            sounds: copied,
            breaks: store.breaks()?.len(),
            missing_sounds: Vec::new(),
            safety_backup: safety_backup.clone(),
        })
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_then_restore_brings_everything_back() {
        let tmp = temp_dir("test").unwrap();
        let sounds = tmp.join("sesler");
        std::fs::create_dir_all(&sounds).unwrap();
        std::fs::write(sounds.join("ayse.mp3"), b"ses").unwrap();

        let store = Store::open(&tmp.join("fabe.db")).unwrap();
        store.add_category("Depo").unwrap();
        store.add_person("Ayşe", "Depo", Some("#FF0000"), Some("ayse.mp3"), 4).unwrap();
        store.add_break("12:30", "ayse.mp3", "Öğle").unwrap();
        store.set_setting("volume", "0.5").unwrap();

        let zip = tmp.join("yedek.zip");
        create(&store, &sounds, &zip).unwrap();

        // Wipe everything, then restore.
        let id = store.people().unwrap()[0].id;
        store.delete_person(id).unwrap();
        store.set_setting("volume", "1").unwrap();
        std::fs::remove_file(sounds.join("ayse.mp3")).unwrap();

        let rep = restore(&zip, &store, &sounds).unwrap();
        assert_eq!(rep.kind, "v4");
        assert_eq!((rep.people, rep.sounds, rep.breaks), (1, 1, 1));
        let p = &store.people().unwrap()[0];
        assert_eq!((p.name.as_str(), p.category.as_str(), p.color.as_deref(), p.count), ("Ayşe", "Depo", Some("#FF0000"), 4));
        assert_eq!(store.setting("volume").unwrap().as_deref(), Some("0.5"));
        assert!(sounds.join("ayse.mp3").is_file());
        assert!(Path::new(&rep.safety_backup).is_file(), "current data is saved before restoring");
        std::fs::remove_dir_all(&tmp).ok();
    }
}
