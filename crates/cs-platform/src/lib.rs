//! Platform seams: time source and configuration storage shared by native and wasm32 builds.

pub mod xdg;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic time source, in seconds.
pub trait Clock {
    fn now(&self) -> f64;
}

/// Clock driven by hand; starts at `0.0` and moves only through [`ManualClock::advance`].
#[derive(Debug, Default)]
pub struct ManualClock {
    /// Current time as `f64` bits.
    bits: AtomicU64,
}

impl ManualClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// Move the clock forward by `dt` seconds.
    pub fn advance(&self, dt: f64) {
        let mut current = self.bits.load(Ordering::Relaxed);
        loop {
            let next = (f64::from_bits(current) + dt).to_bits();
            match self.bits.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }
}

impl Clock for ManualClock {
    fn now(&self) -> f64 {
        f64::from_bits(self.bits.load(Ordering::Relaxed))
    }
}

/// Wall clock measuring seconds elapsed since its creation.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub struct NativeClock {
    start: std::time::Instant,
}

#[cfg(not(target_arch = "wasm32"))]
impl NativeClock {
    pub fn new() -> Self {
        Self {
            start: std::time::Instant::now(),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for NativeClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Clock for NativeClock {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }
}

/// High-resolution browser clock backed by `window.performance.now()`.
#[cfg(target_arch = "wasm32")]
#[derive(Debug, Clone)]
pub struct WebClock {
    start_ms: f64,
}

#[cfg(target_arch = "wasm32")]
impl WebClock {
    pub fn new() -> Self {
        Self {
            start_ms: perf_now_ms(),
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl Default for WebClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_arch = "wasm32")]
impl Clock for WebClock {
    fn now(&self) -> f64 {
        ((perf_now_ms() - self.start_ms) / 1000.0).max(0.0)
    }
}

#[cfg(target_arch = "wasm32")]
fn perf_now_ms() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map_or(0.0, |p| p.now())
}

/// Key-value persistence for client configuration (`config.cfg`, user settings).
pub trait ConfigStorage {
    fn load(&self, key: &str) -> Option<String>;
    fn save(&mut self, key: &str, value: &str) -> Result<(), String>;
    fn remove(&mut self, key: &str) -> Result<(), String>;
}

/// Returns the [`ConfigStorage`] key for a cached `.nav` mesh (`nav_cache_<map>_<crc>.nav`).
pub fn nav_cache_storage_key(map_name: &str, bsp_crc32: u32) -> Option<String> {
    xdg::nav_cache_filename(map_name, bsp_crc32).map(|file| format!("nav_cache_{file}"))
}

/// Loads cached `.nav` bytes from a [`ConfigStorage`] backend (used on `wasm32` / tests).
pub fn load_nav_cache_storage(
    storage: &dyn ConfigStorage,
    map_name: &str,
    bsp_crc32: u32,
) -> Option<Vec<u8>> {
    let key = nav_cache_storage_key(map_name, bsp_crc32)?;
    let hex = storage.load(&key)?;
    decode_hex(hex.trim())
}

/// Saves `.nav` bytes into a [`ConfigStorage`] backend keyed by `(map_name, bsp_crc32)`.
pub fn save_nav_cache_storage(
    storage: &mut dyn ConfigStorage,
    map_name: &str,
    bsp_crc32: u32,
    bytes: &[u8],
) -> Result<(), String> {
    let key = nav_cache_storage_key(map_name, bsp_crc32)
        .ok_or_else(|| format!("invalid nav_cache map name `{map_name}`"))?;
    storage.save(&key, &encode_hex(bytes))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[usize::from(b >> 4)] as char);
        out.push(HEX[usize::from(b & 0x0f)] as char);
    }
    out
}

fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    let raw = hex.as_bytes();
    let (pairs, remainder) = raw.as_chunks::<2>();
    if !remainder.is_empty() {
        return None;
    }
    for pair in pairs {
        let hi = hex_nibble(pair[0])?;
        let lo = hex_nibble(pair[1])?;
        out.push((hi << 4) | lo);
    }
    Some(out)
}

const fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Validates a storage key so native file backends cannot escape their directory.
fn valid_storage_key(key: &str) -> bool {
    !key.is_empty() && !key.contains(['/', '\\', '\0']) && !key.contains("..")
}

/// In-memory config storage for tests and ephemeral sessions.
#[derive(Debug, Default, Clone)]
pub struct MemoryStorage {
    entries: HashMap<String, String>,
}

impl ConfigStorage for MemoryStorage {
    fn load(&self, key: &str) -> Option<String> {
        self.entries.get(key).cloned()
    }

    fn save(&mut self, key: &str, value: &str) -> Result<(), String> {
        if !valid_storage_key(key) {
            return Err(format!("invalid config storage key `{key}`"));
        }
        self.entries.insert(key.to_string(), value.to_string());
        Ok(())
    }

    fn remove(&mut self, key: &str) -> Result<(), String> {
        self.entries.remove(key);
        Ok(())
    }
}

/// Native filesystem config storage rooted at a dedicated directory.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
pub struct NativeFileStorage {
    root: std::path::PathBuf,
}

#[cfg(not(target_arch = "wasm32"))]
impl NativeFileStorage {
    pub fn new(root: impl Into<std::path::PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path_for(&self, key: &str) -> Result<std::path::PathBuf, String> {
        if !valid_storage_key(key) {
            return Err(format!("invalid config storage key `{key}`"));
        }
        Ok(self.root.join(key))
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl ConfigStorage for NativeFileStorage {
    fn load(&self, key: &str) -> Option<String> {
        let path = self.path_for(key).ok()?;
        std::fs::read_to_string(path).ok()
    }

    fn save(&mut self, key: &str, value: &str) -> Result<(), String> {
        let path = self.path_for(key)?;
        std::fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
        std::fs::write(path, value).map_err(|e| e.to_string())
    }

    fn remove(&mut self, key: &str) -> Result<(), String> {
        let path = self.path_for(key)?;
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Browser `window.localStorage` config persistence prefixed with `cs16rs:`.
#[cfg(target_arch = "wasm32")]
#[derive(Debug, Default, Clone)]
pub struct WebLocalStorage;

#[cfg(target_arch = "wasm32")]
impl WebLocalStorage {
    fn storage() -> Option<web_sys::Storage> {
        web_sys::window().and_then(|w| w.local_storage().ok().flatten())
    }

    fn prefixed(key: &str) -> Result<String, String> {
        if !valid_storage_key(key) {
            return Err(format!("invalid config storage key `{key}`"));
        }
        Ok(format!("cs16rs:{key}"))
    }
}

#[cfg(target_arch = "wasm32")]
impl ConfigStorage for WebLocalStorage {
    fn load(&self, key: &str) -> Option<String> {
        let full = Self::prefixed(key).ok()?;
        Self::storage()?.get_item(&full).ok().flatten()
    }

    fn save(&mut self, key: &str, value: &str) -> Result<(), String> {
        let full = Self::prefixed(key)?;
        let st = Self::storage().ok_or_else(|| "localStorage unavailable".to_string())?;
        st.set_item(&full, value).map_err(|e| format!("{e:?}"))
    }

    fn remove(&mut self, key: &str) -> Result<(), String> {
        let full = Self::prefixed(key)?;
        let st = Self::storage().ok_or_else(|| "localStorage unavailable".to_string())?;
        st.remove_item(&full).map_err(|e| format!("{e:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_clock_advances() {
        let c = ManualClock::new();
        c.advance(0.5);
        c.advance(0.25);
        assert_eq!(c.now(), 0.75);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_clock_is_monotonic() {
        let c = NativeClock::new();
        let first = c.now();
        let second = c.now();
        assert!(second >= first);
    }

    #[test]
    fn memory_and_native_storage_roundtrip_and_reject_traversal() {
        let mut mem = MemoryStorage::default();
        assert!(mem.load("config.cfg").is_none());
        mem.save("config.cfg", "sensitivity \"2.5\"\n").unwrap();
        assert_eq!(mem.load("config.cfg").as_deref(), Some("sensitivity \"2.5\"\n"));
        assert!(mem.save("../escape.cfg", "bad").is_err());
        mem.remove("config.cfg").unwrap();
        assert!(mem.load("config.cfg").is_none());

        #[cfg(not(target_arch = "wasm32"))]
        {
            let dir = std::env::temp_dir().join(format!("cs16-platform-test-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let mut fs_store = NativeFileStorage::new(&dir);
            fs_store.save("config.cfg", "name \"WebPlayer\"\n").unwrap();
            assert_eq!(
                fs_store.load("config.cfg").as_deref(),
                Some("name \"WebPlayer\"\n")
            );
            assert!(fs_store.save("../bad", "x").is_err());
            fs_store.remove("config.cfg").unwrap();
            assert!(fs_store.load("config.cfg").is_none());
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn nav_cache_storage_roundtrip() {
        let mut mem = MemoryStorage::default();
        let payload = b"\xfe\xed\xfa\xce\x05\x00\x00\x00hello-nav";
        save_nav_cache_storage(&mut mem, "fy_pool_day", 0x12ab_34cd, payload).unwrap();
        assert_eq!(
            load_nav_cache_storage(&mem, "fy_pool_day", 0x12ab_34cd).as_deref(),
            Some(payload.as_slice())
        );
        assert!(load_nav_cache_storage(&mem, "fy_pool_day", 0x9999_9999).is_none());
        assert!(save_nav_cache_storage(&mut mem, "../bad", 1, payload).is_err());
    }
}
