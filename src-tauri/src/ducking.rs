//! Lowers other programs (Spotify, browsers, media players) while an announcement plays,
//! then brings them back smoothly. Windows only; elsewhere it does nothing.

/// Level other programs are lowered to during an announcement (0.0 - 1.0).
#[allow(dead_code)]
pub const DUCK_LEVEL: f32 = 0.10;

#[cfg(windows)]
mod imp {
    use super::DUCK_LEVEL;
    use std::time::Duration;
    use windows::core::{Interface, Result};
    use windows::Win32::Media::Audio::*;
    use windows::Win32::System::Com::*;

    struct Session {
        key: String,
        vol: ISimpleAudioVolume,
    }

    pub struct Ducker {
        saved: Vec<(String, f32)>,
    }

    fn sessions() -> Result<Vec<Session>> {
        let own = std::process::id();
        let mut out = Vec::new();
        unsafe {
            let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let dev = en.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let mgr: IAudioSessionManager2 = dev.Activate(CLSCTX_ALL, None)?;
            let list = mgr.GetSessionEnumerator()?;
            for i in 0..list.GetCount()? {
                let Ok(ctl) = list.GetSession(i) else { continue };
                let Ok(ctl2) = ctl.cast::<IAudioSessionControl2>() else { continue };
                let pid = ctl2.GetProcessId().unwrap_or(0);
                // pid 0 = Windows system sounds; own pid = us.
                if pid == 0 || pid == own {
                    continue;
                }
                let key = match ctl2.GetSessionInstanceIdentifier() {
                    Ok(p) => {
                        let s = p.to_string().unwrap_or_default();
                        CoTaskMemFree(Some(p.0 as _));
                        s
                    }
                    Err(_) => format!("pid:{pid}"),
                };
                if let Ok(vol) = ctl.cast::<ISimpleAudioVolume>() {
                    out.push(Session { key, vol });
                }
            }
        }
        Ok(out)
    }

    fn fade(targets: &[(ISimpleAudioVolume, f32, f32)], ms: u64) {
        const STEPS: u64 = 10;
        for step in 1..=STEPS {
            let k = step as f32 / STEPS as f32;
            for (v, from, to) in targets {
                unsafe {
                    let _ = v.SetMasterVolume(from + (to - from) * k, std::ptr::null());
                }
            }
            std::thread::sleep(Duration::from_millis(ms / STEPS));
        }
    }

    impl Ducker {
        pub fn new() -> Self {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            Self { saved: Vec::new() }
        }

        pub fn duck(&mut self) {
            if !self.saved.is_empty() {
                return;
            }
            let Ok(list) = sessions() else { return };
            let mut targets = Vec::new();
            for s in list {
                let cur = unsafe { s.vol.GetMasterVolume() }.unwrap_or(1.0);
                self.saved.push((s.key, cur));
                if cur > DUCK_LEVEL {
                    targets.push((s.vol, cur, DUCK_LEVEL));
                }
            }
            fade(&targets, 250);
        }

        pub fn restore(&mut self) {
            if self.saved.is_empty() {
                return;
            }
            let saved = std::mem::take(&mut self.saved);
            let Ok(list) = sessions() else { return };
            let mut targets = Vec::new();
            for s in list {
                if let Some((_, level)) = saved.iter().find(|(k, _)| *k == s.key) {
                    let cur = unsafe { s.vol.GetMasterVolume() }.unwrap_or(*level);
                    targets.push((s.vol, cur, *level));
                }
            }
            fade(&targets, 600);
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub struct Ducker;
    impl Ducker {
        pub fn new() -> Self {
            Ducker
        }
        pub fn duck(&mut self) {}
        pub fn restore(&mut self) {}
    }
}

pub use imp::Ducker;
