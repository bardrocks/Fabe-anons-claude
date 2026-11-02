//! Server / client networking.
//!
//! The server (the PC connected to the speakers) listens on a TCP port (55555 by default,
//! same as the old app). Each request is one short connection:
//!
//! 1. The client sends one line of JSON, e.g.
//!    `{"t":"call","from":"KASA-1","title":"Ayşe","hash":"<sha256>","ext":"mp3","reps":2}`
//! 2. If the server already has that sound (by hash) it plays it and answers `{"ok":true}`.
//!    Otherwise it answers `{"need":"file"}`; the client sends `{"t":"file","size":N}` and
//!    then N bytes on the same connection. The server checks the hash, keeps the file in its
//!    cache and plays it. So every PC can have its own staff list and recordings.
//!
//! Old clients (v1-v3) send `PING` or `PLAY_ANONS:name:file`; those still work.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Largest sound file accepted over the network.
const MAX_FILE: u64 = 50 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum Request {
    Ping { from: String },
    Call {
        from: String,
        title: String,
        hash: String,
        ext: String,
        reps: u32,
        kind: String,
        /// Live announcements carry the sender's "ses gücü".
        #[serde(default = "one")]
        gain: f32,
    },
    File { size: u64 },
}

fn one() -> f32 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Reply {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub need: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// What the server asks the app to do with an incoming announcement.
pub struct Incoming {
    pub from: String,
    pub title: String,
    pub kind: String,
    pub reps: u32,
    /// Local file to play (already verified).
    pub file: PathBuf,
    pub gain: f32,
    /// Old (v1-v3) clients only send a file name that must exist on the server.
    #[allow(dead_code)] // kept for logs and tests
    pub legacy: bool,
    /// Old clients send live announcements as text: the server reads it with its own engine.
    /// `file` is empty then. (text, gain, rate)
    pub speak: Option<(String, f32, f32)>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientSeen {
    pub name: String,
    pub ip: String,
    pub last_seen: String,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    pub listening: bool,
    pub port: u16,
    pub error: Option<String>,
    pub clients: Vec<ClientSeen>,
}

pub fn hash_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path).with_context(|| format!("{} açılamadı", path.display()))?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(hex::encode(h.finalize()))
}

fn safe_ext(ext: &str) -> String {
    let e: String = ext.chars().filter(|c| c.is_ascii_alphanumeric()).take(5).collect::<String>().to_lowercase();
    if e.is_empty() { "mp3".into() } else { e }
}

fn write_json(stream: &mut TcpStream, v: &impl Serialize) -> Result<()> {
    let mut line = serde_json::to_vec(v)?;
    line.push(b'\n');
    stream.write_all(&line)?;
    stream.flush()?;
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(reader: &mut impl BufRead) -> Result<T> {
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if line.trim().is_empty() {
        bail!("Bağlantı kapandı");
    }
    Ok(serde_json::from_str(line.trim())?)
}

// ---------------------------------------------------------------- server

pub type Handler = Arc<dyn Fn(Incoming) -> Result<()> + Send + Sync>;

pub struct Server {
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<ServerStatus>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// The server answers "who is the FABE server?" on these UDP ports (the first one it can
/// open); clients ask on all of them, so nobody ever has to type a port.
pub const DISCOVERY_PORTS: [u16; 3] = [55557, 45557, 35557];
const DISCOVERY_ASK: &[u8] = b"FABE_ANONS_SUNUCU_KIM?";
/// Ports tried for the server after the preferred one, before letting Windows pick any free one.
const FALLBACK_PORTS: std::ops::RangeInclusive<u16> = 55556..=55599;

/// Opens the first free port: the preferred one (55555, same as the old app, so old clients
/// keep working), then 55556-55599, then any free port. Right after a restart Windows can
/// still hold the preferred port for a moment, so it is retried briefly first.
fn bind_free(preferred: u16) -> std::io::Result<TcpListener> {
    for _ in 0..8 {
        match TcpListener::bind(("0.0.0.0", preferred)) {
            Ok(l) => return Ok(l),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => std::thread::sleep(Duration::from_millis(250)),
            Err(e) => return Err(e),
        }
    }
    for p in FALLBACK_PORTS.filter(|p| *p != preferred) {
        if let Ok(l) = TcpListener::bind(("0.0.0.0", p)) {
            return Ok(l);
        }
    }
    TcpListener::bind(("0.0.0.0", 0))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Found {
    pub name: String,
    #[serde(default)]
    pub ip: String,
    pub port: u16,
    #[serde(default)]
    pub version: String,
}

/// Answers discovery requests until `stop` is set.
fn respond(stop: Arc<AtomicBool>, name: String, port: u16) {
    let Some(sock) = DISCOVERY_PORTS.iter().find_map(|p| std::net::UdpSocket::bind(("0.0.0.0", *p)).ok()) else {
        log::warn!("Otomatik bulma için UDP portu açılamadı; istemciler IP adresi elle yazılarak bağlanabilir");
        return;
    };
    let _ = sock.set_read_timeout(Some(Duration::from_millis(300)));
    let reply = serde_json::to_vec(&Found { name, ip: String::new(), port, version: env!("CARGO_PKG_VERSION").into() })
        .unwrap_or_default();
    let mut buf = [0u8; 64];
    while !stop.load(Ordering::Relaxed) {
        if let Ok((n, from)) = sock.recv_from(&mut buf) {
            if &buf[..n] == DISCOVERY_ASK {
                let _ = sock.send_to(&reply, from);
            }
        }
    }
}

/// Finds FABE servers on the local network (asks by broadcast, waits `wait` for answers).
pub fn discover(wait: Duration) -> Vec<Found> {
    let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0") else { return vec![] };
    let _ = sock.set_broadcast(true);
    let mut targets: Vec<std::net::IpAddr> = vec![std::net::Ipv4Addr::BROADCAST.into(), std::net::Ipv4Addr::LOCALHOST.into()];
    // Also the subnet's own broadcast address (x.y.z.255): some routers drop 255.255.255.255.
    if let Some(std::net::IpAddr::V4(me)) = local_ip() {
        let o = me.octets();
        targets.push(std::net::Ipv4Addr::new(o[0], o[1], o[2], 255).into());
    }
    for ip in &targets {
        for p in DISCOVERY_PORTS {
            let _ = sock.send_to(DISCOVERY_ASK, (*ip, p));
        }
    }
    let end = std::time::Instant::now() + wait;
    let mut found: Vec<Found> = Vec::new();
    let mut buf = [0u8; 512];
    while let Some(left) = end.checked_duration_since(std::time::Instant::now()) {
        let _ = sock.set_read_timeout(Some(left.max(Duration::from_millis(1))));
        let Ok((n, from)) = sock.recv_from(&mut buf) else { break };
        if let Ok(mut f) = serde_json::from_slice::<Found>(&buf[..n]) {
            f.ip = if from.ip().is_loopback() { local_ip().map(|i| i.to_string()).unwrap_or_else(|| from.ip().to_string()) } else { from.ip().to_string() };
            if !found.iter().any(|x| x.ip == f.ip && x.port == f.port) {
                found.push(f);
            }
        }
    }
    found
}

/// This PC's address on the local network.
pub fn local_ip() -> Option<std::net::IpAddr> {
    // Connecting a UDP socket sends nothing; it only asks the OS which interface it would use.
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    s.local_addr().ok().map(|a| a.ip())
}

impl Server {
    /// Starts listening in the background on a free port (see `bind_free`) and answers
    /// discovery requests. `cache` holds sounds received from clients; `sounds` is the server's
    /// own sesler folder (for old clients that send a file name).
    pub fn start(preferred: u16, name: String, cache: PathBuf, sounds: PathBuf, handler: Handler) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(ServerStatus { port: preferred, ..Default::default() }));
        let (stop2, status2) = (stop.clone(), status.clone());
        let thread = std::thread::Builder::new()
            .name("fabe-server".into())
            .spawn(move || {
                let listener = match bind_free(preferred) {
                    Ok(l) => l,
                    Err(e) => {
                        let msg = format!("Sunucu başlatılamadı: {e}");
                        log::error!("{msg}");
                        status2.lock().unwrap().error = Some(msg);
                        return;
                    }
                };
                let port = listener.local_addr().map(|a| a.port()).unwrap_or(preferred);
                if port != preferred {
                    log::warn!("{preferred} portu dolu; sunucu {port} portunu kullanıyor (eski 3.x istemciler bağlanamaz)");
                }
                let _ = listener.set_nonblocking(true);
                {
                    let mut st = status2.lock().unwrap();
                    st.listening = true;
                    st.port = port;
                }
                log::info!("Sunucu {port} portunda dinliyor");
                let responder = {
                    let (stop, name) = (stop2.clone(), name.clone());
                    std::thread::spawn(move || respond(stop, name, port))
                };
                while !stop2.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, addr)) => {
                            let ctx = (name.clone(), cache.clone(), sounds.clone(), handler.clone(), status2.clone());
                            std::thread::spawn(move || {
                                let (name, cache, sounds, handler, status) = ctx;
                                if let Err(e) = serve(stream, addr, &name, &cache, &sounds, &handler, &status) {
                                    log::warn!("{addr} isteği başarısız: {e:#}");
                                }
                            });
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(150)),
                        Err(e) => {
                            log::warn!("Bağlantı kabul edilemedi: {e}");
                            std::thread::sleep(Duration::from_millis(500));
                        }
                    }
                }
                let _ = responder.join();
                status2.lock().unwrap().listening = false;
            })
            .expect("server thread");
        Self { stop, status, thread: Some(thread) }
    }

    pub fn status(&self) -> ServerStatus {
        self.status.lock().unwrap().clone()
    }
}

impl Drop for Server {
    /// Waits until the port is really closed, so a server started right after (new
    /// settings) can open the same port.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn seen(status: &Mutex<ServerStatus>, name: &str, addr: SocketAddr) {
    let mut st = status.lock().unwrap();
    let ip = addr.ip().to_string();
    let now = chrono::Local::now().format("%H:%M").to_string();
    st.clients.retain(|c| c.ip != ip);
    st.clients.insert(0, ClientSeen { name: name.to_string(), ip, last_seen: now });
    st.clients.truncate(20);
}

fn serve(
    stream: TcpStream,
    addr: SocketAddr,
    name: &str,
    cache: &Path,
    sounds: &Path,
    handler: &Handler,
    status: &Mutex<ServerStatus>,
) -> Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);

    // Old clients send plain text without a newline.
    let first = reader.fill_buf()?.first().copied();
    if first != Some(b'{') {
        let mut buf = vec![0u8; 4096];
        let n = reader.read(&mut buf)?;
        let msg = String::from_utf8_lossy(&buf[..n]).to_string();
        return serve_legacy(&msg, addr, &mut writer, sounds, handler, status);
    }

    let req: Request = read_json(&mut reader)?;
    match req {
        Request::Ping { from } => {
            seen(status, &from, addr);
            write_json(&mut writer, &Reply { ok: true, name: Some(name.into()), version: Some(env!("CARGO_PKG_VERSION").into()), ..Default::default() })
        }
        Request::Call { from, title, hash, ext, reps, kind, gain } => {
            seen(status, &from, addr);
            if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
                return write_json(&mut writer, &Reply { error: Some("Geçersiz istek".into()), ..Default::default() });
            }
            let file = cache.join(format!("{}.{}", hash.to_lowercase(), safe_ext(&ext)));
            if !file.is_file() {
                write_json(&mut writer, &Reply { need: Some("file".into()), ..Default::default() })?;
                let Request::File { size } = read_json(&mut reader)? else { bail!("Dosya bekleniyordu") };
                if size > MAX_FILE {
                    return write_json(&mut writer, &Reply { error: Some("Ses dosyası çok büyük".into()), ..Default::default() });
                }
                let mut data = vec![0u8; size as usize];
                reader.read_exact(&mut data)?;
                let got = hex::encode(Sha256::digest(&data));
                if !got.eq_ignore_ascii_case(&hash) {
                    return write_json(&mut writer, &Reply { error: Some("Ses dosyası bozuk geldi, tekrar deneyin".into()), ..Default::default() });
                }
                std::fs::create_dir_all(cache)?;
                let tmp = file.with_extension("part");
                std::fs::write(&tmp, &data)?;
                std::fs::rename(&tmp, &file)?;
            }
            match handler(Incoming { from, title, kind, reps: reps.clamp(1, 3), file, gain: gain.clamp(0.1, 10.0), legacy: false, speak: None }) {
                Ok(()) => write_json(&mut writer, &Reply { ok: true, ..Default::default() }),
                Err(e) => write_json(&mut writer, &Reply { error: Some(e.to_string()), ..Default::default() }),
            }
        }
        Request::File { .. } => write_json(&mut writer, &Reply { error: Some("Beklenmeyen istek".into()), ..Default::default() }),
    }
}

fn serve_legacy(
    msg: &str,
    addr: SocketAddr,
    writer: &mut TcpStream,
    sounds: &Path,
    handler: &Handler,
    status: &Mutex<ServerStatus>,
) -> Result<()> {
    let from = format!("Eski sürüm ({})", addr.ip());
    seen(status, &from, addr);
    if msg.trim() == "PING" {
        writer.write_all(b"PONG")?;
        return Ok(());
    }
    if let Some(rest) = msg.strip_prefix("PLAY_ANONS:") {
        let mut parts = rest.splitn(2, ':');
        let (Some(title), Some(stem)) = (parts.next(), parts.next()) else { bail!("Eksik eski istek") };
        let stem = stem.trim().trim_end_matches(".mp3");
        let file = ["mp3", "wav", "ogg", "m4a", "flac"].iter().map(|e| sounds.join(format!("{stem}.{e}"))).find(|p| p.is_file());
        match file {
            Some(file) => handler(Incoming {
                from,
                title: title.to_string(),
                kind: "person".into(),
                reps: 2,
                file,
                gain: 1.0,
                legacy: true,
                speak: None,
            })?,
            None => log::warn!("Eski istemciden gelen '{title}' için sunucuda ses yok ({stem})"),
        }
    } else if let Some(rest) = msg.strip_prefix("PLAY_TTS:") {
        // "PLAY_TTS:<ses gücü>,<hız>:<metin>"
        let (prefs, text) = rest.split_once(':').unwrap_or(("", rest));
        let mut nums = prefs.split(',').map(|v| v.trim().parse::<f32>().ok());
        let gain = nums.next().flatten().unwrap_or(4.0);
        let rate = nums.next().flatten().unwrap_or(1.0);
        let text = text.trim();
        if !text.is_empty() {
            handler(Incoming {
                from,
                title: text.to_string(),
                kind: "tts".into(),
                reps: 2,
                file: PathBuf::new(),
                gain,
                legacy: true,
                speak: Some((text.to_string(), gain, rate)),
            })?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- client

fn connect(ip: &str, port: u16) -> Result<TcpStream> {
    let ip = ip.trim();
    if ip.is_empty() {
        bail!("Sunucu IP adresi girilmemiş (Ayarlar > Ağ ayarları)");
    }
    let addr = (ip, port)
        .to_socket_addrs()
        .map_err(|_| anyhow::anyhow!("\"{ip}\" geçerli bir adres değil"))?
        .next()
        .ok_or_else(|| anyhow::anyhow!("Adres çözülemedi"))?;
    let s = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).map_err(|_| {
        anyhow::anyhow!("Sunucuya ({ip}:{port}) ulaşılamadı. Sunucu bilgisayarda FABE Anons açık ve 'Sunucu' modunda mı?")
    })?;
    s.set_read_timeout(Some(IO_TIMEOUT))?;
    s.set_write_timeout(Some(IO_TIMEOUT))?;
    Ok(s)
}

/// Asks the server who it is. Returns a message for the "Bağlantıyı test et" button.
pub fn ping(ip: &str, port: u16, me: &str) -> Result<String> {
    let start = std::time::Instant::now();
    let mut s = connect(ip, port)?;
    write_json(&mut s, &Request::Ping { from: me.into() })?;
    let mut reader = BufReader::new(s.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let ms = start.elapsed().as_millis();
    if let Ok(r) = serde_json::from_str::<Reply>(line.trim()) {
        if r.ok {
            return Ok(format!(
                "Bağlantı başarılı: {} (FABE Anons {}), {ms} ms",
                r.name.unwrap_or_default(),
                r.version.unwrap_or_default()
            ));
        }
    }
    bail!("Bu adreste FABE Anons sunucusu yok ya da eski sürüm çalışıyor. Sunucu bilgisayarı da 4.0'a güncelleyin.")
}

/// Sends an announcement to the server, uploading the sound if the server doesn't have it yet.
#[allow(clippy::too_many_arguments)]
pub fn send_call(ip: &str, port: u16, me: &str, title: &str, kind: &str, reps: u32, gain: f32, file: &Path) -> Result<()> {
    let hash = hash_file(file)?;
    let ext = file.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_else(|| "mp3".into());
    let mut s = connect(ip, port)?;
    let mut reader = BufReader::new(s.try_clone()?);
    write_json(&mut s, &Request::Call { from: me.into(), title: title.into(), hash, ext, reps, kind: kind.into(), gain })?;
    let mut r: Reply = read_json(&mut reader)?;
    if r.need.as_deref() == Some("file") {
        let data = std::fs::read(file)?;
        write_json(&mut s, &Request::File { size: data.len() as u64 })?;
        s.write_all(&data)?;
        s.flush()?;
        r = read_json(&mut reader)?;
    }
    if r.ok {
        Ok(())
    } else {
        bail!("Sunucu anonsu kabul etmedi: {}", r.error.unwrap_or_else(|| "bilinmeyen hata".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fabe-net-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn client_uploads_once_then_server_uses_cache_and_old_clients_still_work() {
        let dir = tmp("e2e");
        let (cache, sounds) = (dir.join("cache"), dir.join("sesler"));
        std::fs::create_dir_all(&sounds).unwrap();
        std::fs::write(sounds.join("ayse.mp3"), b"server-own-sound").unwrap();
        let (tx, rx) = mpsc::channel::<(String, String, bool, Vec<u8>)>();
        let tx = Mutex::new(tx);
        let handler: Handler = Arc::new(move |i: Incoming| {
            let bytes = match &i.speak {
                Some((text, gain, rate)) => format!("{text}|{gain}|{rate}").into_bytes(),
                None => std::fs::read(&i.file)?,
            };
            tx.lock().unwrap().send((i.from, i.title, i.legacy, bytes)).unwrap();
            Ok(())
        });
        let port = 40000 + (std::process::id() % 20000) as u16;
        let server = Server::start(port, "SUNUCU".into(), cache.clone(), sounds, handler);
        std::thread::sleep(Duration::from_millis(300));
        assert!(server.status().listening);

        assert!(ping("127.0.0.1", port, "KASA-1").unwrap().contains("SUNUCU"));

        let clip = dir.join("eren.ogg");
        std::fs::write(&clip, b"client-only-sound").unwrap();
        send_call("127.0.0.1", port, "KASA-1", "Eren", "person", 2, 1.0, &clip).unwrap();
        let (from, title, legacy, bytes) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!((from.as_str(), title.as_str(), legacy), ("KASA-1", "Eren", false));
        assert_eq!(bytes, b"client-only-sound");
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 1, "sound cached on server");

        // Second time: no upload needed, same result.
        send_call("127.0.0.1", port, "KASA-2", "Eren", "person", 2, 1.0, &clip).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap().0, "KASA-2");

        // Old client protocol.
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all("PLAY_ANONS:Ayşe:ayse".as_bytes()).unwrap();
        drop(s);
        let (_, title, legacy, bytes) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!((title.as_str(), legacy), ("Ayşe", true));
        assert_eq!(bytes, b"server-own-sound");
        // One row per computer (IP); the latest request names it.
        let clients = server.status().clients;
        assert_eq!(clients.len(), 1);
        assert!(clients[0].name.starts_with("Eski sürüm"));

        // Old client live announcement: text only, the server reads it.
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all("PLAY_TTS:6.0,0.9:Araç sahibi kasaya".as_bytes()).unwrap();
        drop(s);
        let (_, title, legacy, bytes) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!((title.as_str(), legacy), ("Araç sahibi kasaya", true));
        assert_eq!(bytes, "Araç sahibi kasaya|6|0.9".as_bytes());

        drop(server);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn restarting_on_the_same_port_works() {
        let dir = std::env::temp_dir().join(format!("fabe-net-restart-{}", std::process::id()));
        let handler: Handler = Arc::new(|_| Ok(()));
        let port = 40000 + ((std::process::id() + 7) % 20000) as u16;
        let start = || Server::start(port, "S".into(), dir.clone(), dir.clone(), handler.clone());
        let mut server = start();
        std::thread::sleep(Duration::from_millis(200));
        assert!(server.status().listening);
        for _ in 0..3 {
            drop(server); // saving network settings restarts the server
            server = start();
            std::thread::sleep(Duration::from_millis(200));
            let st = server.status();
            assert!(st.listening, "{:?}", st.error);
        }
        // A second server while the port is taken picks another free port by itself,
        // and clients find it without being told the port.
        let other = Server::start(port, "IKINCI".into(), dir.clone(), dir.clone(), handler.clone());
        std::thread::sleep(Duration::from_millis(2600));
        let st = other.status();
        assert!(st.listening, "{:?}", st.error);
        assert_ne!(st.port, port);
        let found = discover(Duration::from_millis(800));
        assert!(found.iter().any(|f| f.port == st.port || f.port == port), "{found:?}");
        drop(other);
        drop(server);
    }

    #[test]
    fn unreachable_server_gives_a_clear_message() {
        let e = send_call("127.0.0.1", 1, "X", "Eren", "person", 2, 1.0, Path::new("Cargo.toml")).unwrap_err();
        assert!(e.to_string().contains("ulaşılamadı"), "{e}");
    }
}
