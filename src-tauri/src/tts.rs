//! Live announcements ("Canlı anons"): text to speech.
//!
//! ema-lightning is the main engine. It runs in its own program (`fabe-tts.exe`, built from
//! `tts/fabe_tts.py`) that the app starts once and keeps running, so the model loads only once.
//! The models ship with the app; it never downloads anything.
//!
//! Piper is the automatic fallback: if ema-lightning is still loading, failed to start, gives an
//! error or takes too long, the same text is read with Piper and the screen says so. Every
//! failure is written to `hata.log` and the engine's own messages to `tts.log`.

use anyhow::{anyhow, bail, Context, Result};
use crossbeam_channel::{Receiver, RecvTimeoutError};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Loading torch and the model can take a while on an old office PC.
const READY_TIMEOUT: Duration = Duration::from_secs(180);
/// One announcement is a sentence or two; anything slower than this is treated as a failure.
const SPEAK_TIMEOUT: Duration = Duration::from_secs(30);
/// After a failed start, wait this long before trying ema-lightning again.
const RETRY_AFTER: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Ema,
    Piper,
}

/// Which engine the user wants ("Ayarlar > Canlı anons").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preference {
    Auto,
    Ema,
    Piper,
}

impl Preference {
    pub fn parse(s: &str) -> Self {
        match s {
            "ema" => Preference::Ema,
            "piper" => Preference::Piper,
            _ => Preference::Auto,
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// "yok" | "kapali" | "yukleniyor" | "hazir" | "hata"
    pub ema: String,
    pub ema_error: Option<String>,
    /// "yok" | "hazir"
    pub piper: String,
    /// Engine of the last announcement and why it fell back, if it did.
    pub last_engine: Option<Engine>,
    pub last_note: Option<String>,
}

pub struct Spoken {
    pub file: PathBuf,
    pub engine: Engine,
    /// Set when the fallback was used, e.g. "ema-lightning hâlâ yükleniyor".
    pub note: Option<String>,
}

/// How to start the ema-lightning program.
#[derive(Clone)]
struct EmaCmd {
    program: PathBuf,
    args: Vec<String>,
}

#[derive(Clone)]
struct PiperCmd {
    exe: PathBuf,
    voice: PathBuf,
}

enum EmaState {
    Off,
    Loading,
    Ready(EmaProc),
    Failed { error: String, at: Instant },
}

struct EmaProc {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
}

impl Drop for EmaProc {
    fn drop(&mut self) {
        let _ = writeln!(self.stdin, "{{\"quit\":true}}");
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Deserialize)]
struct Reply {
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    ready: Option<bool>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    ms: Option<u64>,
}

pub struct Tts {
    ema_cmd: Option<EmaCmd>,
    piper: Option<PiperCmd>,
    ema: Arc<Mutex<EmaState>>,
    last: Mutex<(Option<Engine>, Option<String>)>,
    temp: PathBuf,
    log: PathBuf,
    next: AtomicU64,
}

fn exe(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

fn no_window(c: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    let _ = c;
}

impl Tts {
    /// Finds the engines that ship with the app under `<resources>/resources/`.
    ///
    /// For development, `FABE_EMA_CMD` (e.g. `python tts/fabe_tts.py /path/to/models`) and
    /// `FABE_PIPER` / `FABE_PIPER_VOICE` override the bundled programs.
    pub fn new(resource_dir: Option<PathBuf>, data_dir: &Path) -> Self {
        let res = resource_dir.map(|d| d.join("resources"));
        let ema_cmd = std::env::var("FABE_EMA_CMD")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(|s| {
                let mut parts = s.split_whitespace().map(String::from);
                EmaCmd { program: PathBuf::from(parts.next().unwrap_or_default()), args: parts.collect() }
            })
            .or_else(|| {
                let res = res.as_ref()?;
                let program = res.join("tts").join("fabe-tts").join(exe("fabe-tts"));
                let models = res.join("tts").join("models");
                program.is_file().then(|| EmaCmd { program, args: vec![models.to_string_lossy().into_owned()] })
            });
        let piper = match (std::env::var_os("FABE_PIPER"), std::env::var_os("FABE_PIPER_VOICE")) {
            (Some(e), Some(v)) => Some(PiperCmd { exe: e.into(), voice: v.into() }),
            _ => res.as_ref().and_then(|res| {
                let dir = res.join("piper");
                let exe = dir.join(exe("piper"));
                let voice = std::fs::read_dir(&dir)
                    .ok()?
                    .flatten()
                    .map(|e| e.path())
                    .find(|p| p.extension().is_some_and(|x| x == "onnx"))?;
                exe.is_file().then_some(PiperCmd { exe, voice })
            }),
        };
        log::info!(
            "Canlı anons: ema-lightning {}, Piper {}",
            ema_cmd.as_ref().map(|c| c.program.display().to_string()).unwrap_or_else(|| "yok".into()),
            piper.as_ref().map(|c| c.voice.display().to_string()).unwrap_or_else(|| "yok".into()),
        );
        let temp = data_dir.join("canli-anons");
        let _ = std::fs::remove_dir_all(&temp); // leftovers from a crash
        let _ = std::fs::create_dir_all(&temp);
        Self {
            ema_cmd,
            piper,
            ema: Arc::new(Mutex::new(EmaState::Off)),
            last: Mutex::new((None, None)),
            temp,
            log: data_dir.join("tts.log"),
            next: AtomicU64::new(1),
        }
    }

    /// Starts loading ema-lightning in the background, so it is ready when the first
    /// announcement comes. Does nothing if it is already loading or loaded.
    pub fn warm_up(&self) {
        let Some(cmd) = self.ema_cmd.clone() else { return };
        {
            let mut st = self.ema.lock().unwrap_or_else(|e| e.into_inner());
            let busy = match &*st {
                EmaState::Loading | EmaState::Ready(_) => true,
                EmaState::Failed { at, .. } => at.elapsed() < RETRY_AFTER,
                EmaState::Off => false,
            };
            if busy {
                return;
            }
            *st = EmaState::Loading;
        }
        let (slot, log) = (self.ema.clone(), self.log.clone());
        std::thread::spawn(move || {
            let started = Instant::now();
            let next = match start_ema(&cmd, &log) {
                Ok(p) => {
                    log::info!("ema-lightning hazır ({} sn)", started.elapsed().as_secs());
                    EmaState::Ready(p)
                }
                Err(e) => {
                    log::error!("ema-lightning başlatılamadı: {e:#}");
                    EmaState::Failed { error: format!("{e:#}"), at: Instant::now() }
                }
            };
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = next;
        });
    }

    pub fn status(&self) -> Status {
        let (ema, ema_error) = match (&self.ema_cmd, &*self.ema.lock().unwrap_or_else(|e| e.into_inner())) {
            (None, _) => ("yok", None),
            (_, EmaState::Off) => ("kapali", None),
            (_, EmaState::Loading) => ("yukleniyor", None),
            (_, EmaState::Ready(_)) => ("hazir", None),
            (_, EmaState::Failed { error, .. }) => ("hata", Some(error.clone())),
        };
        let last = self.last.lock().unwrap_or_else(|e| e.into_inner()).clone();
        Status {
            ema: ema.into(),
            ema_error,
            piper: if self.piper.is_some() { "hazir" } else { "yok" }.into(),
            last_engine: last.0,
            last_note: last.1,
        }
    }

    /// Turns `text` into a WAV file (gain 1, the player applies "ses gücü").
    /// Blocks; call it off the UI thread.
    pub fn speak(&self, text: &str, rate: f32, pref: Preference) -> Result<Spoken> {
        let text = text.trim();
        if text.is_empty() {
            bail!("Okunacak metni yazın");
        }
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        let file = self.temp.join(format!("anons-{}-{n}.wav", std::process::id()));
        let rate = rate.clamp(0.5, 2.0);

        let mut why = None;
        if pref != Preference::Piper {
            match self.speak_ema(n, text, rate, &file) {
                Ok(()) => return Ok(self.done(file, Engine::Ema, None)),
                Err(e) => {
                    log::warn!("ema-lightning okuyamadı, Piper deneniyor: {e:#}");
                    why = Some(format!("{e:#}"));
                }
            }
            if pref == Preference::Ema && self.piper.is_none() {
                bail!("ema-lightning okuyamadı: {}", why.unwrap_or_default());
            }
        }
        match self.speak_piper(text, rate, &file) {
            Ok(()) => {
                let note = why.map(|w| format!("ema-lightning kullanılamadı ({w}); Piper ile okundu"));
                Ok(self.done(file, Engine::Piper, note))
            }
            Err(e) => {
                let _ = std::fs::remove_file(&file);
                log::error!("Piper okuyamadı: {e:#}");
                match why {
                    Some(w) => bail!("Canlı anons okunamadı. ema-lightning: {w}. Piper: {e:#}"),
                    None => Err(e.context("Piper okuyamadı")),
                }
            }
        }
    }

    fn done(&self, file: PathBuf, engine: Engine, note: Option<String>) -> Spoken {
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) = (Some(engine), note.clone());
        Spoken { file, engine, note }
    }

    fn speak_ema(&self, id: u64, text: &str, rate: f32, out: &Path) -> Result<()> {
        if self.ema_cmd.is_none() {
            bail!("ema-lightning bu kurulumda yok");
        }
        let mut st = self.ema.lock().unwrap_or_else(|e| e.into_inner());
        let proc = match &mut *st {
            EmaState::Ready(p) => p,
            EmaState::Loading => bail!("ema-lightning hâlâ yükleniyor"),
            EmaState::Failed { error, at } if at.elapsed() < RETRY_AFTER => bail!("{error}"),
            _ => {
                drop(st);
                self.warm_up();
                bail!("ema-lightning yeniden başlatılıyor");
            }
        };
        let req = serde_json::json!({ "id": id, "text": text, "speed": rate, "gain": 1.0, "out": out });
        let result = (|| -> Result<()> {
            writeln!(proc.stdin, "{req}").context("ema-lightning'e yazılamadı")?;
            proc.stdin.flush()?;
            let end = Instant::now() + SPEAK_TIMEOUT;
            loop {
                let left = end.saturating_duration_since(Instant::now());
                let line = match proc.lines.recv_timeout(left) {
                    Ok(l) => l,
                    Err(RecvTimeoutError::Timeout) => bail!("ema-lightning {} sn içinde yanıt vermedi", SPEAK_TIMEOUT.as_secs()),
                    Err(RecvTimeoutError::Disconnected) => bail!("ema-lightning kapandı"),
                };
                let Ok(r) = serde_json::from_str::<Reply>(&line) else { continue };
                if r.id != Some(id) {
                    continue; // a late answer to a request that already timed out
                }
                if !r.ok {
                    bail!("{}", r.error.unwrap_or_else(|| "bilinmeyen hata".into()));
                }
                if !out.is_file() {
                    bail!("ema-lightning ses dosyası yazmadı");
                }
                log::info!("ema-lightning {} ms", r.ms.unwrap_or(0));
                return Ok(());
            }
        })();
        if result.is_err() {
            // A stuck or crashed engine is restarted for the next announcement.
            *st = EmaState::Off;
        }
        result
    }

    fn speak_piper(&self, text: &str, rate: f32, out: &Path) -> Result<()> {
        let Some(p) = &self.piper else { bail!("Piper bu kurulumda yok") };
        let mut c = Command::new(&p.exe);
        c.arg("--model")
            .arg(&p.voice)
            .arg("--output_file")
            .arg(out)
            .arg("--length_scale")
            .arg(format!("{:.2}", 1.0 / rate))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if let Some(dir) = p.exe.parent() {
            c.current_dir(dir);
        }
        no_window(&mut c);
        let mut child = c.spawn().context("Piper başlatılamadı")?;
        {
            let mut stdin = child.stdin.take().ok_or_else(|| anyhow!("Piper'a yazılamadı"))?;
            // One line: Piper reads one utterance per line.
            stdin.write_all(text.replace(['\r', '\n'], " ").as_bytes())?;
            stdin.write_all(b"\n")?;
        }
        let end = Instant::now() + SPEAK_TIMEOUT;
        loop {
            if let Some(status) = child.try_wait()? {
                if !status.success() || !out.is_file() {
                    let mut err = String::new();
                    if let Some(mut e) = child.stderr.take() {
                        let _ = std::io::Read::read_to_string(&mut e, &mut err);
                    }
                    bail!("Piper hata verdi: {}", err.lines().last().unwrap_or("").trim());
                }
                return Ok(());
            }
            if Instant::now() > end {
                let _ = child.kill();
                bail!("Piper {} sn içinde bitiremedi", SPEAK_TIMEOUT.as_secs());
            }
            std::thread::sleep(Duration::from_millis(30));
        }
    }
}

fn start_ema(cmd: &EmaCmd, log_path: &Path) -> Result<EmaProc> {
    let mut c = Command::new(&cmd.program);
    c.args(&cmd.args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(dir) = cmd.program.parent().filter(|d| !d.as_os_str().is_empty()) {
        c.current_dir(dir);
    }
    c.env("PYTHONIOENCODING", "utf-8").env("PYTHONUNBUFFERED", "1");
    no_window(&mut c);
    let mut child = c.spawn().with_context(|| format!("{} başlatılamadı", cmd.program.display()))?;
    let stdout = child.stdout.take().ok_or_else(|| anyhow!("çıktı yok"))?;
    let stderr = child.stderr.take().ok_or_else(|| anyhow!("çıktı yok"))?;
    let stdin = child.stdin.take().ok_or_else(|| anyhow!("girdi yok"))?;

    let (tx, lines) = crossbeam_channel::unbounded();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    // The engine's own messages and Python tracebacks go to tts.log (kept small).
    let log_path = log_path.to_path_buf();
    std::thread::spawn(move || {
        if std::fs::metadata(&log_path).map(|m| m.len() > 2_000_000).unwrap_or(false) {
            let _ = std::fs::remove_file(&log_path);
        }
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&log_path).ok();
        if let Some(f) = f.as_mut() {
            let _ = writeln!(f, "--- {} başlatıldı ---", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
        }
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if let Some(f) = f.as_mut() {
                let _ = writeln!(f, "{line}");
            }
        }
    });

    let mut proc = EmaProc { child, stdin, lines };
    let end = Instant::now() + READY_TIMEOUT;
    loop {
        let left = end.saturating_duration_since(Instant::now());
        let line = match proc.lines.recv_timeout(left) {
            Ok(l) => l,
            Err(RecvTimeoutError::Timeout) => bail!("{} sn içinde yüklenemedi", READY_TIMEOUT.as_secs()),
            Err(RecvTimeoutError::Disconnected) => {
                let code = proc.child.wait().ok().and_then(|s| s.code());
                bail!("açılırken kapandı (kod {code:?}); ayrıntı tts.log dosyasında")
            }
        };
        let Ok(r) = serde_json::from_str::<Reply>(&line) else { continue };
        match r.ready {
            Some(true) => return Ok(proc),
            Some(false) => bail!("{}", r.error.unwrap_or_else(|| "yüklenemedi".into())),
            None => continue,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fabe-tts-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A stand-in engine (a shell script) that speaks the same protocol, so the fallback logic
    /// can be tested without torch.
    #[cfg(unix)]
    fn fake_engine(dir: &Path, body: &str) -> EmaCmd {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("fake-ema.sh");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        EmaCmd { program: p, args: vec![] }
    }

    #[cfg(unix)]
    fn tts_with(dir: &Path, ema: Option<EmaCmd>, piper: Option<PiperCmd>) -> Tts {
        Tts {
            ema_cmd: ema,
            piper,
            ema: Arc::new(Mutex::new(EmaState::Off)),
            last: Mutex::new((None, None)),
            temp: dir.join("tmp"),
            log: dir.join("tts.log"),
            next: AtomicU64::new(1),
        }
    }

    #[cfg(unix)]
    fn wait_ready(t: &Tts) -> String {
        for _ in 0..200 {
            let s = t.status().ema;
            if s != "yukleniyor" {
                return s;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        "zaman aşımı".into()
    }

    #[cfg(unix)]
    #[test]
    fn uses_ema_when_ready() {
        let dir = tmp("ok");
        std::fs::create_dir_all(dir.join("tmp")).unwrap();
        // Copies a small WAV to the requested path, like the real engine writes one.
        let wav = dir.join("src.wav");
        std::fs::write(&wav, b"RIFF").unwrap();
        let ema = fake_engine(
            &dir,
            &format!(
                r#"echo '{{"ready":true}}'
while read -r line; do
  id=$(echo "$line" | sed 's/.*"id":\([0-9]*\).*/\1/')
  out=$(echo "$line" | sed 's/.*"out":"\([^"]*\)".*/\1/')
  cp "{}" "$out"
  echo "{{\"id\":$id,\"ok\":true,\"ms\":3}}"
done"#,
                wav.display()
            ),
        );
        let t = tts_with(&dir, Some(ema), None);
        t.warm_up();
        assert_eq!(wait_ready(&t), "hazir");
        let s = t.speak("Ahmet Bey kasaya", 1.0, Preference::Auto).unwrap();
        assert_eq!(s.engine, Engine::Ema);
        assert!(s.file.is_file());
        assert!(s.note.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn falls_back_to_piper_when_ema_fails() {
        let dir = tmp("fb");
        std::fs::create_dir_all(dir.join("tmp")).unwrap();
        let ema = fake_engine(&dir, r#"echo '{"ready":false,"error":"Model dosyaları bulunamadı"}'"#);
        // A stand-in Piper that writes the --output_file it is given.
        let piper = dir.join("piper.sh");
        std::fs::write(&piper, "#!/bin/sh\nwhile [ $# -gt 0 ]; do [ \"$1\" = --output_file ] && out=$2; shift; done\ncat > /dev/null\necho RIFF > \"$out\"\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&piper, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let t = tts_with(&dir, Some(ema), Some(PiperCmd { exe: piper, voice: dir.join("tr.onnx") }));
        t.warm_up();
        assert_eq!(wait_ready(&t), "hata");
        let s = t.speak("Mehmet Bey depoya", 1.0, Preference::Auto).unwrap();
        assert_eq!(s.engine, Engine::Piper);
        assert!(s.note.unwrap().contains("Model dosyaları bulunamadı"));
        assert!(s.file.is_file());
        // Asking for Piper only never touches ema-lightning.
        let s = t.speak("Deneme", 1.0, Preference::Piper).unwrap();
        assert_eq!(s.engine, Engine::Piper);
        assert!(s.note.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn empty_text_is_refused() {
        let dir = tmp("empty");
        let t = Tts::new(None, &dir);
        assert!(t.speak("   ", 1.0, Preference::Auto).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
