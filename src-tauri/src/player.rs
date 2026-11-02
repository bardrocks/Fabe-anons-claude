//! The announcement queue and audio output.
//!
//! One thread owns the sound card. Every announcement - a staff button, a live (TTS)
//! announcement, a scheduled break, or a request from another computer - goes into the
//! same queue, so two people pressing at once never cut each other off.

use crate::ducking::Ducker;
use anyhow::{Context, Result};
use crossbeam_channel::{unbounded, Receiver, Sender};
use rodio::{buffer::SamplesBuffer, source::UniformSourceIterator, Decoder, OutputStream, Sink, Source};
use serde::Serialize;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Gap between the two readings of a name.
const REPEAT_GAP: Duration = Duration::from_millis(1000);
/// Every clip is converted to this one format before it reaches the sound card. Switching
/// sample rates between clips (gong 44.1 kHz, Piper 22.05 kHz...) garbled the start of the
/// next clip on some PCs.
const OUT_RATE: u32 = 48_000;
const OUT_CHANNELS: u16 = 2;
const TICK: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Person,
    Tts,
    Break,
}

#[derive(Debug, Clone)]
pub struct Item {
    pub id: u64,
    pub title: String,
    pub kind: Kind,
    pub source: String,
    pub file: PathBuf,
    pub reps: u32,
    /// Extra gain for this item (live announcements use the "ses gücü" setting).
    pub gain: f32,
    /// Delete `file` after playing (temporary TTS output).
    pub temp: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueView {
    pub id: u64,
    pub title: String,
    pub kind: Kind,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NowPlaying {
    pub id: u64,
    pub title: String,
    pub kind: Kind,
    pub source: String,
    pub rep: u32,
    pub reps: u32,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PlayerState {
    pub current: Option<NowPlaying>,
    pub queue: Vec<QueueView>,
    pub position_ms: u64,
    pub duration_ms: u64,
    /// Output level 0..1 for the VU meter.
    pub level: f32,
    pub error: Option<String>,
}

enum Cmd {
    Enqueue(Item),
    Stop,
    Volume(f32),
    Chime(bool),
}

pub struct Player {
    tx: Sender<Cmd>,
    next_id: AtomicU64,
}

impl Player {
    pub fn start<F>(on_state: F, volume: f32, chime: bool) -> Self
    where
        F: Fn(PlayerState) + Send + 'static,
    {
        let (tx, rx) = unbounded();
        std::thread::Builder::new()
            .name("fabe-audio".into())
            .spawn(move || Engine::new(rx, Box::new(on_state), volume, chime).run())
            .expect("audio thread");
        Self { tx, next_id: AtomicU64::new(1) }
    }

    pub fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn enqueue(&self, item: Item) {
        let _ = self.tx.send(Cmd::Enqueue(item));
    }
    pub fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
    }
    pub fn set_volume(&self, v: f32) {
        let _ = self.tx.send(Cmd::Volume(v.clamp(0.0, 1.0)));
    }
    pub fn set_chime(&self, on: bool) {
        let _ = self.tx.send(Cmd::Chime(on));
    }
}

/// Decoded audio kept in memory, so files are never locked while playing
/// (the old app had "PermissionError" problems from exactly that).
#[derive(Clone)]
pub struct Clip {
    pub channels: u16,
    pub rate: u32,
    pub samples: Vec<f32>,
}

impl Clip {
    pub fn load(path: &Path) -> Result<Self> {
        let f = std::fs::File::open(path).with_context(|| format!("{} açılamadı", path.display()))?;
        let dec = Decoder::new(std::io::BufReader::new(f))
            .with_context(|| format!("{} çözülemedi (desteklenmeyen biçim olabilir)", path.display()))?;
        let samples: Vec<f32> = UniformSourceIterator::<_, f32>::new(dec.convert_samples::<f32>(), OUT_CHANNELS, OUT_RATE).collect();
        Ok(Self { channels: OUT_CHANNELS, rate: OUT_RATE, samples })
    }

    /// Converts samples in any format to the one the player uses.
    fn uniform(channels: u16, rate: u32, samples: Vec<f32>) -> Self {
        let src = SamplesBuffer::new(channels, rate, samples);
        let samples = UniformSourceIterator::<_, f32>::new(src, OUT_CHANNELS, OUT_RATE).collect();
        Self { channels: OUT_CHANNELS, rate: OUT_RATE, samples }
    }

    /// Adds silence before the clip and a 15 ms fade-in, so speech that starts on the very
    /// first sample (Piper does) is not clipped or clicks while the sound card wakes up.
    pub fn with_lead_in(mut self, lead: Duration) -> Self {
        let ch = self.channels as usize;
        let fade = (self.rate as usize * 15 / 1000) * ch;
        for (i, s) in self.samples.iter_mut().take(fade).enumerate() {
            *s *= (i / ch) as f32 / (fade / ch).max(1) as f32;
        }
        let pad = (lead.as_secs_f64() * self.rate as f64) as usize * ch;
        let mut out = vec![0.0; pad];
        out.extend_from_slice(&self.samples);
        self.samples = out;
        self
    }

    pub fn duration(&self) -> Duration {
        let frames = self.samples.len() as f64 / self.channels.max(1) as f64;
        Duration::from_secs_f64(frames / self.rate.max(1) as f64)
    }

    fn level_at(&self, t: Duration) -> f32 {
        let start = (t.as_secs_f64() * self.rate as f64) as usize * self.channels as usize;
        let win = &self.samples[start.min(self.samples.len())..(start + 2048).min(self.samples.len())];
        if win.is_empty() {
            return 0.0;
        }
        let rms = (win.iter().map(|s| s * s).sum::<f32>() / win.len() as f32).sqrt();
        (rms * 3.0).min(1.0)
    }

    fn with_gain(&self, gain: f32) -> Self {
        if (gain - 1.0).abs() < f32::EPSILON {
            return self.clone();
        }
        let samples = self.samples.iter().map(|s| (s * gain).clamp(-1.0, 1.0)).collect();
        Self { samples, ..*self }
    }

    /// Two-tone "ding-dong" played before an announcement.
    pub fn chime() -> Self {
        let rate = 44_100u32;
        let len = (rate as f32 * 1.5) as usize;
        let tone = |f: f32, start: f32, i: usize| {
            let t = i as f32 / rate as f32 - start;
            if t < 0.0 {
                return 0.0;
            }
            let env = (t / 0.02).min(1.0) * (-t * 3.2).exp();
            (2.0 * std::f32::consts::PI * f * t).sin() * env
        };
        let samples = (0..len).map(|i| 0.32 * (tone(659.25, 0.0, i) + tone(523.25, 0.45, i))).collect();
        Self::uniform(1, rate, samples)
    }
}

struct Engine {
    rx: Receiver<Cmd>,
    on_state: Box<dyn Fn(PlayerState) + Send>,
    queue: VecDeque<Item>,
    volume: f32,
    chime: bool,
    ducker: Ducker,
    stopped: bool,
}

impl Engine {
    fn new(rx: Receiver<Cmd>, on_state: Box<dyn Fn(PlayerState) + Send>, volume: f32, chime: bool) -> Self {
        Self { rx, on_state, queue: VecDeque::new(), volume, chime, ducker: Ducker::new(), stopped: false }
    }

    fn queue_view(&self) -> Vec<QueueView> {
        self.queue
            .iter()
            .map(|i| QueueView { id: i.id, title: i.title.clone(), kind: i.kind, source: i.source.clone() })
            .collect()
    }

    fn emit(&self, mut s: PlayerState) {
        s.queue = self.queue_view();
        (self.on_state)(s);
    }

    fn handle(&mut self, c: Cmd, sink: Option<&Sink>) {
        match c {
            Cmd::Enqueue(i) => self.queue.push_back(i),
            Cmd::Stop => {
                self.stopped = true;
                for i in self.queue.drain(..) {
                    if i.temp {
                        let _ = std::fs::remove_file(&i.file);
                    }
                }
                if let Some(s) = sink {
                    s.stop();
                }
            }
            Cmd::Volume(v) => {
                self.volume = v;
                if let Some(s) = sink {
                    s.set_volume(v);
                }
            }
            Cmd::Chime(on) => self.chime = on,
        }
    }

    fn run(mut self) {
        let mut output: Option<(OutputStream, rodio::OutputStreamHandle)> = None;
        loop {
            if self.queue.is_empty() {
                self.ducker.restore();
                self.emit(PlayerState::default());
                match self.rx.recv() {
                    Ok(c) => self.handle(c, None),
                    Err(_) => return,
                }
                continue;
            }
            let item = self.queue.pop_front().unwrap();
            self.stopped = false;

            if output.is_none() {
                output = OutputStream::try_default().ok();
            }
            let Some((_, handle)) = output.as_ref() else {
                self.emit(PlayerState { error: Some("Ses kartı bulunamadı".into()), ..Default::default() });
                continue;
            };
            let clip = match Clip::load(&item.file) {
                Ok(c) if item.kind == Kind::Tts => c.with_gain(item.gain).with_lead_in(Duration::from_millis(150)),
                Ok(c) => c.with_gain(item.gain),
                Err(e) => {
                    log::error!("{e:#}");
                    if item.temp {
                        let _ = std::fs::remove_file(&item.file);
                    }
                    self.emit(PlayerState { error: Some(format!("{}: ses dosyası çalınamadı", item.title)), ..Default::default() });
                    continue;
                }
            };
            let Ok(sink) = Sink::try_new(handle) else {
                output = None;
                continue;
            };
            sink.set_volume(self.volume);
            self.ducker.duck();

            let reps = item.reps.max(1);
            for rep in 1..=reps {
                let now = NowPlaying {
                    id: item.id,
                    title: item.title.clone(),
                    kind: item.kind,
                    source: item.source.clone(),
                    rep,
                    reps,
                };
                // Breaks (and "Dinle" previews) play without the gong.
                if rep == 1 && self.chime && item.kind != Kind::Break && !self.play(&sink, &Clip::chime(), &now) {
                    break;
                }
                if !self.play(&sink, &clip, &now) {
                    break;
                }
                if rep < reps && !self.wait(REPEAT_GAP, &sink, &now) {
                    break;
                }
            }
            if item.temp {
                let _ = std::fs::remove_file(&item.file);
            }
        }
    }

    /// Plays one clip; returns false if the user pressed DURDUR.
    fn play(&mut self, sink: &Sink, clip: &Clip, now: &NowPlaying) -> bool {
        sink.append(SamplesBuffer::new(clip.channels, clip.rate, clip.samples.clone()));
        let dur = clip.duration();
        let start = Instant::now();
        while !sink.empty() {
            while let Ok(c) = self.rx.try_recv() {
                self.handle(c, Some(sink));
            }
            if self.stopped {
                return false;
            }
            let pos = start.elapsed().min(dur);
            self.emit(PlayerState {
                current: Some(now.clone()),
                position_ms: pos.as_millis() as u64,
                duration_ms: dur.as_millis() as u64,
                level: clip.level_at(pos) * self.volume.max(0.2),
                ..Default::default()
            });
            std::thread::sleep(TICK);
        }
        true
    }

    fn wait(&mut self, d: Duration, sink: &Sink, now: &NowPlaying) -> bool {
        let end = Instant::now() + d;
        while Instant::now() < end {
            while let Ok(c) = self.rx.try_recv() {
                self.handle(c, Some(sink));
            }
            if self.stopped {
                return false;
            }
            self.emit(PlayerState { current: Some(now.clone()), ..Default::default() });
            std::thread::sleep(TICK);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chime_is_audible_and_bounded() {
        let c = Clip::chime();
        assert!((c.duration().as_secs_f32() - 1.5).abs() < 0.01);
        assert!(c.samples.iter().all(|s| s.abs() <= 1.0));
        assert!(c.level_at(Duration::from_millis(100)) > 0.1);
    }

    /// A 16-bit mono WAV like Piper writes (22.05 kHz).
    fn wav(rate: u32, samples: &[i16]) -> Vec<u8> {
        let data = samples.len() as u32 * 2;
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&rate.to_le_bytes());
        v.extend_from_slice(&(rate * 2).to_le_bytes());
        v.extend_from_slice(&2u16.to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&data.to_le_bytes());
        for s in samples {
            v.extend_from_slice(&s.to_le_bytes());
        }
        v
    }

    #[test]
    fn every_clip_reaches_the_card_in_one_format() {
        let dir = std::env::temp_dir().join(format!("fabe-player-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("piper.wav");
        // 1 s of loud tone starting on the very first sample.
        let tone: Vec<i16> = (0..22_050).map(|i| ((i as f32 * 0.1).sin() * 20_000.0) as i16).collect();
        std::fs::write(&path, wav(22_050, &tone)).unwrap();
        let c = Clip::load(&path).unwrap();
        assert_eq!((c.channels, c.rate), (OUT_CHANNELS, OUT_RATE));
        assert!((c.duration().as_secs_f32() - 1.0).abs() < 0.01);
        let chime = Clip::chime();
        assert_eq!((chime.channels, chime.rate), (OUT_CHANNELS, OUT_RATE));

        let led = c.clone().with_lead_in(Duration::from_millis(150));
        assert!((led.duration().as_secs_f32() - 1.15).abs() < 0.01);
        let pad = (0.15 * OUT_RATE as f32) as usize * OUT_CHANNELS as usize;
        assert!(led.samples[..pad].iter().all(|s| *s == 0.0));
        // Fades in instead of starting at full level.
        assert!(led.samples[pad..pad + 20].iter().all(|s| s.abs() < 0.05));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn gain_clips_instead_of_wrapping() {
        let c = Clip { channels: 1, rate: 8000, samples: vec![0.5, -0.5] }.with_gain(4.0);
        assert_eq!(c.samples, vec![1.0, -1.0]);
    }
}
