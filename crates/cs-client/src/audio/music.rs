//! GoldSrc background music controller: `svc_cdtrack`, `mp3` / `cd` commands, menu music,
//! `mp3volume`, `bgmvolume`, and `MP3FadeTime` smooth crossfade/fade-out.

#[derive(Debug, Clone, PartialEq)]
pub enum MusicCommand {
    Play { path: String, looping: bool, volume: f32 },
    SetVolume(f32),
    Stop,
}

#[derive(Debug, Clone, PartialEq)]
enum MusicSourceKind {
    None,
    MenuFallback,
    Explicit,
    CdTrack,
}

#[derive(Debug, Clone)]
pub struct MusicController {
    source_kind: MusicSourceKind,
    current_path: Option<String>,
    looping: bool,
    master_mp3_volume: f32,
    bgm_enabled: bool,
    fade_duration: f32,
    fade_remaining: f32,
    fading_out: bool,
    pending_cmds: Vec<MusicCommand>,
    last_sent_vol: f32,
}

impl Default for MusicController {
    fn default() -> Self {
        Self::new()
    }
}

impl MusicController {
    pub fn new() -> Self {
        Self {
            source_kind: MusicSourceKind::None,
            current_path: None,
            looping: false,
            master_mp3_volume: 0.5,
            bgm_enabled: true,
            fade_duration: 2.0,
            fade_remaining: 0.0,
            fading_out: false,
            pending_cmds: Vec::new(),
            last_sent_vol: -1.0,
        }
    }

    pub fn set_params(&mut self, mp3_volume: f32, bgm_enabled: bool, fade_time: f32) {
        self.master_mp3_volume = if mp3_volume.is_finite() {
            mp3_volume.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.bgm_enabled = bgm_enabled;
        self.fade_duration = if fade_time.is_finite() {
            fade_time.clamp(0.0, 30.0)
        } else {
            0.0
        };
    }

    pub fn current_path(&self) -> Option<&str> {
        self.current_path.as_deref()
    }

    pub fn effective_volume(&self) -> f32 {
        if !self.bgm_enabled || self.current_path.is_none() {
            return 0.0;
        }
        let fade_mul = if self.fading_out && self.fade_duration > 0.0 {
            (self.fade_remaining / self.fade_duration).clamp(0.0, 1.0)
        } else {
            1.0
        };
        (self.master_mp3_volume * fade_mul).clamp(0.0, 1.0)
    }

    /// Map GoldSrc CD track number (`2..=28`) to the retail MP3 path under `media/`.
    pub fn cd_track_to_path(track: i32) -> Option<String> {
        if track <= 1 {
            return None;
        }
        if (2..=18).contains(&track) {
            return Some(format!("media/Half-Life{:02}.mp3", track - 1));
        }
        if (19..=23).contains(&track) {
            return Some(format!("media/Prospero{:02}.mp3", track - 18));
        }
        let suspense = match track {
            24 => "media/Suspense01.mp3",
            25 => "media/Suspense02.mp3",
            26 => "media/Suspense03.mp3",
            27 => "media/Suspense05.mp3",
            28 => "media/Suspense07.mp3",
            _ => return None,
        };
        Some(suspense.to_string())
    }

    pub fn handle_svc_cdtrack(&mut self, track: i8, loop_track: i8) {
        if track <= 0 {
            self.stop_with_fade();
            return;
        }
        if let Some(path) = Self::cd_track_to_path(i32::from(track)) {
            let looping = loop_track > 0 || loop_track == -1;
            self.play_internal(MusicSourceKind::CdTrack, path, looping);
        }
    }

    pub fn handle_mp3_command(&mut self, args: &[String]) {
        let Some(sub) = args.first().map(|s| s.to_ascii_lowercase()) else {
            return;
        };
        match sub.as_str() {
            "play" | "loop" => {
                if let Some(raw_path) = args.get(1) {
                    let norm = normalize_mp3_path(raw_path);
                    self.play_internal(MusicSourceKind::Explicit, norm, sub == "loop");
                }
            }
            "stop" => self.stop_with_fade(),
            _ => {}
        }
    }

    pub fn handle_cd_command(&mut self, args: &[String]) {
        let Some(sub) = args.first().map(|s| s.to_ascii_lowercase()) else {
            return;
        };
        match sub.as_str() {
            "play" | "loop" => {
                if let Some(track) = args.get(1).and_then(|s| s.parse::<i32>().ok())
                    && let Some(path) = Self::cd_track_to_path(track)
                {
                    self.play_internal(MusicSourceKind::CdTrack, path, sub == "loop");
                }
            }
            "stop" | "off" => self.stop_with_fade(),
            _ => {}
        }
    }

    /// Synchronize menu startup music when no explicit/server track is active.
    pub fn sync_menu_music(&mut self, want_menu_music: bool, menu_path: Option<&str>) {
        if matches!(
            self.source_kind,
            MusicSourceKind::Explicit | MusicSourceKind::CdTrack
        ) {
            return;
        }
        if !want_menu_music || self.master_mp3_volume <= 0.01 || !self.bgm_enabled {
            if self.source_kind == MusicSourceKind::MenuFallback {
                self.stop_with_fade();
            }
            return;
        }
        if self.source_kind == MusicSourceKind::MenuFallback && !self.fading_out {
            return;
        }
        if let Some(path) = menu_path {
            self.play_internal(MusicSourceKind::MenuFallback, path.to_string(), false);
        }
    }

    pub fn stop_with_fade(&mut self) {
        if self.current_path.is_none() {
            return;
        }
        if self.fade_duration <= 0.02 {
            self.stop_immediate();
            return;
        }
        if !self.fading_out {
            self.fading_out = true;
            self.fade_remaining = self.fade_duration;
        }
    }

    pub fn stop_immediate(&mut self) {
        if self.current_path.take().is_some() {
            self.pending_cmds.push(MusicCommand::Stop);
        }
        self.source_kind = MusicSourceKind::None;
        self.looping = false;
        self.fading_out = false;
        self.fade_remaining = 0.0;
        self.last_sent_vol = -1.0;
    }

    fn play_internal(&mut self, kind: MusicSourceKind, path: String, looping: bool) {
        self.source_kind = kind;
        self.current_path = Some(path.clone());
        self.looping = looping;
        self.fading_out = false;
        self.fade_remaining = 0.0;
        let vol = self.effective_volume();
        self.last_sent_vol = vol;
        self.pending_cmds.push(MusicCommand::Play {
            path,
            looping,
            volume: vol,
        });
    }

    pub fn step(&mut self, dt_sec: f32) -> Vec<MusicCommand> {
        if self.fading_out && self.current_path.is_some() {
            self.fade_remaining = (self.fade_remaining - dt_sec.max(0.0)).max(0.0);
            if self.fade_remaining <= 0.0 {
                self.stop_immediate();
            }
        }
        if self.current_path.is_some() {
            let vol = self.effective_volume();
            if (vol - self.last_sent_vol).abs() > 0.005 {
                self.last_sent_vol = vol;
                self.pending_cmds.push(MusicCommand::SetVolume(vol));
            }
        }
        std::mem::take(&mut self.pending_cmds)
    }
}

fn normalize_mp3_path(raw: &str) -> String {
    let clean = raw.trim().replace('\\', "/");
    let stripped = clean.trim_start_matches('/');
    if stripped.to_ascii_lowercase().starts_with("media/")
        || stripped.to_ascii_lowercase().starts_with("sound/")
    {
        stripped.to_string()
    } else {
        format!("media/{stripped}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cdtrack_mp3_commands_and_mp3_fade_time_work() {
        assert_eq!(
            MusicController::cd_track_to_path(2).as_deref(),
            Some("media/Half-Life01.mp3")
        );
        assert_eq!(
            MusicController::cd_track_to_path(24).as_deref(),
            Some("media/Suspense01.mp3")
        );

        let mut mc = MusicController::new();
        mc.set_params(0.8, true, 1.0);
        mc.handle_mp3_command(&["loop".into(), "Half-Life01.mp3".into()]);
        let cmds = mc.step(0.01);
        assert!(matches!(
            cmds.first(),
            Some(MusicCommand::Play { path, looping: true, .. }) if path == "media/Half-Life01.mp3"
        ));

        mc.handle_mp3_command(&["stop".into()]);
        let mid = mc.step(0.5);
        assert!(matches!(mid.last(), Some(MusicCommand::SetVolume(v)) if *v < 0.45 && *v > 0.35));
        let end = mc.step(0.6);
        assert!(end.contains(&MusicCommand::Stop));
        assert!(mc.current_path().is_none());
    }
}
