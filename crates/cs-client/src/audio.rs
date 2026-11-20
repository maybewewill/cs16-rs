//! GoldSrc WAV sound playback using `rodio 0.23` with on-demand VFS byte caching and 3D distance attenuation.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;

use cs_vfs::Vfs;
use glam::Vec3;
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player};
use tracing::{info, warn};

pub struct AudioEngine {
    sink: Option<MixerDeviceSink>,
    cache: HashMap<String, Option<Arc<[u8]>>>,
    menu_music: Option<Player>,
    menu_music_started: bool,
}

impl AudioEngine {
    pub fn new() -> Self {
        let sink = match DeviceSinkBuilder::from_default_device().and_then(|b| b.open_stream()) {
            Ok(mut s) => {
                s.log_on_drop(false);
                info!("Initialized audio output stream");
                Some(s)
            }
            Err(err) => {
                warn!(%err, "Audio output unavailable; running muted");
                None
            }
        };
        Self {
            sink,
            cache: HashMap::new(),
            menu_music: None,
            menu_music_started: false,
        }
    }

    pub fn play_ui_click(&mut self, vfs: Option<&Vfs>, master_volume: f32) {
        self.play_2d(vfs, "UI/buttonclick.wav", 0.55 * master_volume);
    }

    /// Start subtle CS 1.6 Main Menu MP3 music (`media/gamestartup.mp3` or `media/Half-Life01.mp3`).
    pub fn sync_menu_music(&mut self, vfs: Option<&Vfs>, should_play: bool, mp3_vol: f32) {
        if !should_play || mp3_vol <= 0.01 {
            if let Some(p) = self.menu_music.take() {
                p.stop();
            }
            self.menu_music_started = false;
            return;
        }
        if let Some(ref p) = self.menu_music {
            p.set_volume(mp3_vol.clamp(0.0, 1.0));
            return;
        }
        if self.menu_music_started {
            return;
        }
        self.menu_music_started = true;
        let Some(sink) = &self.sink else {
            return;
        };
        let Some(vfs) = vfs else {
            return;
        };
        for path in ["media/gamestartup.mp3", "media/Half-Life01.mp3"] {
            if let Ok(bytes) = vfs.read_bytes(path)
                && let Ok(source) = Decoder::new(Cursor::new(SharedBytes(Arc::from(bytes))))
            {
                let player = Player::connect_new(sink.mixer());
                player.set_volume(mp3_vol.clamp(0.0, 1.0));
                player.append(source);
                self.menu_music = Some(player);
                return;
            }
        }
    }

    /// Play a non-positional sound (e.g. `"UI/buttonclick.wav"` or `"radio/ctwin.wav"`).
    pub fn play_2d(&mut self, vfs: Option<&Vfs>, sound_path: &str, volume: f32) {
        if volume <= 0.005 || sound_path.is_empty() {
            return;
        }
        let Some(bytes) = self.load_cached(vfs, sound_path) else {
            return;
        };
        let Some(sink) = &self.sink else {
            return;
        };
        if let Ok(source) = Decoder::new(Cursor::new(SharedBytes(bytes))) {
            let player = Player::connect_new(sink.mixer());
            player.set_volume(volume.clamp(0.0, 1.5));
            player.append(source);
            player.detach();
        }
    }

    /// Play a 3D world sound attenuated by distance from the listener.
    pub fn play_3d(
        &mut self,
        vfs: Option<&Vfs>,
        sound_path: &str,
        pos: Vec3,
        listener_pos: Vec3,
        base_volume: f32,
        master_volume: f32,
    ) {
        let dist = pos.distance(listener_pos);
        let max_dist = 1800.0f32;
        if dist >= max_dist {
            return;
        }
        let atten = (1.0 - dist / max_dist).powf(1.35);
        let vol = base_volume * master_volume * atten;
        self.play_2d(vfs, sound_path, vol);
    }

    fn load_cached(&mut self, vfs: Option<&Vfs>, rel: &str) -> Option<Arc<[u8]>> {
        let normalized = if rel.to_ascii_lowercase().starts_with("sound/") {
            rel.to_ascii_lowercase()
        } else {
            format!("sound/{}", rel.to_ascii_lowercase())
        };

        if let Some(entry) = self.cache.get(&normalized) {
            return entry.clone();
        }

        let loaded = vfs
            .and_then(|v| v.read_bytes(&normalized).ok())
            .map(Arc::<[u8]>::from);
        self.cache.insert(normalized, loaded.clone());
        loaded
    }
}

/// Zero-copy `AsRef<[u8]>` wrapper around `Arc<[u8]>` for `std::io::Cursor` + `rodio::Decoder`.
struct SharedBytes(Arc<[u8]>);

impl AsRef<[u8]> for SharedBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}
