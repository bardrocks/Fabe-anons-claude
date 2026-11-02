//! FABE Anons v4 - Tauri back end.

mod backup;
mod db;
mod ducking;
mod migrate;
mod paths;
mod player;
mod sounds;
mod update;

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
/// Same port as the old app, so old clients can still find the server.
const DEFAULT_PORT: u16 = 55555;

fn default_pc_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Bu bilgisayar".into())
}

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
    /// "yerel" | "sunucu" | "istemci"
    net_mode: String,
    net_server_ip: String,
    net_port: u16,
    pc_name: String,
    version: String,
}

impl Settings {
    fn load(s: &Store) -> Self {
        Self {
            volume: s.setting_f32("volume", 0.8),
            tts_gain: s.setting_f32("tts_gain", 4.0),
            tts_rate: s.setting_f32("tts_rate", 1.0),
            chime: s.setting("chime").ok().flatten().map(|v| v != "0" && v != "false").unwrap_or(true),
            net_mode: s.setting("net_mode").ok().flatten().unwrap_or_else(|| "yerel".into()),
            net_server_ip: s.setting("net_server_ip").ok().flatten().unwrap_or_default(),
            net_port: s.setting("net_port").ok().flatten().and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_PORT),
            pc_name: s.setting("pc_name").ok().flatten().unwrap_or_else(default_pc_name),
            version: env!("CARGO_PKG_VERSION").into(),
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

fn changed(handle: &tauri::AppHandle) {
    let _ = handle.emit("data-changed", ());
}

fn opt(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// Copies a sound the user picked into the app's sesler folder. Returns the stored file name.
#[tauri::command]
fn import_sound(path: String, name_hint: String) -> Result<String, String> {
    sounds::import(std::path::Path::new(&path), &paths::sounds_dir(), &name_hint).map_err(err)
}

/// Plays a sound once to try it ("Dinle"), without logging it.
#[tauri::command]
fn preview_sound(file: String, app: AppState) -> Result<(), String> {
    let path = paths::sounds_dir().join(&file);
    if !path.is_file() {
        return Err("Ses dosyası bulunamadı".into());
    }
    app.player.enqueue(Item {
        id: app.player.next_id(),
        title: "Deneme".into(),
        kind: Kind::Break,
        source: "Dinleme".into(),
        file: path,
        reps: 1,
        gain: 1.0,
        temp: false,
    });
    Ok(())
}

/// Deletes a sound file once nothing uses it any more.
fn drop_sound_if_unused(store: &Store, file: Option<String>) {
    if let Some(f) = file {
        if !store.sound_in_use(&f).unwrap_or(true) {
            let _ = std::fs::remove_file(paths::sounds_dir().join(f));
        }
    }
}

#[tauri::command]
fn create_person(
    name: String,
    category: String,
    color: Option<String>,
    sound_file: String,
    app: AppState,
    handle: tauri::AppHandle,
) -> Result<i64, String> {
    let id = app.store.create_person(&name, &category, opt(color).as_deref(), &sound_file).map_err(err)?;
    changed(&handle);
    Ok(id)
}

#[tauri::command]
fn update_person(
    id: i64,
    name: String,
    category: String,
    color: Option<String>,
    sound_file: Option<String>,
    app: AppState,
    handle: tauri::AppHandle,
) -> Result<(), String> {
    let old = app.store.person(id).map_err(err)?.and_then(|p| p.sound_file);
    let new_sound = opt(sound_file);
    app.store.update_person(id, &name, &category, opt(color).as_deref(), new_sound.as_deref()).map_err(err)?;
    if new_sound.is_some() && new_sound != old {
        drop_sound_if_unused(&app.store, old);
    }
    changed(&handle);
    Ok(())
}

#[tauri::command]
fn delete_person(id: i64, app: AppState, handle: tauri::AppHandle) -> Result<(), String> {
    let sound = app.store.delete_person(id).map_err(err)?;
    drop_sound_if_unused(&app.store, sound);
    changed(&handle);
    Ok(())
}

#[tauri::command]
fn rename_category(id: i64, name: String, app: AppState, handle: tauri::AppHandle) -> Result<(), String> {
    app.store.rename_category(id, &name).map_err(err)?;
    changed(&handle);
    Ok(())
}

/// Returns a short message saying where the category's people went.
#[tauri::command]
fn delete_category(id: i64, app: AppState, handle: tauri::AppHandle) -> Result<String, String> {
    let (moved, target) = app.store.delete_category(id).map_err(err)?;
    changed(&handle);
    Ok(if moved > 0 { format!("{moved} kişi \"{target}\" kategorisine taşındı") } else { String::new() })
}

#[tauri::command]
fn move_category(id: i64, step: i64, app: AppState, handle: tauri::AppHandle) -> Result<(), String> {
    app.store.move_category(id, step).map_err(err)?;
    changed(&handle);
    Ok(())
}

fn valid_time(t: &str) -> Result<String, String> {
    chrono::NaiveTime::parse_from_str(t.trim(), "%H:%M")
        .map(|v| v.format("%H:%M").to_string())
        .map_err(|_| "Saat SS:DD biçiminde olmalı, ör. 12:30".to_string())
}

#[tauri::command]
fn create_break(time: String, label: String, sound_file: String, app: AppState, handle: tauri::AppHandle) -> Result<(), String> {
    app.store.add_break(&valid_time(&time)?, &sound_file, label.trim()).map_err(err)?;
    changed(&handle);
    Ok(())
}

#[tauri::command]
fn update_break(
    id: i64,
    time: String,
    label: String,
    sound_file: Option<String>,
    app: AppState,
    handle: tauri::AppHandle,
) -> Result<(), String> {
    let old = app.store.breaks().map_err(err)?.into_iter().find(|b| b.id == id).map(|b| b.sound_file);
    let new_sound = opt(sound_file);
    app.store.update_break(id, &valid_time(&time)?, new_sound.as_deref(), label.trim()).map_err(err)?;
    if new_sound.is_some() && new_sound != old {
        drop_sound_if_unused(&app.store, old);
    }
    changed(&handle);
    Ok(())
}

#[tauri::command]
fn delete_break(id: i64, app: AppState, handle: tauri::AppHandle) -> Result<(), String> {
    let sound = app.store.delete_break(id).map_err(err)?;
    drop_sound_if_unused(&app.store, sound);
    changed(&handle);
    Ok(())
}

/// "Tüm verileri yedekle"
#[tauri::command]
async fn create_backup(path: String, app: tauri::State<'_, Arc<App>>) -> Result<(), String> {
    let app = app.inner().clone();
    tauri::async_runtime::spawn_blocking(move || backup::create(&app.store, &paths::sounds_dir(), std::path::Path::new(&path)))
        .await
        .map_err(err)?
        .map_err(err)
}

/// "Yedekten geri yükle": accepts backups of this app and of the old app.
#[tauri::command]
async fn restore_backup(
    path: String,
    app: tauri::State<'_, Arc<App>>,
    handle: tauri::AppHandle,
) -> Result<backup::RestoreReport, String> {
    let app = app.inner().clone();
    let rep = tauri::async_runtime::spawn_blocking(move || {
        backup::restore(std::path::Path::new(&path), &app.store, &paths::sounds_dir())
    })
    .await
    .map_err(err)?
    .map_err(err)?;
    log::info!("Yedekten geri yüklendi ({}): {} kişi, {} ses, {} mola", rep.kind, rep.people, rep.sounds, rep.breaks);
    changed(&handle);
    Ok(rep)
}

#[tauri::command]
fn list_backups() -> Result<Vec<backup::BackupFile>, String> {
    backup::list().map_err(err)
}

#[tauri::command]
fn open_backups_folder() -> Result<String, String> {
    let dir = paths::backups_dir();
    std::fs::create_dir_all(&dir).map_err(err)?;
    Ok(dir.to_string_lossy().into())
}

/// "Bağlantıyı test et": can this PC reach the server?
#[tauri::command]
async fn test_connection(ip: String, port: u16) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        use std::net::ToSocketAddrs;
        let ip = ip.trim().to_string();
        if ip.is_empty() {
            return Err("Önce sunucunun IP adresini yazın".to_string());
        }
        let addr = (ip.as_str(), port)
            .to_socket_addrs()
            .map_err(|_| format!("\"{ip}\" geçerli bir adres değil"))?
            .next()
            .ok_or("Adres çözülemedi")?;
        let start = std::time::Instant::now();
        std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(3))
            .map(|_| format!("Bağlantı başarılı ({} ms)", start.elapsed().as_millis()))
            .map_err(|_| format!("{ip}:{port} adresine ulaşılamadı. Sunucu açık mı, aynı ağda mısınız, güvenlik duvarı izin veriyor mu?"))
    })
    .await
    .map_err(err)?
}

/// Local IP addresses of this PC (shown on the network page so clients know what to type).
#[tauri::command]
fn local_ips() -> Vec<String> {
    // Connecting a UDP socket sends nothing; it only asks the OS which interface it would use.
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("8.8.8.8:80").map(|_| s))
        .and_then(|s| s.local_addr())
        .map(|a| vec![a.ip().to_string()])
        .unwrap_or_default()
}

#[tauri::command]
async fn check_update() -> Result<Option<update::Release>, String> {
    tauri::async_runtime::spawn_blocking(update::check).await.map_err(err)?.map_err(err)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateProgress {
    step: String,
    done: u64,
    total: u64,
}

/// Full backup, download, verify, start the installer, quit.
#[tauri::command]
async fn install_update(
    release: update::Release,
    app: tauri::State<'_, Arc<App>>,
    handle: tauri::AppHandle,
) -> Result<(), String> {
    let app = app.inner().clone();
    let h = handle.clone();
    let installer = tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<std::path::PathBuf> {
        let say = |step: &str, done: u64, total: u64| {
            let _ = h.emit("update-progress", UpdateProgress { step: step.into(), done, total });
        };
        say("Tüm veriler yedekleniyor…", 0, 0);
        let b = backup::auto(&app.store, &paths::sounds_dir(), "guncelleme-oncesi")?;
        log::info!("Güncelleme öncesi yedek: {}", b.display());
        let path = update::download(&release, |d, t| say("Yeni sürüm indiriliyor…", d, t))?;
        say("Kurulum başlatılıyor…", 0, 0);
        update::launch_installer(&path)?;
        Ok(path)
    })
    .await
    .map_err(err)?
    .map_err(|e| err(format!("{e:#}")))?;
    log::info!("Güncelleme kuruluyor: {}", installer.display());
    handle.exit(0);
    Ok(())
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
        "net_mode" => {
            if !["yerel", "sunucu", "istemci"].contains(&value.as_str()) {
                return Err("Geçersiz çalışma modu".into());
            }
        }
        "net_server_ip" | "pc_name" => {}
        "net_port" => {
            value.parse::<u16>().map_err(|_| "Port 1-65535 arasında bir sayı olmalı".to_string())?;
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
        .invoke_handler(tauri::generate_handler![
            snapshot,
            add_category,
            rename_category,
            delete_category,
            move_category,
            set_person_category,
            create_person,
            update_person,
            delete_person,
            import_sound,
            preview_sound,
            create_break,
            update_break,
            delete_break,
            call_person,
            speak,
            import_old_backup,
            create_backup,
            restore_backup,
            list_backups,
            open_backups_folder,
            test_connection,
            local_ips,
            check_update,
            install_update,
            stop_all,
            set_setting
        ])
        .run(tauri::generate_context!())
        .expect("FABE Anons başlatılamadı");
}
