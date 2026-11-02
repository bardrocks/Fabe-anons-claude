//! Where FABE Anons keeps its files.
//!
//! v4 keeps everything under `%APPDATA%\FABE Anons` so updates never touch user data.
//! The old (v1-v3) app used `%APPDATA%\Fabe_Anons_Veri`; we read from it once to migrate.

use std::path::PathBuf;

fn appdata() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn data_dir() -> PathBuf {
    appdata().join("FABE Anons")
}

pub fn sounds_dir() -> PathBuf {
    data_dir().join("sesler")
}

pub fn db_path() -> PathBuf {
    data_dir().join("fabe.db")
}

pub fn log_path() -> PathBuf {
    data_dir().join("hata.log")
}

/// Data folder of the old Python app (v1-v3).
pub fn legacy_dir() -> PathBuf {
    appdata().join("Fabe_Anons_Veri")
}

pub fn ensure() -> std::io::Result<()> {
    std::fs::create_dir_all(sounds_dir())
}

/// Automatic backups (before restoring or updating).
pub fn backups_dir() -> PathBuf {
    data_dir().join("yedekler")
}
