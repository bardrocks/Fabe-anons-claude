//! Live announcements ("Canlı anons"): text to speech.
//!
//! Engines, tried in this order (Ayarlar > Canlı anons can skip the first or second):
//!
//! 1. ema-lightning, the main engine. It runs in its own program (`fabe-tts.exe`, built from
//!    `tts/fabe_tts.py`) that the app starts once and keeps running, so the model loads only
//!    once. The models ship with the app; it never downloads anything.
//! 2. Microsoft Edge TTS (Emel = kadın, Ahmet = erkek), also its own program (`fabe-edge.exe`,
//!    built from `tts/fabe_edge.py`) shipped with the app, so no PC needs anything installed.
//!    It needs internet.
//! 3. The Turkish voice built into Windows (Tolga), the last resort when there is no internet.
//!
//! Every fallback is shown on screen and written to `hata.log`; the engines' own messages go to
//! `tts.log`.

use anyhow::{anyhow, bail, Context, Result};
use crossbeam_channel::{Receiver, RecvTimeoutError};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// After a failed start, wait this long before trying an engine program again.
const RETRY_AFTER: Duration = Duration::from_secs(60);
/// The Windows voice runs through PowerShell, which is slow to start on an old PC.
const WINDOWS_TIMEOUT: Duration = Duration::from_secs(40);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Ema,
    Edge,
    Windows,
}

/// Which engines the user wants ("Ayarlar > Canlı anons").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preference {
    /// ema-lightning, then Edge, then Windows.
    Auto,
    /// ema-lightning, then Windows (never uses the internet).
    Ema,
    /// Edge, then Windows (ema-lightning is not even loaded).
    Edge,
}

impl Preference {
    pub fn parse(s: &str) -> Self {
        match s {
            "ema" => Preference::Ema,
            // "piper" was saved by 4.3 and older; Edge took Piper's place.
            "edge" | "piper" => Preference::Edge,
            _ => Preference::Auto,
        }
    }

    fn engines(self) -> &'static [Engine] {
        match self {
            Preference::Auto => &[Engine::Ema, Engine::Edge, Engine::Windows],
            Preference::Ema => &[Engine::Ema, Engine::Windows],
            Preference::Edge => &[Engine::Edge, Engine::Windows],
        }
    }

    pub fn uses_ema(self) -> bool {
        self != Preference::Edge
    }
}

/// Edge voice ("Ayarlar > Canlı anons > Ses").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Voice {
    Kadin,
    Erkek,
}

impl Voice {
    pub fn parse(s: &str) -> Self {
        if s == "erkek" { Voice::Erkek } else { Voice::Kadin }
    }

    fn edge_name(self) -> &'static str {
        match self {
            Voice::Kadin => "tr-TR-EmelNeural",
            Voice::Erkek => "tr-TR-AhmetNeural",
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// "yok" | "kapali" | "yukleniyor" | "hazir" | "hata"
    pub ema: String,
    pub ema_error: Option<String>,
    /// Same values as `ema`.
    pub edge: String,
    pub edge_error: Option<String>,
    /// "var" | "yok": whether the Windows voice can be tried on this PC.
    pub windows: String,
    /// Engine of the last announcement and why it fell back, if it did.
    pub last_engine: Option<Engine>,
    pub last_note: Option<String>,
}

#[derive(Debug)]
pub struct Spoken {
    pub file: PathBuf,
    pub engine: Engine,
    /// Set when a fallback was used, e.g. "ema-lightning hâlâ yükleniyor; Edge (Emel) ile okundu".
    pub note: Option<String>,
}

/// How to start an engine program.
#[derive(Clone)]
struct Cmd {
    program: PathBuf,
    args: Vec<String>,
}

enum State {
    Off,
    Loading,
    Ready(Proc),
    Failed { error: String, at: Instant },
}

struct Proc {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
}

impl Drop for Proc {
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

/// An engine that runs as its own long-lived program and speaks the JSON-lines protocol of
/// `tts/fabe_tts.py` and `tts/fabe_edge.py`.
struct Sidecar {
    name: &'static str,
    cmd: Option<Cmd>,
    state: Arc<Mutex<State>>,
    ready_timeout: Duration,
    speak_timeout: Duration,
    log: PathBuf,
}

impl Sidecar {
    fn new(name: &'static str, cmd: Option<Cmd>, ready_timeout: Duration, speak_timeout: Duration, log: PathBuf) -> Self {
        Self { name, cmd, state: Arc::new(Mutex::new(State::Off)), ready_timeout, speak_timeout, log }
    }

    /// Starts the program in the background. Does nothing if it is already loading or loaded.
    fn warm_up(&self) {
        let Some(cmd) = self.cmd.clone() else { return };
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let busy = match &*st {
                State::Loading | State::Ready(_) => true,
                State::Failed { at, .. } => at.elapsed() < RETRY_AFTER,
                State::Off => false,
            };
            if busy {
                return;
            }
            *st = State::Loading;
        }
        let (slot, log, name, timeout) = (self.state.clone(), self.log.clone(), self.name, self.ready_timeout);
        std::thread::spawn(move || {
            let started = Instant::now();
            let next = match start(&cmd, &log, name, timeout) {
                Ok(p) => {
                    log::info!("{name} hazır ({} sn)", started.elapsed().as_secs());
                    State::Ready(p)
                }
                Err(e) => {
                    log::error!("{name} başlatılamadı: {e:#}");
                    State::Failed { error: format!("{e:#}"), at: Instant::now() }
                }
            };
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = next;
        });
    }

    fn status(&self) -> (String, Option<String>) {
        let (s, e) = match (&self.cmd, &*self.state.lock().unwrap_or_else(|e| e.into_inner())) {
            (None, _) => ("yok", None),
            (_, State::Off) => ("kapali", None),
            (_, State::Loading) => ("yukleniyor", None),
            (_, State::Ready(_)) => ("hazir", None),
            (_, State::Failed { error, .. }) => ("hata", Some(error.clone())),
        };
        (s.into(), e)
    }

    /// Sends one request and waits for its answer. `req` must carry `id` and `out`.
    fn request(&self, id: u64, req: serde_json::Value, out: &Path) -> Result<()> {
        let name = self.name;
        if self.cmd.is_none() {
            bail!("{name} bu kurulumda yok");
        }
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let proc = match &mut *st {
            State::Ready(p) => p,
            State::Loading => bail!("{name} hâlâ yükleniyor"),
            State::Failed { error, at } if at.elapsed() < RETRY_AFTER => bail!("{error}"),
            _ => {
                drop(st);
                self.warm_up();
                bail!("{name} yeniden başlatılıyor");
            }
        };
        // Set when the program is stuck or gone; an ordinary error (e.g. no internet) leaves it running.
        let mut restart = true;
        let result = (|| -> Result<()> {
            writeln!(proc.stdin, "{req}").with_context(|| format!("{name} programına yazılamadı"))?;
            proc.stdin.flush()?;
            let end = Instant::now() + self.speak_timeout;
            loop {
                let left = end.saturating_duration_since(Instant::now());
                let line = match proc.lines.recv_timeout(left) {
                    Ok(l) => l,
                    Err(RecvTimeoutError::Timeout) => bail!("{name} {} sn içinde yanıt vermedi", self.speak_timeout.as_secs()),
                    Err(RecvTimeoutError::Disconnected) => bail!("{name} kapandı"),
                };
                let Ok(r) = serde_json::from_str::<Reply>(&line) else { continue };
                if r.id != Some(id) {
                    continue; // a late answer to a request that already timed out
                }
                restart = false;
                if !r.ok {
                    bail!("{}", r.error.unwrap_or_else(|| "bilinmeyen hata".into()));
                }
                if !out.is_file() {
                    bail!("{name} ses dosyası yazmadı");
                }
                log::info!("{name} {} ms", r.ms.unwrap_or(0));
                return Ok(());
            }
        })();
        if result.is_err() && restart {
            *st = State::Off; // restarted for the next announcement
        }
        result
    }
}

pub struct Tts {
    ema: Sidecar,
    edge: Sidecar,
    windows: bool,
    last: Mutex<(Option<Engine>, Option<String>)>,
    temp: PathBuf,
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

/// `FABE_EMA_CMD="python tts/fabe_tts.py /path/to/models"` style override for development.
fn cmd_from_env(var: &str) -> Option<Cmd> {
    let s = std::env::var(var).ok().filter(|s| !s.trim().is_empty())?;
    let mut parts = s.split_whitespace().map(String::from);
    Some(Cmd { program: PathBuf::from(parts.next().unwrap_or_default()), args: parts.collect() })
}

fn label(engine: Engine, voice: Voice) -> String {
    match engine {
        Engine::Ema => "ema-lightning".into(),
        Engine::Edge => match voice {
            Voice::Kadin => "Edge (Emel)".into(),
            Voice::Erkek => "Edge (Ahmet)".into(),
        },
        Engine::Windows => "Windows sesi".into(),
    }
}

impl Tts {
    /// Finds the engines that ship with the app under `<resources>/resources/tts/`.
    ///
    /// For development, `FABE_EMA_CMD` and `FABE_EDGE_CMD` (e.g. `python tts/fabe_edge.py`)
    /// override the bundled programs.
    pub fn new(resource_dir: Option<PathBuf>, data_dir: &Path) -> Self {
        let tts_dir = resource_dir.map(|d| d.join("resources").join("tts"));
        let ema_cmd = cmd_from_env("FABE_EMA_CMD").or_else(|| {
            let dir = tts_dir.as_ref()?;
            let program = dir.join("fabe-tts").join(exe("fabe-tts"));
            let models = dir.join("models");
            program.is_file().then(|| Cmd { program, args: vec![models.to_string_lossy().into_owned()] })
        });
        let edge_cmd = cmd_from_env("FABE_EDGE_CMD").or_else(|| {
            let program = tts_dir.as_ref()?.join("fabe-edge").join(exe("fabe-edge"));
            program.is_file().then(|| Cmd { program, args: vec![] })
        });
        log::info!(
            "Canlı anons: ema-lightning {}, Edge {}",
            ema_cmd.as_ref().map(|c| c.program.display().to_string()).unwrap_or_else(|| "yok".into()),
            edge_cmd.as_ref().map(|c| c.program.display().to_string()).unwrap_or_else(|| "yok".into()),
        );
        let temp = data_dir.join("canli-anons");
        let _ = std::fs::remove_dir_all(&temp); // leftovers from a crash
        let _ = std::fs::create_dir_all(&temp);
        let log = data_dir.join("tts.log");
        Self {
            // Loading torch and the model can take a while on an old office PC.
            ema: Sidecar::new("ema-lightning", ema_cmd, Duration::from_secs(180), Duration::from_secs(30), log.clone()),
            // Each announcement is one round trip to Microsoft.
            edge: Sidecar::new("Edge", edge_cmd, Duration::from_secs(60), Duration::from_secs(25), log),
            windows: cfg!(windows),
            last: Mutex::new((None, None)),
            temp,
            next: AtomicU64::new(1),
        }
    }

    /// Starts the engine programs in the background, so they are ready when the first
    /// announcement comes.
    pub fn warm_up(&self, pref: Preference) {
        if pref.uses_ema() {
            self.ema.warm_up();
        }
        self.edge.warm_up();
    }

    pub fn status(&self) -> Status {
        let (ema, ema_error) = self.ema.status();
        let (edge, edge_error) = self.edge.status();
        let last = self.last.lock().unwrap_or_else(|e| e.into_inner()).clone();
        Status {
            ema,
            ema_error,
            edge,
            edge_error,
            windows: if self.windows { "var" } else { "yok" }.into(),
            last_engine: last.0,
            last_note: last.1,
        }
    }

    /// Turns `text` into a sound file (gain 1, the player applies "ses gücü").
    /// Blocks; call it off the UI thread.
    pub fn speak(&self, text: &str, rate: f32, pref: Preference, voice: Voice) -> Result<Spoken> {
        let text = text.trim();
        if text.is_empty() {
            bail!("Okunacak metni yazın");
        }
        // Text typed with Caps Lock would otherwise be spelled letter by letter.
        // Turkish rule: I -> ı, İ -> i.
        let text = &text.replace('I', "ı").replace('İ', "i").to_lowercase();
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        let rate = rate.clamp(0.5, 2.0);

        let mut failed: Vec<String> = Vec::new();
        for &engine in pref.engines() {
            let ext = if engine == Engine::Edge { "mp3" } else { "wav" };
            let file = self.temp.join(format!("anons-{}-{n}.{ext}", std::process::id()));
            let r = match engine {
                Engine::Ema => self.ema.request(
                    n,
                    serde_json::json!({ "id": n, "text": text, "speed": rate, "gain": 1.0, "out": file }),
                    &file,
                ),
                Engine::Edge => self.edge.request(
                    n,
                    serde_json::json!({ "id": n, "text": text, "voice": voice.edge_name(), "speed": rate, "out": file }),
                    &file,
                ),
                Engine::Windows => self.speak_windows(text, rate, &file),
            };
            match r {
                Ok(()) => {
                    let note = (!failed.is_empty())
                        .then(|| format!("{}; {} ile okundu", failed.join("; "), label(engine, voice)));
                    *self.last.lock().unwrap_or_else(|e| e.into_inner()) = (Some(engine), note.clone());
                    return Ok(Spoken { file, engine, note });
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&file);
                    log::warn!("{} okuyamadı: {e:#}", label(engine, voice));
                    failed.push(format!("{}: {e:#}", label(engine, voice)));
                }
            }
        }
        log::error!("Canlı anons okunamadı: {}", failed.join("; "));
        bail!("Canlı anons okunamadı. {}", failed.join(". "))
    }

    /// The Turkish voice built into Windows, through PowerShell: the modern voices (Tolga, the
    /// same ones Windows Narrator uses) first, then the older SAPI voices.
    fn speak_windows(&self, text: &str, rate: f32, out: &Path) -> Result<()> {
        if !self.windows {
            bail!("yalnızca Windows'ta var");
        }
        let script: Vec<u8> = WINDOWS_SCRIPT.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let mut c = Command::new("powershell.exe");
        c.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &base64(&script)])
            .env("FABE_TTS_TEXT", text.replace(['\r', '\n'], " "))
            .env("FABE_TTS_OUT", out)
            .env("FABE_TTS_RATE", format!("{rate:.2}"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        no_window(&mut c);
        let mut child = c.spawn().context("PowerShell başlatılamadı")?;
        let end = Instant::now() + WINDOWS_TIMEOUT;
        loop {
            if let Some(status) = child.try_wait()? {
                let mut msg = String::new();
                if let Some(mut e) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut e, &mut msg);
                }
                if !status.success() || !out.is_file() || std::fs::metadata(out).map(|m| m.len() < 1000).unwrap_or(true) {
                    let line = msg.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("ses üretilemedi");
                    bail!("{line}");
                }
                let mut which = String::new();
                if let Some(mut o) = child.stdout.take() {
                    let _ = std::io::Read::read_to_string(&mut o, &mut which);
                }
                log::info!("Windows sesi: {}", which.trim());
                return Ok(());
            }
            if Instant::now() > end {
                let _ = child.kill();
                bail!("{} sn içinde bitiremedi", WINDOWS_TIMEOUT.as_secs());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// Runs in Windows PowerShell 5.1. Text, output path and speed come in environment variables so
/// nothing needs quoting. Writes a WAV; prints which voice it used; errors go to stderr.
const WINDOWS_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$text = $env:FABE_TTS_TEXT
$out = $env:FABE_TTS_OUT
$rate = [double]::Parse($env:FABE_TTS_RATE, [Globalization.CultureInfo]::InvariantCulture)
$modern = $null
try {
  Add-Type -AssemblyName System.Runtime.WindowsRuntime
  $null = [Windows.Media.SpeechSynthesis.SpeechSynthesizer, Windows.Media.SpeechSynthesis, ContentType = WindowsRuntime]
  $voice = [Windows.Media.SpeechSynthesis.SpeechSynthesizer]::AllVoices | Where-Object { $_.Language -like 'tr*' } | Select-Object -First 1
  if ($voice) {
    $asTask = [System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {
      $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1'
    } | Select-Object -First 1
    $s = New-Object Windows.Media.SpeechSynthesis.SpeechSynthesizer
    $s.Voice = $voice
    $s.Options.SpeakingRate = [Math]::Max(0.5, [Math]::Min(2.0, $rate))
    $task = $asTask.MakeGenericMethod([Windows.Media.SpeechSynthesis.SpeechSynthesisStream]).Invoke($null, @($s.SynthesizeTextToStreamAsync($text)))
    $null = $task.Wait(30000)
    $src = [System.IO.WindowsRuntimeStreamExtensions]::AsStream($task.Result)
    $dst = [IO.File]::Create($out)
    try { $src.CopyTo($dst) } finally { $dst.Close(); $src.Close() }
    $modern = $voice.DisplayName
  }
} catch {
  [Console]::Error.WriteLine("Yeni Windows sesleri: $($_.Exception.Message)")
}
if ($modern) { "Windows: $modern"; exit 0 }
Add-Type -AssemblyName System.Speech
$s = New-Object System.Speech.Synthesis.SpeechSynthesizer
$v = $s.GetInstalledVoices() | Where-Object { $_.Enabled -and $_.VoiceInfo.Culture.Name -like 'tr*' } | Select-Object -First 1
if (-not $v) { [Console]::Error.WriteLine('Bu bilgisayarda Türkçe Windows sesi yok'); exit 2 }
$s.SelectVoice($v.VoiceInfo.Name)
$s.Rate = [int][Math]::Max(-10, [Math]::Min(10, [Math]::Round(($rate - 1) * 10)))
$s.SetOutputToWaveFile($out)
$s.Speak($text)
$s.Dispose()
"SAPI: $($v.VoiceInfo.Name)"
"#;

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

fn start(cmd: &Cmd, log_path: &Path, name: &'static str, ready_timeout: Duration) -> Result<Proc> {
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
        let open = || std::fs::OpenOptions::new().create(true).append(true).open(&log_path).ok();
        if let Some(mut f) = open() {
            let _ = writeln!(f, "--- {} {name} başlatıldı ---", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
        }
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            // Opened per line: two engines write to the same file.
            if let Some(mut f) = open() {
                let _ = writeln!(f, "[{name}] {line}");
            }
        }
    });

    let mut proc = Proc { child, stdin, lines };
    let end = Instant::now() + ready_timeout;
    loop {
        let left = end.saturating_duration_since(Instant::now());
        let line = match proc.lines.recv_timeout(left) {
            Ok(l) => l,
            Err(RecvTimeoutError::Timeout) => bail!("{} sn içinde yüklenemedi", ready_timeout.as_secs()),
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
        std::fs::create_dir_all(d.join("tmp")).unwrap();
        d
    }

    /// A stand-in engine (a shell script) that speaks the same protocol, so the fallback logic
    /// can be tested without torch or the internet. It copies `src` to the requested path, or
    /// answers with `error` when the text contains "hata".
    #[cfg(unix)]
    fn fake(dir: &Path, file: &str, src: &Path) -> Cmd {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(file);
        let body = format!(
            r#"#!/bin/sh
echo '{{"ready":true}}'
while read -r line; do
  id=$(echo "$line" | sed 's/.*"id":\([0-9]*\).*/\1/')
  out=$(echo "$line" | sed 's/.*"out":"\([^"]*\)".*/\1/')
  case "$line" in
    *hata*) echo "{{\"id\":$id,\"ok\":false,\"error\":\"İnternete bağlanılamadı\"}}" ;;
    *) printf '%s' "$line" > "{log}"; cp "{src}" "$out"; echo "{{\"id\":$id,\"ok\":true,\"ms\":3}}" ;;
  esac
done
"#,
            src = src.display(),
            log = dir.join(format!("{file}.last")).display(),
        );
        std::fs::write(&p, body).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        Cmd { program: p, args: vec![] }
    }

    #[cfg(unix)]
    fn broken(dir: &Path) -> Cmd {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("broken.sh");
        std::fs::write(&p, "#!/bin/sh\necho '{\"ready\":false,\"error\":\"Model dosyaları bulunamadı\"}'\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        Cmd { program: p, args: vec![] }
    }

    fn tts_with(dir: &Path, ema: Option<Cmd>, edge: Option<Cmd>) -> Tts {
        let log = dir.join("tts.log");
        Tts {
            ema: Sidecar::new("ema-lightning", ema, Duration::from_secs(5), Duration::from_secs(5), log.clone()),
            edge: Sidecar::new("Edge", edge, Duration::from_secs(5), Duration::from_secs(5), log),
            windows: false,
            last: Mutex::new((None, None)),
            temp: dir.join("tmp"),
            next: AtomicU64::new(1),
        }
    }

    fn wait_ready(t: &Tts) -> (String, String) {
        for _ in 0..250 {
            let s = t.status();
            if s.ema != "yukleniyor" && s.edge != "yukleniyor" {
                return (s.ema, s.edge);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        ("zaman aşımı".into(), "zaman aşımı".into())
    }

    #[cfg(unix)]
    fn wav(dir: &Path) -> PathBuf {
        let p = dir.join("src.wav");
        std::fs::write(&p, b"RIFF").unwrap();
        p
    }

    #[cfg(unix)]
    #[test]
    fn uses_ema_when_ready() {
        let dir = tmp("ok");
        let src = wav(&dir);
        let t = tts_with(&dir, Some(fake(&dir, "ema.sh", &src)), Some(fake(&dir, "edge.sh", &src)));
        t.warm_up(Preference::Auto);
        assert_eq!(wait_ready(&t), ("hazir".into(), "hazir".into()));
        let s = t.speak("Ahmet Bey kasaya", 1.0, Preference::Auto, Voice::Kadin).unwrap();
        assert_eq!(s.engine, Engine::Ema);
        assert!(s.file.is_file());
        assert!(s.note.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn falls_back_to_edge_with_the_chosen_voice() {
        let dir = tmp("fb");
        let src = wav(&dir);
        let t = tts_with(&dir, Some(broken(&dir)), Some(fake(&dir, "edge.sh", &src)));
        t.warm_up(Preference::Auto);
        assert_eq!(wait_ready(&t), ("hata".into(), "hazir".into()));
        let s = t.speak("Mehmet Bey depoya", 0.8, Preference::Auto, Voice::Erkek).unwrap();
        assert_eq!(s.engine, Engine::Edge);
        assert_eq!(s.file.extension().unwrap(), "mp3");
        let note = s.note.unwrap();
        assert!(note.contains("Model dosyaları bulunamadı") && note.contains("Edge (Ahmet) ile okundu"), "{note}");
        let sent = std::fs::read_to_string(dir.join("edge.sh.last")).unwrap();
        assert!(sent.contains("tr-TR-AhmetNeural") && sent.contains("\"speed\":0.8"), "{sent}");
        assert_eq!(t.status().last_engine, Some(Engine::Edge));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn edge_only_never_starts_ema() {
        let dir = tmp("edge");
        let src = wav(&dir);
        let t = tts_with(&dir, Some(fake(&dir, "ema.sh", &src)), Some(fake(&dir, "edge.sh", &src)));
        t.warm_up(Preference::Edge);
        assert_eq!(wait_ready(&t), ("kapali".into(), "hazir".into()));
        let s = t.speak("Deneme", 1.0, Preference::Edge, Voice::Kadin).unwrap();
        assert_eq!(s.engine, Engine::Edge);
        assert!(s.note.is_none());
        assert!(std::fs::read_to_string(dir.join("edge.sh.last")).unwrap().contains("tr-TR-EmelNeural"));
        assert!(!dir.join("ema.sh.last").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn every_failure_is_reported() {
        let dir = tmp("all");
        let src = wav(&dir);
        let t = tts_with(&dir, None, Some(fake(&dir, "edge.sh", &src)));
        t.warm_up(Preference::Auto);
        wait_ready(&t);
        // "hata" makes the stand-in Edge fail like it does without internet; Windows is off here.
        let e = format!("{:#}", t.speak("hata", 1.0, Preference::Auto, Voice::Kadin).unwrap_err());
        assert!(e.contains("ema-lightning bu kurulumda yok"), "{e}");
        assert!(e.contains("Edge (Emel): İnternete bağlanılamadı"), "{e}");
        assert!(e.contains("Windows sesi"), "{e}");
        // The Edge program keeps running after an ordinary error.
        assert_eq!(t.status().edge, "hazir");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn capitals_are_lowered_the_turkish_way() {
        let dir = tmp("caps");
        let src = wav(&dir);
        let t = tts_with(&dir, None, Some(fake(&dir, "edge.sh", &src)));
        t.warm_up(Preference::Edge);
        wait_ready(&t);
        t.speak("IŞIL HANIM İRSALİYE İÇİN KASAYA", 1.0, Preference::Edge, Voice::Kadin).unwrap();
        let sent = std::fs::read_to_string(dir.join("edge.sh.last")).unwrap();
        assert!(sent.contains("ışıl hanım irsaliye için kasaya"), "{sent}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn old_piper_setting_means_edge() {
        assert_eq!(Preference::parse("piper"), Preference::Edge);
        assert_eq!(Preference::parse("edge"), Preference::Edge);
        assert_eq!(Preference::parse("ema"), Preference::Ema);
        assert_eq!(Preference::parse("otomatik"), Preference::Auto);
        assert_eq!(Voice::parse("erkek").edge_name(), "tr-TR-AhmetNeural");
        assert_eq!(Voice::parse("").edge_name(), "tr-TR-EmelNeural");
    }

    #[test]
    fn base64_matches_powershell_encoding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn empty_text_is_refused() {
        let dir = tmp("empty");
        let t = Tts::new(None, &dir);
        assert!(t.speak("   ", 1.0, Preference::Auto, Voice::Kadin).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
