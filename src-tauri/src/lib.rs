//! FABE Anons v4 - Tauri back end.

mod backup;
mod db;
mod ducking;
mod media;
mod migrate;
mod network;
mod paths;
mod player;
mod sounds;
mod tts;
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
/// Names and live announcements are always read twice.
const NAME_REPS: u32 = 2;
/// Same port as the old app, so old clients can still find the server.
const DEFAULT_PORT: u16 = 55555;

fn default_pc_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Bu bilgisayar".into())
}

pub struct App {
    store: Store,
    player: Player,
    /// Running only in "sunucu" mode.
    server: Mutex<Option<network::Server>>,
    tts: tts::Tts,
}

struct NetConfig {
    mode: String,
    ip: String,
    port: u16,
    me: String,
}

impl App {
    fn net(&self) -> NetConfig {
        let s = Settings::load(&self.store);
        NetConfig { mode: s.net_mode, ip: s.net_server_ip, port: s.net_port, me: s.pc_name }
    }
}

/// Starts or stops the server to match the saved network settings.
fn apply_network(app: &Arc<App>, handle: &tauri::AppHandle) {
    let cfg = app.net();
    let mut slot = app.server.lock().unwrap_or_else(|e| e.into_inner());
    *slot = None; // stops a running server
    if cfg.mode != "sunucu" {
        return;
    }
    let (a, h) = (app.clone(), handle.clone());
    let handler: network::Handler = Arc::new(move |inc: network::Incoming| {
        let kind = match inc.kind.as_str() {
            "break" => Kind::Break,
            "tts" => Kind::Tts,
            _ => Kind::Person,
        };
        let (file, temp) = match &inc.speak {
            // Old clients send only the text; this PC reads it.
            Some((text, _, rate)) => {
                let s = Settings::load(&a.store);
                (a.tts.speak(text, *rate, tts::Preference::parse(&s.tts_engine))?.file, true)
            }
            None => (inc.file, kind == Kind::Tts),
        };
        a.player.enqueue(Item {
            id: a.player.next_id(),
            title: inc.title.clone(),
            kind,
            source: inc.from.clone(),
            file,
            reps: if kind == Kind::Break { inc.reps.max(1) } else { NAME_REPS },
            gain: if kind == Kind::Tts { inc.gain } else { 1.0 },
            temp,
        });
        // If the server has someone with the same name, count it in their statistics.
        let person = match kind {
            Kind::Person => a.store.people().ok().and_then(|ps| ps.into_iter().find(|p| p.name == inc.title)).map(|p| p.id),
            _ => None,
        };
        let kind_name = match kind {
            Kind::Break => "break",
            Kind::Tts => "tts",
            Kind::Person => "person",
        };
        a.store.log_call(person, kind_name, &inc.title, &inc.from)?;
        let _ = h.emit("data-changed", ());
        Ok(())
    });
    *slot = Some(network::Server::start(cfg.port, cfg.me, paths::network_cache_dir(), paths::sounds_dir(), handler));
}

/// Sends an announcement to the server. If the server can't be reached at the saved address
/// (its IP or port changed, or none was saved yet), finds it on the network, saves the new
/// address and tries once more, so nobody has to type IPs or ports.
fn send_to_server(app: &App, title: &str, kind: &str, gain: f32, file: &std::path::Path) -> anyhow::Result<()> {
    let cfg = app.net();
    let first = if cfg.ip.trim().is_empty() {
        Err(anyhow::anyhow!("Sunucu bulunamadı. Sunucu bilgisayarda FABE Anons açık ve 'Sunucu' modunda mı?"))
    } else {
        network::send_call(&cfg.ip, cfg.port, &cfg.me, title, kind, NAME_REPS, gain, file)
    };
    let Err(e) = first else { return Ok(()) };
    let found = network::discover(Duration::from_millis(1500));
    let pick = found.iter().find(|f| f.ip == cfg.ip.trim()).or(if found.len() == 1 { found.first() } else { None });
    let Some(f) = pick else {
        if found.len() > 1 {
            anyhow::bail!("Ağda birden fazla FABE sunucusu var. Ayarlar > Ağ ayarları > Sunucuyu bul ile birini seçin.");
        }
        return Err(e);
    };
    if f.ip == cfg.ip.trim() && f.port == cfg.port {
        return Err(e); // it is there but refused: a real error
    }
    log::info!("Sunucu yeni adreste bulundu: {}:{} ({})", f.ip, f.port, f.name);
    app.store.set_setting("net_server_ip", &f.ip)?;
    app.store.set_setting("net_port", &f.port.to_string())?;
    network::send_call(&f.ip, f.port, &cfg.me, title, kind, NAME_REPS, gain, file)
}

type AppState<'a> = State<'a, Arc<App>>;

/// Allowed range for the person button size slider, in percent.
const PAD_SIZE_MIN: u16 = 70;
const PAD_SIZE_MAX: u16 = 200;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    volume: f32,
    tts_gain: f32,
    tts_rate: f32,
    /// "otomatik" | "ema" | "piper"
    tts_engine: String,
    chime: bool,
    /// "yerel" | "sunucu" | "istemci"
    net_mode: String,
    net_server_ip: String,
    net_port: u16,
    pc_name: String,
    /// Size of the person buttons on the main screen, in percent (70-200).
    pad_size: u16,
    version: String,
}

impl Settings {
    fn load(s: &Store) -> Self {
        Self {
            volume: s.setting_f32("volume", 0.8),
            tts_gain: s.setting_f32("tts_gain", 4.0),
            tts_rate: s.setting_f32("tts_rate", 1.0),
            tts_engine: s.setting("tts_engine").ok().flatten().unwrap_or_else(|| "otomatik".into()),
            chime: s.setting("chime").ok().flatten().map(|v| v != "0" && v != "false").unwrap_or(true),
            net_mode: s.setting("net_mode").ok().flatten().unwrap_or_else(|| "yerel".into()),
            net_server_ip: s.setting("net_server_ip").ok().flatten().unwrap_or_default(),
            net_port: s.setting("net_port").ok().flatten().and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_PORT),
            pc_name: s.setting("pc_name").ok().flatten().unwrap_or_else(default_pc_name),
            pad_size: s
                .setting("pad_size")
                .ok()
                .flatten()
                .and_then(|v| v.parse::<u16>().ok())
                .map(|v| v.clamp(PAD_SIZE_MIN, PAD_SIZE_MAX))
                .unwrap_or(100),
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
async fn call_person(id: i64, app: tauri::State<'_, Arc<App>>, handle: tauri::AppHandle) -> Result<String, String> {
    let app = app.inner().clone();
    let p = app.store.person(id).map_err(err)?.ok_or("Kişi bulunamadı")?;
    let file = p.sound_file.as_deref().ok_or_else(|| format!("{} için ses kaydı yok", p.name))?;
    let path = paths::sounds_dir().join(file);
    let cfg = app.net();
    if cfg.mode == "istemci" {
        // This PC sends; the server plays.
        let (title, me, a) = (p.name.clone(), cfg.me.clone(), app.clone());
        tauri::async_runtime::spawn_blocking(move || send_to_server(&a, &title, "person", 1.0, &path))
        .await
        .map_err(err)?
        .map_err(|e| err(format!("{e:#}")))?;
        app.store.log_call(Some(p.id), "person", &p.name, &format!("{me} → sunucu")).map_err(err)?;
        let _ = handle.emit("data-changed", ());
        return Ok(format!("{} sunucuya gönderildi", p.name));
    }
    app.player.enqueue(Item {
        id: app.player.next_id(),
        title: p.name.clone(),
        kind: Kind::Person,
        source: LOCAL_SOURCE.into(),
        file: path,
        reps: NAME_REPS,
        gain: 1.0,
        temp: false,
    });
    app.store.log_call(Some(p.id), "person", &p.name, LOCAL_SOURCE).map_err(err)?;
    let _ = handle.emit("data-changed", ());
    Ok(String::new())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SpeakResult {
    engine: tts::Engine,
    /// Shown to the user: why the fallback was used, or that it went to the server.
    note: Option<String>,
}

/// "SESLENDİR": reads the text twice, with ema-lightning or (if that fails) Piper.
#[tauri::command]
async fn speak(text: String, app: tauri::State<'_, Arc<App>>, handle: tauri::AppHandle) -> Result<SpeakResult, String> {
    let app = app.inner().clone();
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("Okunacak metni yazın".into());
    }
    let s = Settings::load(&app.store);
    let pref = tts::Preference::parse(&s.tts_engine);
    let (a, t) = (app.clone(), text.clone());
    let spoken = tauri::async_runtime::spawn_blocking(move || a.tts.speak(&t, s.tts_rate, pref))
        .await
        .map_err(err)?
        .map_err(|e| err(format!("{e:#}")))?;
    let cfg = app.net();
    if cfg.mode == "istemci" {
        let (file, title, gain, me, a) = (spoken.file.clone(), text.clone(), s.tts_gain, cfg.me.clone(), app.clone());
        let sent = tauri::async_runtime::spawn_blocking(move || {
            let r = send_to_server(&a, &title, "tts", gain, &file);
            let _ = std::fs::remove_file(&file);
            r
        })
        .await
        .map_err(err)?;
        sent.map_err(|e| err(format!("{e:#}")))?;
        app.store.log_call(None, "tts", &text, &format!("{me} → sunucu")).map_err(err)?;
        changed(&handle);
        let note = match spoken.note {
            Some(n) => format!("Sunucuya gönderildi. {n}"),
            None => "Sunucuya gönderildi".into(),
        };
        return Ok(SpeakResult { engine: spoken.engine, note: Some(note) });
    }
    app.player.enqueue(Item {
        id: app.player.next_id(),
        title: text.clone(),
        kind: Kind::Tts,
        source: LOCAL_SOURCE.into(),
        file: spoken.file,
        reps: NAME_REPS,
        gain: s.tts_gain,
        temp: true,
    });
    app.store.log_call(None, "tts", &text, LOCAL_SOURCE).map_err(err)?;
    changed(&handle);
    Ok(SpeakResult { engine: spoken.engine, note: spoken.note })
}

/// "En çok çağrılanlar" for bugün / hafta / ay. ("tumu" uses the people's own counters,
/// which include counts carried over from the old app.)
#[tauri::command]
fn stats(period: String, app: AppState) -> Result<Vec<db::Ranked>, String> {
    let now = chrono::Local::now();
    let since = match period.as_str() {
        "bugun" => now.format("%Y-%m-%d 00:00:00").to_string(),
        "hafta" => (now - chrono::Duration::days(6)).format("%Y-%m-%d 00:00:00").to_string(),
        "ay" => (now - chrono::Duration::days(29)).format("%Y-%m-%d 00:00:00").to_string(),
        _ => return Err("Geçersiz dönem".into()),
    };
    app.store.top_called(&since, 5).map_err(err)
}

/// Drag and drop on the main screen: saves the new order of the buttons.
#[tauri::command]
fn reorder_people(ids: Vec<i64>, app: AppState, handle: tauri::AppHandle) -> Result<(), String> {
    app.store.reorder_people(&ids).map_err(err)?;
    changed(&handle);
    Ok(())
}

/// Lower-cases the Turkish way (I → ı, İ → i) so searches match what people type.
fn tr_lower(s: &str) -> String {
    s.replace('I', "ı").replace('İ', "i").to_lowercase()
}

/// "Geçmiş" page: the latest announcements, optionally only those matching `query`.
#[tauri::command]
fn search_history(query: String, app: AppState) -> Result<Vec<CallEntry>, String> {
    let q = tr_lower(query.trim());
    let rows = app.store.history(if q.is_empty() { 200 } else { 20_000 }).map_err(err)?;
    if q.is_empty() {
        return Ok(rows);
    }
    Ok(rows
        .into_iter()
        .filter(|h| tr_lower(&h.title).contains(&q) || tr_lower(&h.source).contains(&q) || h.at.contains(&q))
        .take(200)
        .collect())
}

/// Which live-announcement engines are ready ("Ayarlar > Canlı anons" and the mic panel).
#[tauri::command]
fn tts_status(app: AppState) -> tts::Status {
    app.tts.status()
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

/// "Bağlantıyı test et": is there a FABE Anons server at this address?
#[tauri::command]
async fn test_connection(ip: String, port: u16, app: tauri::State<'_, Arc<App>>) -> Result<String, String> {
    let me = app.net().me;
    tauri::async_runtime::spawn_blocking(move || network::ping(&ip, port, &me).map_err(|e| e.to_string()))
        .await
        .map_err(err)?
}

/// Server state for the network page: listening or not, and which computers have connected.
#[tauri::command]
fn net_status(app: AppState) -> Option<network::ServerStatus> {
    app.server.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|s| s.status())
}

/// Local IP addresses of this PC (shown on the network page).
#[tauri::command]
fn local_ips() -> Vec<String> {
    network::local_ip().map(|i| vec![i.to_string()]).unwrap_or_default()
}

/// "Sunucuyu bul": FABE servers on the local network.
#[tauri::command]
async fn find_servers() -> Result<Vec<network::Found>, String> {
    tauri::async_runtime::spawn_blocking(|| network::discover(Duration::from_millis(1500))).await.map_err(err)
}

/// Saves the server the user picked from "Sunucuyu bul" (IP and port, no typing).
#[tauri::command]
fn use_server(ip: String, port: u16, app: AppState) -> Result<(), String> {
    app.store.set_setting("net_server_ip", ip.trim()).map_err(err)?;
    app.store.set_setting("net_port", &port.to_string()).map_err(err)?;
    Ok(())
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
fn set_setting(key: String, value: String, app: AppState, handle: tauri::AppHandle) -> Result<(), String> {
    match key.as_str() {
        "volume" => {
            let v: f32 = value.parse().map_err(err)?;
            app.player.set_volume(v);
        }
        "chime" => app.player.set_chime(value != "0" && value != "false"),
        "tts_gain" | "tts_rate" => {
            value.parse::<f32>().map_err(err)?;
        }
        "tts_engine" => {
            if !["otomatik", "ema", "piper"].contains(&value.as_str()) {
                return Err("Geçersiz anons motoru".into());
            }
            if value != "piper" {
                app.tts.warm_up();
            }
        }
        "net_mode" => {
            if !["yerel", "sunucu", "istemci"].contains(&value.as_str()) {
                return Err("Geçersiz çalışma modu".into());
            }
        }
        "net_server_ip" | "pc_name" => {}
        "pad_size" => match value.parse::<u16>() {
            Ok(v) if (PAD_SIZE_MIN..=PAD_SIZE_MAX).contains(&v) => {}
            _ => return Err(format!("Buton boyutu %{PAD_SIZE_MIN}-%{PAD_SIZE_MAX} arasında olmalı")),
        },
        "net_port" => {
            value.parse::<u16>().map_err(|_| "Port 1-65535 arasında bir sayı olmalı".to_string())?;
        }
        _ => return Err(format!("Bilinmeyen ayar: {key}")),
    }
    app.store.set_setting(&key, &value).map_err(err)?;
    if matches!(key.as_str(), "net_mode" | "net_port" | "pc_name") {
        apply_network(app.inner(), &handle);
    }
    Ok(())
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
                    // Clients don't play breaks; the server PC (with the speakers) does.
                    if app.net().mode == "istemci" {
                        continue;
                    }
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
            let resource_dir = tauri_app.path().resource_dir().ok();
            media::init(resource_dir.clone());
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
            let tts = tts::Tts::new(resource_dir, &paths::data_dir());
            if settings.tts_engine != "piper" {
                tts.warm_up(); // ema-lightning loads in the background
            }
            let app = Arc::new(App { store, player, server: Mutex::new(None), tts });
            apply_network(&app, &handle);
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
            tts_status,
            stats,
            search_history,
            reorder_people,
            import_old_backup,
            create_backup,
            restore_backup,
            list_backups,
            open_backups_folder,
            test_connection,
            net_status,
            local_ips,
            find_servers,
            use_server,
            check_update,
            install_update,
            stop_all,
            set_setting
        ])
        .run(tauri::generate_context!())
        .expect("FABE Anons başlatılamadı");
}
