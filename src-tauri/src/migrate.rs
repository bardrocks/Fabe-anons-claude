//! One-time import of the old Python app's data (`%APPDATA%\Fabe_Anons_Veri`).
//!
//! The old folder is only read, never changed, so the old app keeps working side by side.

use crate::db::Store;
use anyhow::Result;
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub people: usize,
    pub sounds: usize,
    pub breaks: usize,
    pub missing_sounds: Vec<String>,
}

const AUDIO_EXTS: &[&str] = &["mp3", "wav", "ogg", "opus", "m4a", "aac", "flac", "wma", "webm", "amr"];

fn read_json(path: &Path) -> Option<Value> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Finds the sound for an old `dosya_adi`, whatever its extension.
fn find_sound(legacy_sounds: &Path, stem: &str) -> Option<PathBuf> {
    AUDIO_EXTS
        .iter()
        .map(|e| legacy_sounds.join(format!("{stem}.{e}")))
        .find(|p| p.is_file())
}

fn copy_into(src: &Path, sounds: &Path) -> Option<String> {
    let name = src.file_name()?.to_string_lossy().to_string();
    let dst = sounds.join(&name);
    if !dst.exists() {
        std::fs::copy(src, &dst).ok()?;
    }
    Some(name)
}

pub fn import(legacy: &Path, store: &Store, sounds: &Path) -> Result<Report> {
    let mut rep = Report::default();
    let legacy_sounds = legacy.join("sesler");

    let stats = read_json(&legacy.join("istatistikler.json")).unwrap_or(Value::Null);
    let count_of = |name: &str| stats.get(name).and_then(Value::as_i64).unwrap_or(0);

    if let Some(Value::Object(people)) = read_json(&legacy.join("personel.json")) {
        for (name, v) in people {
            // Old versions stored a bare file stem; v2.1+ store an object.
            let (stem, cat, color) = match &v {
                Value::String(s) => (s.clone(), "Personeller".to_string(), None),
                Value::Object(o) => (
                    o.get("dosya_adi").and_then(Value::as_str).unwrap_or_default().to_string(),
                    o.get("kategori").and_then(Value::as_str).unwrap_or("Personeller").to_string(),
                    o.get("renk").and_then(Value::as_str).map(str::to_string),
                ),
                _ => continue,
            };
            let category = if cat.trim().is_empty() { "Personeller" } else { cat.trim() };
            let sound = find_sound(&legacy_sounds, &stem).and_then(|p| copy_into(&p, sounds));
            if sound.is_some() {
                rep.sounds += 1;
            } else {
                rep.missing_sounds.push(name.clone());
            }
            // Old color #2B2B2B was the default grey: treat it as "no color".
            let color = color.filter(|c| !c.eq_ignore_ascii_case("#2B2B2B"));
            store.add_person(&name, category, color.as_deref(), sound.as_deref(), count_of(&name))?;
            rep.people += 1;
        }
    }

    if let Some(Value::Array(items)) = read_json(&legacy.join("zamanli_anonslar.json")) {
        for it in items {
            let Some(time) = it.get("saat").and_then(Value::as_str) else { continue };
            let src = it
                .get("dosya_yolu")
                .and_then(Value::as_str)
                .map(PathBuf::from)
                .or_else(|| it.get("dosya").and_then(Value::as_str).map(|d| legacy_sounds.join(d)));
            // The old app saved absolute paths; if the user moved PCs, look in its sesler folder too.
            let src = src.filter(|p| p.is_file()).or_else(|| {
                let p = it.get("dosya_yolu").and_then(Value::as_str)?;
                let alt = legacy_sounds.join(p.rsplit(['/', '\\']).next()?);
                alt.is_file().then_some(alt)
            });
            if let Some(file) = src.as_deref().and_then(|p| copy_into(p, sounds)) {
                let label = it.get("aciklama").and_then(Value::as_str).unwrap_or("Mola");
                store.add_break(time, &file, label)?;
                rep.breaks += 1;
            }
        }
    }

    if let Some(v) = read_json(&legacy.join("ayarlar.json")) {
        for (old, new) in [("ses_seviyesi", "volume"), ("tts_carpan", "tts_gain"), ("tts_hiz", "tts_rate")] {
            if let Some(x) = v.get(old).and_then(Value::as_f64) {
                store.set_setting(new, &x.to_string())?;
            }
        }
    }

    store.set_setting("migrated_from_v3", &chrono::Local::now().to_rfc3339())?;
    Ok(rep)
}

/// Restores a backup ZIP made by the old app ("TÜM VERİLERİ YEDEKLE" or the automatic
/// backup before an update). The ZIP holds the old data folder: personel.json, sesler/, ...
/// It is unpacked to a temporary folder and imported like the live old folder.
pub fn import_zip(zip_path: &Path, store: &Store, sounds: &Path) -> Result<Report> {
    let tmp = std::env::temp_dir().join(format!("fabe-yedek-{}-{}", std::process::id(), chrono::Local::now().timestamp_millis()));
    std::fs::create_dir_all(&tmp)?;
    let result = (|| {
        let file = std::fs::File::open(zip_path)?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| anyhow::anyhow!("Yedek dosyası okunamadı: {e}"))?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            // enclosed_name rejects "../" tricks; nested old backups are skipped.
            let Some(rel) = entry.enclosed_name() else { continue };
            if entry.is_dir() || rel.components().any(|c| c.as_os_str() == "otomatik_yedekler") {
                continue;
            }
            let out = tmp.join(rel);
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::io::copy(&mut entry, &mut std::fs::File::create(&out)?)?;
        }
        let root = find_legacy_root(&tmp).ok_or_else(|| anyhow::anyhow!("Bu ZIP bir FABE Anons yedeği değil (personel.json yok)"))?;
        import(&root, store, sounds)
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

/// The folder holding personel.json: the ZIP root, or one folder down if the user re-zipped it.
fn find_legacy_root(dir: &Path) -> Option<PathBuf> {
    if dir.join("personel.json").is_file() {
        return Some(dir.to_path_buf());
    }
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| p.join("personel.json").is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_old_backup_zip_twice_without_duplicates() {
        use std::io::Write;
        let tmp = std::env::temp_dir().join(format!("fabe-zip-{}", std::process::id()));
        let sounds = tmp.join("new");
        std::fs::create_dir_all(&sounds).unwrap();
        let zip_path = tmp.join("Fabe_Anons_Yedek.zip");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
            let o = zip::write::SimpleFileOptions::default();
            z.start_file("personel.json", o).unwrap();
            z.write_all(r##"{"Eren":{"dosya_adi":"eren","kategori":"Personeller","renk":"#2E7D32"}}"##.as_bytes()).unwrap();
            z.start_file("sesler/eren.m4a", o).unwrap();
            z.write_all(b"x").unwrap();
            z.start_file("sesler/mola1.mp3", o).unwrap();
            z.write_all(b"x").unwrap();
            z.start_file("zamanli_anonslar.json", o).unwrap();
            z.write_all(br#"[{"saat":"12:30","dosya_yolu":"D:\\eski\\mola1.mp3"}]"#).unwrap();
            z.start_file("otomatik_yedekler/eski.zip", o).unwrap();
            z.write_all(b"x").unwrap();
            z.finish().unwrap();
        }
        let store = Store::memory().unwrap();
        for _ in 0..2 {
            let rep = import_zip(&zip_path, &store, &sounds).unwrap();
            assert_eq!((rep.people, rep.sounds, rep.breaks), (1, 1, 1));
        }
        let p = &store.people().unwrap()[0];
        assert_eq!(p.color.as_deref(), Some("#2E7D32"));
        assert_eq!(p.sound_file.as_deref(), Some("eren.m4a"));
        assert_eq!(store.breaks().unwrap().len(), 1);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn imports_old_layout_including_odd_file_names() {
        let tmp = std::env::temp_dir().join(format!("fabe-mig-{}", std::process::id()));
        let legacy = tmp.join("old");
        let sounds = tmp.join("new");
        std::fs::create_dir_all(legacy.join("sesler")).unwrap();
        std::fs::create_dir_all(&sounds).unwrap();
        std::fs::write(legacy.join("sesler/mnevver.mp3"), b"x").unwrap();
        std::fs::write(legacy.join("sesler/ayse.ogg"), b"x").unwrap();
        std::fs::write(legacy.join("sesler/mola_1912_mola1.mp3"), b"x").unwrap();
        std::fs::write(
            legacy.join("personel.json"),
            r##"{"Münevver":{"dosya_adi":"mnevver","kategori":"Personeller"},
                 "Ayşe":"ayse",
                 "ALUCH":{"dosya_adi":"aluch","kategori":"Firmalar","renk":"#1B5E20"}}"##,
        )
        .unwrap();
        std::fs::write(legacy.join("istatistikler.json"), r#"{"Ayşe":10,"artemis2":1}"#).unwrap();
        std::fs::write(
            legacy.join("zamanli_anonslar.json"),
            r#"[{"saat":"19:12","dosya_yolu":"C:\\Users\\x\\AppData\\Roaming\\Fabe_Anons_Veri\\sesler\\mola_1912_mola1.mp3"}]"#,
        )
        .unwrap();

        let store = Store::memory().unwrap();
        let rep = import(&legacy, &store, &sounds).unwrap();
        assert_eq!(rep.people, 3);
        assert_eq!(rep.sounds, 2);
        assert_eq!(rep.missing_sounds, vec!["ALUCH".to_string()]);
        assert_eq!(rep.breaks, 1, "absolute Windows path falls back to the sesler folder");

        let people = store.people().unwrap();
        let ayse = people.iter().find(|p| p.name == "Ayşe").unwrap();
        assert_eq!(ayse.count, 10);
        assert_eq!(ayse.sound_file.as_deref(), Some("ayse.ogg"));
        let aluch = people.iter().find(|p| p.name == "ALUCH").unwrap();
        assert_eq!(aluch.category, "Firmalar");
        assert!(sounds.join("mnevver.mp3").is_file());
        std::fs::remove_dir_all(&tmp).ok();
    }
}
