//! FABE Anons v4 - Tauri back end.

mod db;
mod ducking;
mod migrate;
mod paths;
mod player;

use db::{BreakEntry, CallEntry, Category, Person, Store};
use player::{Item, Kind, Player};
use serde::Serialize;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{Emitter, Manager, State};

/// Name shown in history for announcements made on this computer.
const LOCAL_SOURCE: &str = "Bu bilgisayar";
/// Names are always read twice.
const NAME_REPS: u32 = 2;

pub struct App {
    store: Store,
    player: Player,
}

type AppState<'a> = State<'a, Arc<App>>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    volume: f32,
    tts_gain: f32,
    tts_rate: f32,
    chime: bool,
}

impl Settings {
    fn load(s: &Store) -> Self {
        Self {
            volume: s.setting_f32("volume", 0.8),
            tts_gain: s.setting_f32("tts_gain", 4.0),
            tts_rate: s.setting_f32("tts_rate", 1.0),
            chime: s.setting("chime").ok().flatten().map(|v| v != "0" && v != "false").unwrap_or(true),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    categories: Vec<Category>,
    people: Vec<Person>,
    history: Vec<CallEntry>,
    breaks: Vec<BreakEntry>,
    settings: Settings,
}

fn err(e: impl std::fmt::Display) -> String {
    log::error!("{e}");
    e.to_string()
}

#[tauri::command]
fn snapshot(app: AppState) -> Result<Snapshot, String> {
    let s = &app.store;
    Ok(Snapshot {
        categories: s.categories().map_err(err)?,
        people: s.people().map_err(err)?,
        history: s.history(30).map_err(err)?,
        breaks: s.breaks().map_err(err)?,
        settings: Settings::load(s),
    })
}

#[tauri::command]
fn call_person(id: i64, app: AppState, handle: tauri::AppHandle) -> Result<(), String> {
    let p = app.store.person(id).map_err(err)?.ok_or("Kişi bulunamadı")?;
    let file = p.sound_file.as_deref().ok_or_else(|| format!("{} için ses kaydı yok", p.name))?;
    app.player.enqueue(Item {
        id: app.player.next_id(),
        title: p.name.clone(),
        kind: Kind::Person,
        source: LOCAL_SOURCE.into(),
        file: paths::sounds_dir().join(file),
        reps: NAME_REPS,
        gain: 1.0,
        temp: false,
    });
    app.store.log_call(Some(p.id), "person", &p.name, LOCAL_SOURCE).map_err(err)?;
    let _ = handle.emit("data-changed", ());
    Ok(())
}

#[tauri::command]
fn speak(text: String) -> Result<(), String> {
    let _ = text;
    Err("Canlı anons motoru bir sonraki sürümde bağlanacak".into())
}

#[tauri::command]
fn add_category(name: String, app: AppState, handle: tauri::AppHandle) -> Result<Category, String> {
    let c = app.store.add_category(&name).map_err(err)?;
    let _ = handle.emit("data-changed", ());
    Ok(c)
}

#[tauri::command]
fn set_person_category(id: i64, category: String, app: AppState, handle: tauri::AppHandle) -> Result<(), String> {
    app.store.set_person_category(id, &category).map_err(err)?;
    let _ = handle.emit("data-changed", ());
    Ok(())
}

/// "Eski yedekten geri yükle": imports a ZIP made by the old app (v1-v3).
#[tauri::command]
fn import_old_backup(path: String, app: AppState, handle: tauri::AppHandle) -> Result<migrate::Report, String> {
    let rep = migrate::import_zip(std::path::Path::new(&path), &app.store, &paths::sounds_dir()).map_err(err)?;
    log::info!("Yedekten geri yüklendi ({path}): {} kişi, {} ses, {} mola", rep.people, rep.sounds, rep.breaks);
    let _ = handle.emit("data-changed", ());
    Ok(rep)
}

#[tauri::command]
fn stop_all(app: AppState) {
    app.player.stop();
}

#[tauri::command]
fn set_setting(key: String, value: String, app: AppState) -> Result<(), String> {
    match key.as_str() {
        "volume" => {
            let v: f32 = value.parse().map_err(err)?;
            app.player.set_volume(v);
        }
        "chime" => app.player.set_chime(value != "0" && value != "false"),
        "tts_gain" | "tts_rate" => {
            value.parse::<f32>().map_err(err)?;
        }
        _ => return Err(format!("Bilinmeyen ayar: {key}")),
    }
    app.store.set_setting(&key, &value).map_err(err)
}

/// Plays scheduled breaks at their HH:MM, once per minute.
fn start_break_scheduler(app: Arc<App>, handle: tauri::AppHandle) {
    std::thread::Builder::new()
        .name("fabe-breaks".into())
        .spawn(move || {
            let mut last = String::new();
            loop {
                let now = chrono::Local::now().format("%H:%M").to_string();
                if now != last {
                    last = now.clone();
                    for b in app.store.breaks().unwrap_or_default().into_iter().filter(|b| b.time == now) {
                        let title = if b.label.is_empty() { "Mola".to_string() } else { b.label.clone() };
                        app.player.enqueue(Item {
                            id: app.player.next_id(),
                            title: title.clone(),
                            kind: Kind::Break,
                            source: "Zamanlı anons".into(),
                            file: paths::sounds_dir().join(&b.sound_file),
                            reps: 1,
                            gain: 1.0,
                            temp: false,
                        });
                        let _ = app.store.log_call(None, "break", &title, "Zamanlı anons");
                        let _ = handle.emit("data-changed", ());
                    }
                }
                std::thread::sleep(Duration::from_secs(5));
            }
        })
        .expect("break thread");
}

/// Writes warnings and errors to `hata.log` in the data folder.
struct FileLog(Mutex<Option<std::fs::File>>);

impl log::Log for FileLog {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Info
    }
    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) || !r.target().starts_with("fabe") {
            return;
        }
        let line = format!("{} [{}] {}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), r.level(), r.args());
        eprint!("{line}");
        if let Some(f) = self.0.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            let _ = f.write_all(line.as_bytes());
        }
    }
    fn flush(&self) {}
}

fn init_log() {
    let file = std::fs::OpenOptions::new().create(true).append(true).open(paths::log_path()).ok();
    let logger: &'static FileLog = Box::leak(Box::new(FileLog(Mutex::new(file))));
    if log::set_logger(logger).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}

fn open_store() -> anyhow::Result<Store> {
    paths::ensure()?;
    let store = Store::open(&paths::db_path())?;
    let legacy = paths::legacy_dir();
    if store.is_empty()? && legacy.is_dir() {
        match migrate::import(&legacy, &store, &paths::sounds_dir()) {
            Ok(r) => log::info!(
                "Eski veriler aktarıldı: {} kişi, {} ses, {} mola; sesi olmayan: {:?}",
                r.people,
                r.sounds,
                r.breaks,
                r.missing_sounds
            ),
            Err(e) => log::error!("Eski veriler aktarılamadı: {e:#}"),
        }
    }
    Ok(store)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = paths::ensure();
    init_log();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|tauri_app| {
            let store = open_store()?;
            let settings = Settings::load(&store);
            let handle = tauri_app.handle().clone();
            let emit_handle = handle.clone();
            let player = Player::start(
                move |state| {
                    let _ = emit_handle.emit("player", state);
                },
                settings.volume,
                settings.chime,
            );
            let app = Arc::new(App { store, player });
            start_break_scheduler(app.clone(), handle);
            tauri_app.manage(app);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![snapshot, add_category, set_person_category, call_person, speak, import_old_backup, stop_all, set_setting])
        .run(tauri::generate_context!())
        .expect("FABE Anons başlatılamadı");
}
