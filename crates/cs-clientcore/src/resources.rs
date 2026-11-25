//! The resource list of a map change: which files the server expects the client to have, which
//! of them are missing, the downloads that fetch them, and the file-consistency answer.
//!
//! Ported from Xash3D FWGS `engine/client/cl_parse.c` (`CL_ParseResourceList`,
//! `CL_ParseConsistencyInfo`, `CL_SendConsistencyInfo`, `CL_EstimateNeededResources`,
//! `CL_BatchResourceRequest`, `CL_ParseResLocation`), `engine/client/cl_custom.c`
//! (`CL_ResourcePath`, `CL_CheckFile`) and `engine/client/cl_main.c` (`CL_ProcessFile`), and
//! `engine/common/net_chan.c` (`Netchan_CopyFileFragments`, the GoldSrc file header), all read
//! 2026-11-23.
//!
//! A model's consistency check type is byte 0 of the 32 reserved bytes of its list entry,
//! munged with the spawn count; every other item is checked as `exactfile` (an md5).
//! `COM_IsSafeFileToDownload` is not in the fetched sources; [`is_safe_to_download`] is a
//! whitelist of our own (see there).

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

use cs_proto48::bitbuf::MsgWriter;
use cs_proto48::compress::decompress_stream;
use cs_proto48::crypto::{com_munge, com_unmunge};
use cs_proto48::messages::ResourceEntry;

/// Resource kinds (`resourcetype_t`).
pub mod res_type {
    pub const SOUND: u8 = 0;
    pub const SKIN: u8 = 1;
    pub const MODEL: u8 = 2;
    pub const DECAL: u8 = 3;
    pub const GENERIC: u8 = 4;
    pub const EVENTSCRIPT: u8 = 5;
    pub const WORLD: u8 = 6;
}

/// The client cannot play without this resource (`RES_FATALIFMISSING`).
pub const RES_FATALIFMISSING: u8 = 1 << 0;
/// The entry carries an md5 (`RES_CUSTOM`).
pub const RES_CUSTOM: u8 = 1 << 2;

/// Directory sounds live in (`DEFAULT_SOUNDPATH`).
pub const SOUND_PATH: &str = "sound/";
/// Bits of a resource index in `svc_resourcelist` (`SV_SendConsistencyList`, 10 bits).
const CONSISTENCY_LIST_INDEX_BITS: u32 = 10;
/// Bits of a resource index in `clc_fileconsistency` (`SV_ParseConsistencyResponse`, 12 bits).
pub const CONSISTENCY_RESPONSE_INDEX_BITS: u32 = 12;
/// A resource the server could not find reports this size (the 24-bit field read as -1).
/// Download locations kept from `svc_resourcelocation`.
pub const MAX_DOWNLOAD_LOCATIONS: usize = 16;
/// Largest file the client accepts (`MAX_NETCHAN_DECOMPRESSED_SIZE`, Xash `net_chan.c`).
pub const MAX_FILE_SIZE: u32 = 64 * 1024 * 1024;
const MISSING_ON_SERVER: u32 = 0x00FF_FFFF;

/// The client's view of its game files. The shell supplies a real one; [`AllFilesPresent`]
/// stands in where there is nothing to check (tests, the headless probe).
pub trait ResourceFiles: Send {
    /// Whether `path` (relative to the game directory, `/` separated) exists.
    fn exists(&self, path: &str) -> bool;
    /// The md5 of the file, `None` when it does not exist.
    fn md5(&self, path: &str) -> Option<[u8; 16]>;
    /// Bounding box of a studio model, `None` when the file is missing.
    fn studio_bounds(&self, _path: &str) -> Option<([f32; 3], [f32; 3])> {
        None
    }
    /// Stores a downloaded file. The implementation must write into a download cache of its
    /// own (never the game directory proper) and must not overwrite an existing game file:
    /// the file came from a server. The core calls it only for a file it requested with
    /// `dlfile`, under a name that passed [`is_safe_to_download`]. Returns `false` when the
    /// file was not kept. When the shell checks whether a file exists (here, and before it
    /// stores) it must resolve Windows 8.3 short names (`PROGRA~1`) and case, so that the
    /// "no overwrite" rule cannot be dodged by a name that reaches an existing file another way.
    /// A root-level `.wad` is stored like any other file: into the cache, never over a wad of
    /// the game.
    fn store(&mut self, _path: &str, _data: &[u8]) -> bool {
        false
    }
}

/// Claims every file exists; answers no md5.
#[derive(Debug, Default, Clone, Copy)]
pub struct AllFilesPresent;

impl ResourceFiles for AllFilesPresent {
    fn exists(&self, _path: &str) -> bool {
        true
    }
    fn md5(&self, _path: &str) -> Option<[u8; 16]> {
        None
    }
}

/// `consistency_t::check_type` (`force_type_t`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckType {
    ExactFile,
    ModelSameBounds,
    ModelSpecifyBounds,
    ModelSpecifyBoundsIfAvail,
}

impl CheckType {
    pub fn from_wire(v: u8) -> Option<Self> {
        Some(match v {
            0 => Self::ExactFile,
            1 => Self::ModelSameBounds,
            2 => Self::ModelSpecifyBounds,
            3 => Self::ModelSpecifyBoundsIfAvail,
            _ => return None,
        })
    }
}

/// A file the server wants checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsistencyItem {
    pub filename: String,
    pub is_sound: bool,
    /// Position of the resource in the list, echoed in the answer.
    pub orig_index: u16,
    pub check: CheckType,
}

/// A resource of the list not yet settled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeededResource {
    pub entry: ResourceEntry,
    /// The client lacks the file (`RES_WASMISSING`).
    pub was_missing: bool,
    /// A `dlfile` request went out for it (`RES_REQUESTED`).
    pub requested: bool,
}

/// A file taken from a file-stream transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedFile {
    pub name: String,
    pub data: Vec<u8>,
}

/// What [`ResourceManager::batch_request`] decided.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BatchOutcome {
    /// Paths to request from the server with `dlfile <path>`.
    pub dlfiles: Vec<String>,
    /// Every resource is settled: nothing waits for a download.
    pub done: bool,
    /// A resource flagged `RES_FATALIFMISSING` is missing and cannot be fetched.
    pub fatal: Option<String>,
    /// Resources left unfetched because their path is unsafe or downloads are off, as console
    /// lines (never silent).
    pub skipped: Vec<String>,
}

/// Completed HTTP FastDL transfer returned by [`HttpFetcher::poll_completed`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpFetchResult {
    pub path: String,
    pub is_bz2: bool,
    pub data: Result<Vec<u8>, String>,
}

/// Platform-neutral asynchronous HTTP client hook for `sv_downloadurl` FastDL.
pub trait HttpFetcher: Send {
    fn start_fetch(&mut self, base_url: &str, rel_path: &str, is_bz2: bool) -> bool;
    fn poll_completed(&mut self) -> Vec<HttpFetchResult>;
    fn has_pending(&self) -> bool;
}

/// Resource state of the current sign-on.
#[derive(Debug, Clone, Default)]
pub struct ResourceManager {
    needed: Vec<NeededResource>,
    on_hand: Vec<ResourceEntry>,
    consistency: Vec<ConsistencyItem>,
    need_consistency_response: bool,
    download_locations: Vec<String>,
    total_to_transfer: u64,
    fatal_failure: Option<String>,
    /// `key_lower -> (rel_path, location_index, is_bz2)`.
    http_inflight: BTreeMap<String, (String, usize, bool)>,
    http_exhausted: BTreeSet<String>,
    http_needs_fallback: bool,
}

impl ResourceManager {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Takes the list of a `svc_resourcelist` (`CL_ParseResourceList`) and its consistency
    /// positions. Fails on a position past the end of the list (the original raises
    /// "malformed consistency info packet").
    pub fn begin(
        &mut self,
        resources: &[ResourceEntry],
        consistency_indices: &[u16],
        spawn_count: u32,
    ) -> Result<(), String> {
        self.needed = resources
            .iter()
            .map(|entry| NeededResource {
                entry: entry.clone(),
                was_missing: false,
                requested: false,
            })
            .collect();
        self.on_hand.clear();
        self.consistency.clear();
        self.need_consistency_response = !consistency_indices.is_empty();
        for &index in consistency_indices {
            if index >= 1 << CONSISTENCY_LIST_INDEX_BITS {
                return Err(format!("malformed consistency info packet (index {index})"));
            }
            let entry = resources.get(usize::from(index)).ok_or_else(|| {
                format!(
                    "malformed consistency info packet (index {index}, {} resources)",
                    resources.len()
                )
            })?;
            // A model's check type is byte 0 of its reserved bytes, munged with the spawn count
            // (`CL_ParseConsistencyInfo`, `cl_parse.c`): an all-zero block means exactfile.
            let mut check = CheckType::ExactFile;
            if entry.res_type == res_type::MODEL
                && let Some(mut reserved) = entry.reserved.filter(|r| r.iter().any(|&b| b != 0))
            {
                com_unmunge(&mut reserved, spawn_count as i32);
                check = CheckType::from_wire(reserved[0]).ok_or_else(|| {
                    format!("unknown consistency type {} for {}", reserved[0], entry.filename)
                })?;
            }
            self.consistency.push(ConsistencyItem {
                filename: entry.filename.clone(),
                is_sound: entry.res_type == res_type::SOUND,
                orig_index: index,
                check,
            });
        }
        Ok(())
    }

    pub fn needed(&self) -> &[NeededResource] {
        &self.needed
    }

    pub fn on_hand(&self) -> &[ResourceEntry] {
        &self.on_hand
    }

    pub fn consistency(&self) -> &[ConsistencyItem] {
        &self.consistency
    }

    /// Bytes the missing resources add up to (`cls.dl.nTotalToTransfer`).
    pub fn total_to_transfer(&self) -> u64 {
        self.total_to_transfer
    }

    /// Download locations the server announced with `svc_resourcelocation`.
    pub fn download_locations(&self) -> &[String] {
        &self.download_locations
    }

    /// `CL_ParseResLocation`: the text holds whitespace-separated locations (the original
    /// ignores text over 256 bytes).
    /// Fails when the server announces more than [`MAX_DOWNLOAD_LOCATIONS`] in all.
    pub fn add_download_locations(&mut self, text: &str) -> Result<(), String> {
        if text.len() > 256 {
            return Ok(());
        }
        for location in text.split_whitespace() {
            if self.download_locations.len() >= MAX_DOWNLOAD_LOCATIONS {
                return Err("too many download locations".to_string());
            }
            self.download_locations.push(location.to_string());
        }
        Ok(())
    }

    /// `CL_EstimateNeededResources`: marks what the files lack. Returns the byte total.
    pub fn estimate_missing(&mut self, files: &dyn ResourceFiles) -> u64 {
        let mut total = 0u64;
        for r in &mut self.needed {
            match r.entry.res_type {
                res_type::SOUND
                | res_type::MODEL
                | res_type::SKIN
                | res_type::GENERIC
                | res_type::EVENTSCRIPT => {
                    // Inline models and sentences are not files.
                    if r.entry.filename.starts_with(['*', '!', '#']) {
                        continue;
                    }
                    if !files.exists(&resource_path(&r.entry)) {
                        r.was_missing = true;
                        total += u64::from(r.entry.download_size & MISSING_ON_SERVER);
                    }
                }
                // Customizations: only those sent with an md5 need fetching; the client has no
                // HPAK store yet, so they are not asked for (see `batch_request`).
                _ => {}
            }
        }
        self.total_to_transfer = total;
        total
    }

    /// `CL_BatchResourceRequest` for the resources the server's list names: settles what is
    /// present, asks for the missing files that may be fetched, and reports the rest.
    /// `allow_download` is `cl_allowdownload`.
    pub fn batch_request(&mut self, allow_download: bool) -> BatchOutcome {
        let mut out = BatchOutcome::default();
        let mut still_needed = Vec::new();
        for mut r in std::mem::take(&mut self.needed) {
            if !r.was_missing {
                self.on_hand.push(r.entry);
                continue;
            }
            match r.entry.res_type {
                res_type::SOUND | res_type::MODEL | res_type::EVENTSCRIPT | res_type::GENERIC => {}
                // `t_skin` goes straight on hand in the original.
                _ => {
                    self.on_hand.push(r.entry);
                    continue;
                }
            }
            // `CL_CheckFile`.
            let path = resource_path(&r.entry);
            if r.entry.download_size == MISSING_ON_SERVER {
                // The server has no such file either: not fatal any more.
                self.on_hand.push(r.entry);
                continue;
            }
            if !is_safe_to_download(&path) || !allow_download {
                out.skipped.push(if allow_download {
                    format!("not downloading {path}: the path is not one the client accepts from a server")
                } else {
                    format!("not downloading {path}: downloads are off")
                });
                if r.entry.flags & RES_FATALIFMISSING != 0 && out.fatal.is_none() {
                    out.fatal = Some(path);
                }
                self.on_hand.push(r.entry);
                continue;
            }
            if !r.requested {
                r.requested = true;
                out.dlfiles.push(path);
            }
            still_needed.push(r);
        }
        self.needed = still_needed;
        out.done = self.needed.is_empty();
        out
    }

    /// `CL_ProcessFile`: a file arrived. Settles the matching requested resource.
    ///
    /// The stream's name must be the very path that was requested (`resource_path`, the
    /// `dlfile` argument), compared without regard to case: no prefix stripping, so a
    /// server cannot answer a request for `sound/a.wav` with a file of another name.
    pub fn file_received(&mut self, name: &str) -> bool {
        let Some(pos) = self
            .needed
            .iter()
            .position(|r| r.requested && resource_path(&r.entry).eq_ignore_ascii_case(name))
        else {
            return false;
        };
        let r = self.needed.remove(pos);
        self.on_hand.push(r.entry);
        true
    }

    /// Downloads requested and not yet received.
    pub fn pending_downloads(&self) -> usize {
        self.needed.iter().filter(|r| r.requested).count()
    }

    /// Whether the server asked for a consistency answer that is still owed.
    pub fn consistency_owed(&self) -> bool {
        self.need_consistency_response
    }

    /// `CL_SendConsistencyInfo`: the payload of `clc_fileconsistency` (before its length
    /// prefix), munged with the spawn count, or `None` when no answer is owed.
    pub fn consistency_answer(
        &mut self,
        files: &dyn ResourceFiles,
        spawn_count: u32,
    ) -> Option<Vec<u8>> {
        if !self.need_consistency_response {
            return None;
        }
        self.need_consistency_response = false;
        let mut w = MsgWriter::new();
        w.start_bit_writing();
        // Servers rely on the md5 not being reset after a file that does not exist
        // (`MD5_HashFile` leaves the buffer alone), so it lives across the items.
        let mut md5 = [0u8; 16];
        for item in &self.consistency {
            w.write_one_bit(true);
            w.write_bits(u32::from(item.orig_index), CONSISTENCY_RESPONSE_INDEX_BITS);
            let path = if item.is_sound {
                format!("{SOUND_PATH}{}", item.filename)
            } else {
                item.filename.clone()
            };
            match item.check {
                CheckType::ExactFile => {
                    if let Some(hash) = files.md5(&path) {
                        md5 = hash;
                    }
                    w.write_bits(u32::from_le_bytes([md5[0], md5[1], md5[2], md5[3]]), 32);
                }
                CheckType::ModelSameBounds
                | CheckType::ModelSpecifyBounds
                | CheckType::ModelSpecifyBoundsIfAvail => {
                    // A missing file answers with the "no model" box (`Mod_GetStudioBounds`
                    // failing is a host error in the original; sending the box is milder).
                    let (mins, maxs) = files
                        .studio_bounds(&path)
                        .unwrap_or(([-1.0; 3], [-1.0; 3]));
                    for v in mins.into_iter().chain(maxs) {
                        w.write_bits(v.to_bits(), 32);
                    }
                }
            }
        }
        w.write_one_bit(false);
        w.end_bit_writing();
        let mut bytes = w.into_bytes();
        com_munge(&mut bytes, spawn_count as i32);
        Some(bytes)
    }

    /// Requests missing files via HTTP FastDL first (when `http` is installed and
    /// `sv_downloadurl` locations were received), falling back to `dlfile` once all HTTP URLs
    /// for a resource have failed.
    pub fn batch_request_with_http(
        &mut self,
        allow_download: bool,
        allow_dlfile_fallback: bool,
        mut http: Option<&mut (dyn HttpFetcher + '_)>,
    ) -> BatchOutcome {
        if let Some(fatal) = self.fatal_failure.take() {
            return BatchOutcome {
                fatal: Some(fatal),
                done: true,
                ..BatchOutcome::default()
            };
        }
        if let Some(fetcher) = http.as_deref_mut()
            && allow_download
            && !self.download_locations.is_empty()
        {
            let first_url = self.download_locations[0].clone();
            for item in &mut self.needed {
                if !item.was_missing || item.requested {
                    continue;
                }
                let rel_path = res_rel_path(&item.entry);
                let key = rel_path.to_ascii_lowercase();
                if self.http_exhausted.contains(&key)
                    || self.http_inflight.contains_key(&key)
                    || !is_safe_to_download(&rel_path)
                {
                    continue;
                }
                let try_bz2 = !key.ends_with(".bz2");
                if fetcher.start_fetch(&first_url, &rel_path, try_bz2) {
                    item.requested = true;
                    self.http_inflight
                        .insert(key, (rel_path, 0usize, try_bz2));
                }
            }
        }
        self.batch_request(allow_download && allow_dlfile_fallback)
    }

    /// Polls completed HTTP FastDL jobs, stores verified/decompressed files, retries raw on `.bz2`
    /// failure or advances to the next mirror URL, and marks exhausted entries for `dlfile` fallback.
    pub fn poll_http(
        &mut self,
        fetcher: &mut dyn HttpFetcher,
        files: &mut dyn ResourceFiles,
    ) -> (Vec<String>, bool) {
        let mut downloaded = Vec::new();
        let mut activity = false;
        for res in fetcher.poll_completed() {
            activity = true;
            let key = res.path.to_ascii_lowercase();
            let Some((rel_path, loc_idx, _)) = self.http_inflight.remove(&key) else {
                continue;
            };
            let decoded = match res.data {
                Ok(bytes) if !bytes.is_empty() && is_safe_to_download(&rel_path) => {
                    if res.is_bz2 {
                        decompress_bz2(&bytes, MAX_FILE_SIZE as usize)
                    } else if bytes.len() <= MAX_FILE_SIZE as usize {
                        Some(bytes)
                    } else {
                        None
                    }
                }
                _ => None,
            };
            if let Some(raw) = decoded.filter(|b| !b.is_empty()) {
                let _ = files.store(&rel_path, &raw);
                if self.file_received(&rel_path) {
                    downloaded.push(rel_path);
                }
                continue;
            }
            if res.is_bz2 {
                let url = &self.download_locations[loc_idx];
                if fetcher.start_fetch(url, &rel_path, false) {
                    self.http_inflight.insert(key, (rel_path, loc_idx, false));
                    continue;
                }
            } else if loc_idx + 1 < self.download_locations.len() {
                let next_idx = loc_idx + 1;
                let url = &self.download_locations[next_idx];
                let try_bz2 = !key.ends_with(".bz2");
                if fetcher.start_fetch(url, &rel_path, try_bz2) {
                    self.http_inflight
                        .insert(key, (rel_path, next_idx, try_bz2));
                    continue;
                }
            }
            self.http_exhausted.insert(key);
            self.http_needs_fallback = true;
            for item in &mut self.needed {
                if item.was_missing && res_rel_path(&item.entry).eq_ignore_ascii_case(&rel_path) {
                    item.requested = false;
                }
            }
        }
        (downloaded, activity)
    }

    pub fn take_http_needs_fallback(&mut self) -> bool {
        std::mem::take(&mut self.http_needs_fallback)
    }

    /// Handles `svc_filetxferfailed` for a pending resource download. Returns `true` if it matched.
    pub fn file_failed(&mut self, filename: &str) -> bool {
        let mut matched = false;
        for item in &mut self.needed {
            if !item.was_missing {
                continue;
            }
            let rel_path = res_rel_path(&item.entry);
            if rel_path.eq_ignore_ascii_case(filename)
                || item.entry.filename.eq_ignore_ascii_case(filename)
            {
                item.was_missing = false;
                item.requested = false;
                matched = true;
                if (item.entry.flags & RES_FATALIFMISSING) != 0 {
                    self.fatal_failure = Some(rel_path);
                }
            }
        }
        let key = filename.to_ascii_lowercase();
        self.http_inflight.remove(&key);
        matched
    }

    /// Remaining downloads count and the next active resource path for loading overlays.
    pub fn download_summary(&self) -> (usize, Option<String>) {
        let pending = self.needed.iter().filter(|r| r.was_missing).count();
        let current = self
            .needed
            .iter()
            .find(|r| r.was_missing && r.requested)
            .or_else(|| self.needed.iter().find(|r| r.was_missing))
            .map(|r| res_rel_path(&r.entry));
        (pending, current)
    }
}

fn res_rel_path(entry: &ResourceEntry) -> String {
    if entry.res_type == res_type::SOUND {
        format!("sound/{}", entry.filename)
    } else {
        entry.filename.clone()
    }
}

/// Parses a GoldSrc custom spray name `!MD5<32 hex chars>` into its 16-byte digest.
pub fn parse_md5_filename(name: &str) -> Option<[u8; 16]> {
    let hex_str = name.strip_prefix("!MD5")?.trim();
    if hex_str.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex_str[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// Decompresses raw `.bz2` bytes (FastDL HTTP payload), bounded strictly to `max_bytes`.
pub fn decompress_bz2(data: &[u8], max_bytes: usize) -> Option<Vec<u8>> {
    let mut decoder = bzip2_rs::DecoderReader::new(data);
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = decoder.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        if out.len() + n > max_bytes {
            return None;
        }
        out.extend_from_slice(&buf[..n]);
    }
    Some(out)
}

/// `CL_ResourcePath`: sounds live under `sound/`.
pub fn resource_path(entry: &ResourceEntry) -> String {
    if entry.res_type == res_type::SOUND {
        format!("{SOUND_PATH}{}", entry.filename)
    } else {
        entry.filename.clone()
    }
}

/// Directories a downloaded file may go into, each with the reason a server sends files there.
/// `models/` studio models, `sound/` wavs, `sprites/` sprites and the HUD `.txt` lists,
/// `maps/` the map and its `.res`/`.txt` companions, `gfx/` skybox and HUD images, `events/`
/// event scripts, `resource/` GUI `.res`/`.txt`, `overviews/` radar `.txt`/`.bmp`/`.tga`.
/// A root-level `.wad` is accepted separately (custom maps list their texture packs, see
/// [`is_safe_to_download`]); `.mp3` is accepted under `sound/` only.
const SAFE_DIRS: [&str; 8] = [
    "models", "sound", "sprites", "maps", "gfx", "events", "resource", "overviews",
];
/// `.mdl` models, `.wav` sounds, `.spr` sprites, `.bsp` maps, `.tga`/`.bmp` images, `.sc` event
/// scripts, `.txt`/`.res` text lists. Nothing the OS or the engine would run.
const SAFE_EXTENSIONS: [&str; 9] = [
    "mdl", "wav", "spr", "bsp", "tga", "bmp", "sc", "txt", "res",
];

/// Windows device names, which open a device whatever the extension.
fn is_device_name(component: &str) -> bool {
    // Windows ignores trailing spaces and dots of the stem: `con .mdl` opens `con`.
    let stem = component
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches([' ', '.'])
        .to_ascii_lowercase();
    matches!(stem.as_str(), "con" | "aux" | "nul" | "prn" | "conin$" | "conout$")
        || ["com", "lpt"].iter().any(|p| {
            stem.strip_prefix(p)
                .is_some_and(|n| n.len() == 1 && matches!(n.as_bytes()[0], b'1'..=b'9'))
        })
}

/// Whether a server may make the client write `path`: a whitelist, standing in for
/// `COM_IsSafeFileToDownload` (not in the fetched sources). The path is `/` separated, relative,
/// printable ASCII, either `dir/.../file` in one of [`SAFE_DIRS`] with an extension from
/// [`SAFE_EXTENSIONS`] (or `.mp3` under `sound/`), or a bare `name.wad` made of
/// `[A-Za-z0-9_.-]` in the game root. Rejected: `..`, `.`, empty components, backslashes, drive letters (`:`),
/// components ending in a dot or space (Windows drops them, so `x.dll.` is `x.dll`), control
/// characters and device names.
pub fn is_safe_to_download(path: &str) -> bool {
    if path.is_empty()
        || path.len() > 256
        || !path.bytes().all(|b| (0x20..0x7F).contains(&b))
        || path.contains(['\\', ':', '*', '?', '"', '<', '>', '|'])
    {
        return false;
    }
    let components: Vec<&str> = path.split('/').collect();
    // A map's texture pack sits in the game root: one component, a plain name, `.wad`.
    if components.len() == 1 {
        let name = components[0];
        return name.len() > 4
            && name.to_ascii_lowercase().ends_with(".wad")
            && !name.starts_with('.')
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
            && !name.contains("..")
            && !is_device_name(name);
    }
    if components.is_empty()
        || components.iter().any(|c| {
            c.is_empty()
                || *c == "."
                || *c == ".."
                || c.ends_with(['.', ' '])
                || c.starts_with(' ')
                || is_device_name(c)
        })
    {
        return false;
    }
    if !SAFE_DIRS.contains(&components[0].to_ascii_lowercase().as_str()) {
        return false;
    }
    let file = components[components.len() - 1];
    file.rsplit_once('.').is_some_and(|(_, ext)| {
        let ext = ext.to_ascii_lowercase();
        SAFE_EXTENSIONS.contains(&ext.as_str())
            // Music a map plays; under `sound/` only.
            || (ext == "mp3" && components[0].eq_ignore_ascii_case("sound"))
    })
}

/// Parses an assembled GoldSrc file stream (`Netchan_CopyFileFragments`): the file name, the
/// compressor name (`bz2` or empty), the declared uncompressed size (`u32`), then the data.
/// Refused (`None`): an unsafe name, a name starting with `!` (customizations stay in memory
/// in the original; there is no store for them here), a compressor other than `bz2`, a declared
/// size of 0 or over [`MAX_FILE_SIZE`], and data whose real size differs from the declared one.
pub fn parse_file_stream(stream: &[u8]) -> Option<ReceivedFile> {
    let mut pos = 0;
    let cstr = |pos: &mut usize| -> Option<String> {
        let rest = stream.get(*pos..)?;
        let end = rest.iter().position(|&b| b == 0)?;
        let s = String::from_utf8_lossy(&rest[..end]).into_owned();
        *pos += end + 1;
        Some(s)
    };
    let name = cstr(&mut pos)?;
    let compressor = cstr(&mut pos)?;
    let size_bytes = stream.get(pos..pos + 4)?;
    let declared = u32::from_le_bytes([size_bytes[0], size_bytes[1], size_bytes[2], size_bytes[3]]);
    pos += 4;
    let is_spray = parse_md5_filename(&name).is_some() && declared <= 65536;
    if (!is_safe_to_download(&name) && !is_spray) || declared == 0 || declared > MAX_FILE_SIZE {
        return None;
    }
    let body = &stream[pos..];
    let data = if compressor.eq_ignore_ascii_case("bz2") {
        let mut framed = b"BZ2\0".to_vec();
        framed.extend_from_slice(body);
        // Bounded by the declared size: a bomb stops at the limit.
        decompress_stream(&framed, declared as usize).ok()?
    } else if compressor.is_empty() {
        body.to_vec()
    } else {
        return None;
    };
    (data.len() == declared as usize).then_some(ReceivedFile { name, data })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(res_type: u8, name: &str, index: u16, size: u32, flags: u8) -> ResourceEntry {
        ResourceEntry {
            res_type,
            filename: name.to_string(),
            index,
            download_size: size,
            flags,
            md5: None,
            reserved: None,
        }
    }

    struct Files(Vec<&'static str>);

    impl ResourceFiles for Files {
        fn exists(&self, path: &str) -> bool {
            self.0.contains(&path)
        }
        fn md5(&self, path: &str) -> Option<[u8; 16]> {
            self.0.contains(&path).then(|| {
                let mut h = [0u8; 16];
                h[0] = path.len() as u8;
                h
            })
        }
    }

    #[test]
    fn missing_files_are_requested_once_and_settle_when_they_arrive() {
        let list = [
            entry(res_type::MODEL, "models/a.mdl", 2, 100, 0),
            entry(res_type::SOUND, "weapons/x.wav", 1, 50, 0),
            entry(res_type::MODEL, "*1", 3, 0, 0),
            entry(res_type::GENERIC, "sprites/y.txt", 1, 7, 0),
        ];
        let mut m = ResourceManager::default();
        m.begin(&list, &[], 3).unwrap();
        let files = Files(vec!["sprites/y.txt"]);
        assert_eq!(m.estimate_missing(&files), 150);
        let out = m.batch_request(true);
        assert_eq!(out.dlfiles, vec!["models/a.mdl", "sound/weapons/x.wav"]);
        assert!(!out.done);
        assert_eq!(m.pending_downloads(), 2);
        // A second batch asks for nothing new.
        assert!(m.batch_request(true).dlfiles.is_empty());
        assert!(m.file_received("sound/weapons/x.wav"));
        assert!(!m.file_received("models/unknown.mdl"));
        assert!(m.file_received("models/a.mdl"));
        assert!(m.batch_request(true).done);
        assert_eq!(m.on_hand().len(), 4);
    }

    #[test]
    fn a_fatal_missing_file_without_downloads_is_reported() {
        let list = [entry(res_type::MODEL, "models/a.mdl", 2, 100, RES_FATALIFMISSING)];
        let mut m = ResourceManager::default();
        m.begin(&list, &[], 3).unwrap();
        m.estimate_missing(&Files(vec![]));
        let out = m.batch_request(false);
        assert_eq!(out.fatal.as_deref(), Some("models/a.mdl"));
        assert!(out.dlfiles.is_empty());
        assert!(out.done);
    }

    #[test]
    fn unsafe_paths_are_never_requested() {
        assert!(is_safe_to_download("models/a.mdl"));
        assert!(is_safe_to_download("sound/weapons/x.wav"));
        assert!(is_safe_to_download("Maps/de_dust2.BSP"));
        // Texture packs of custom maps, and music under sound/.
        assert!(is_safe_to_download("cs_custom-2.WAD"));
        assert!(is_safe_to_download("sound/music/a.mp3"));
        for bad in ["models/a.mp3", "a.mp3", ".wad", "con.wad", "a b.wad", "x..wad", "a.wad.", "sub/a.wad", "a.dll"] {
            assert!(!is_safe_to_download(bad), "{bad:?} must be refused");
        }
        for bad in [
            "../x.mdl",
            "models/../../x.mdl",
            "C:/x.mdl",
            "/etc/passwd",
            "models\\a.mdl",
            "addons/x.dll",
            "models/x.dll",
            "models/x.dll.",
            "models/x.dll ",
            "models/x.mdl.",
            "models/x.dll.mdl.exe",
            "models/con.mdl",
            "models/con .mdl",
            "models/con..mdl",
            "models/nul .txt",
            "models/CON",
            "models/nul.txt",
            "models/aux.mdl",
            "models/prn.mdl",
            "models/com1.mdl",
            "models/lpt9.mdl",
            "models/a\u{1}.mdl",
            "x.mdl",
            "models//a.mdl",
            "models/./a.mdl",
            "!MD5abcdef",
            "models/a.cfg",
            "models/a:stream.mdl",
        ] {
            assert!(!is_safe_to_download(bad), "{bad:?} must be refused");
        }
        assert!(is_safe_to_download("models/com10.mdl"));
    }

    #[test]
    fn consistency_answer_lists_each_item_and_keeps_the_md5_across_missing_files() {
        use cs_proto48::bitbuf::MsgReader;
        use cs_proto48::crypto::com_unmunge;
        let list = [
            entry(res_type::MODEL, "models/a.mdl", 2, 1, 0),
            entry(res_type::MODEL, "models/gone.mdl", 3, 1, 0),
        ];
        let mut m = ResourceManager::default();
        m.begin(&list, &[0, 1], 5).unwrap();
        assert!(m.consistency_owed());
        let files = Files(vec!["models/a.mdl"]);
        let mut payload = m.consistency_answer(&files, 5).unwrap();
        assert!(!m.consistency_owed());
        assert!(m.consistency_answer(&files, 5).is_none());
        com_unmunge(&mut payload, 5);
        let mut r = MsgReader::new(&payload);
        r.start_bit_reading();
        let mut seen = Vec::new();
        while r.read_one_bit() {
            seen.push((r.read_bits(CONSISTENCY_RESPONSE_INDEX_BITS), r.read_bits(32)));
        }
        let first = u32::from(b"models/a.mdl".len() as u8);
        // The missing file repeats the previous md5.
        assert_eq!(seen, vec![(0, first), (1, first)]);
    }

    #[test]
    fn bad_consistency_position_is_an_error() {
        let mut m = ResourceManager::default();
        let list = [entry(res_type::MODEL, "models/a.mdl", 2, 1, 0)];
        assert!(m.begin(&list, &[1], 3).is_err());
    }

    fn stream(name: &str, compressor: &str, size: u32, data: &[u8]) -> Vec<u8> {
        let mut s = format!("{name}\0{compressor}\0").into_bytes();
        s.extend_from_slice(&size.to_le_bytes());
        s.extend_from_slice(data);
        s
    }

    #[test]
    fn file_stream_header_is_name_compressor_size() {
        assert_eq!(
            parse_file_stream(&stream("models/a.mdl", "", 3, b"abc")),
            Some(ReceivedFile {
                name: "models/a.mdl".to_string(),
                data: b"abc".to_vec()
            })
        );
        assert_eq!(parse_file_stream(b"x"), None);
    }

    #[test]
    fn file_streams_with_bad_names_sizes_or_compressors_are_refused() {
        // Customizations and unsafe names.
        assert_eq!(parse_file_stream(&stream("!MD5abc", "", 3, b"abc")), None);
        assert_eq!(parse_file_stream(&stream("../a.mdl", "", 3, b"abc")), None);
        assert_eq!(parse_file_stream(&stream("models/x.dll.", "", 3, b"abc")), None);
        assert_eq!(parse_file_stream(&stream("models/x.dll ", "", 3, b"abc")), None);
        assert_eq!(parse_file_stream(&stream("models/con.mdl", "", 3, b"abc")), None);
        // Declared size against the real one, zero and over the limit.
        assert_eq!(parse_file_stream(&stream("models/a.mdl", "", 4, b"abc")), None);
        assert_eq!(parse_file_stream(&stream("models/a.mdl", "", 2, b"abc")), None);
        assert_eq!(parse_file_stream(&stream("models/a.mdl", "", 0, b"")), None);
        assert_eq!(
            parse_file_stream(&stream("models/a.mdl", "", MAX_FILE_SIZE + 1, b"abc")),
            None
        );
        // A compressor other than bz2, and bz2 that is not bzip2.
        assert_eq!(parse_file_stream(&stream("models/a.mdl", "lzss", 3, b"abc")), None);
        assert_eq!(parse_file_stream(&stream("models/a.mdl", "bz2", 3, b"abc")), None);
    }

    #[test]
    fn download_locations_split_on_whitespace_and_are_capped() {
        let mut m = ResourceManager::default();
        m.add_download_locations("http://a/ http://b/").unwrap();
        assert_eq!(m.download_locations().len(), 2);
        m.add_download_locations(&"x".repeat(300)).unwrap();
        assert_eq!(m.download_locations().len(), 2);
        for i in 0..MAX_DOWNLOAD_LOCATIONS - 2 {
            m.add_download_locations(&format!("http://h{i}/")).unwrap();
        }
        assert!(m.add_download_locations("http://one-too-many/").is_err());
        assert_eq!(m.download_locations().len(), MAX_DOWNLOAD_LOCATIONS);
    }

    #[test]
    fn a_models_check_type_comes_from_its_munged_reserved_bytes() {
        let spawn_count = 7;
        let mut reserved = [0u8; 32];
        reserved[0] = 1; // samebounds
        com_munge(&mut reserved, spawn_count as i32);
        let mut model = entry(res_type::MODEL, "models/p.mdl", 2, 1, 0);
        model.reserved = Some(reserved);
        let plain = entry(res_type::MODEL, "models/q.mdl", 3, 1, 0);
        let mut m = ResourceManager::default();
        m.begin(&[model.clone(), plain], &[0, 1], spawn_count).unwrap();
        assert_eq!(m.consistency()[0].check, CheckType::ModelSameBounds);
        assert_eq!(m.consistency()[1].check, CheckType::ExactFile);
        // An unknown type is an error, as in the original.
        let mut bad = [0u8; 32];
        bad[0] = 9;
        com_munge(&mut bad, spawn_count as i32);
        model.reserved = Some(bad);
        assert!(m.begin(&[model], &[0], spawn_count).is_err());
        // Positions past the 10-bit field are refused.
        assert!(m.begin(&[], &[1024], spawn_count).is_err());
    }

    #[test]
    fn a_received_file_must_carry_the_requested_path() {
        let list = [
            entry(res_type::SOUND, "extra/boom.wav", 4, 50, 0),
            entry(res_type::GENERIC, "cs_custom.wad", 1, 50, 0),
        ];
        let mut m = ResourceManager::default();
        m.begin(&list, &[], 3).unwrap();
        m.estimate_missing(&Files(vec![]));
        let out = m.batch_request(true);
        assert_eq!(out.dlfiles, vec!["sound/extra/boom.wav", "cs_custom.wad"]);
        // The bare name of a sound is not its path.
        assert!(!m.file_received("extra/boom.wav"));
        assert!(!m.file_received("other/boom.wav"));
        assert!(m.file_received("SOUND/extra/boom.wav"));
        assert!(m.file_received("cs_custom.wad"));
        assert_eq!(m.pending_downloads(), 0);
    }

    #[test]
    fn an_unsafe_resource_is_skipped_with_a_console_line() {
        let list = [entry(res_type::GENERIC, "../evil.dll", 1, 5, 0)];
        let mut m = ResourceManager::default();
        m.begin(&list, &[], 3).unwrap();
        m.estimate_missing(&Files(vec![]));
        let out = m.batch_request(true);
        assert!(out.dlfiles.is_empty());
        assert_eq!(out.skipped.len(), 1);
        assert!(out.skipped[0].contains("../evil.dll"));
    }

    #[derive(Default)]
    struct MockHttp {
        started: Vec<(String, String, bool)>,
        responses: Vec<HttpFetchResult>,
    }

    impl HttpFetcher for MockHttp {
        fn start_fetch(&mut self, base_url: &str, rel_path: &str, is_bz2: bool) -> bool {
            self.started
                .push((base_url.to_string(), rel_path.to_string(), is_bz2));
            true
        }

        fn poll_completed(&mut self) -> Vec<HttpFetchResult> {
            std::mem::take(&mut self.responses)
        }

        fn has_pending(&self) -> bool {
            !self.responses.is_empty()
        }
    }

    #[derive(Default)]
    struct WritableFiles(BTreeMap<String, Vec<u8>>);

    impl ResourceFiles for WritableFiles {
        fn exists(&self, path: &str) -> bool {
            self.0.contains_key(&path.to_ascii_lowercase())
        }

        fn md5(&self, path: &str) -> Option<[u8; 16]> {
            self.0
                .get(&path.to_ascii_lowercase())
                .map(|b| cs_proto48::crypto::md5_digest(b))
        }

        fn store(&mut self, path: &str, data: &[u8]) -> bool {
            self.0.insert(path.to_ascii_lowercase(), data.to_vec());
            true
        }
    }

    #[test]
    fn http_fastdl_tries_bz2_then_raw_then_falls_back_to_dlfile() {
        let list = [
            entry(res_type::MODEL, "models/a.mdl", 2, 10, RES_FATALIFMISSING),
            entry(res_type::MODEL, "models/b.mdl", 3, 10, 0),
        ];
        let mut m = ResourceManager::default();
        m.add_download_locations("http://cdn.example/cstrike").unwrap();
        m.begin(&list, &[], 1).unwrap();
        let mut files = WritableFiles::default();
        m.estimate_missing(&files);

        let mut http = MockHttp::default();
        let out = m.batch_request_with_http(true, true, Some(&mut http));
        assert!(out.dlfiles.is_empty());
        assert!(!out.done);
        assert_eq!(http.started.len(), 2);
        assert!(http.started[0].2 && http.started[1].2);

        // `models/a.mdl.bz2` returns 404 -> retries raw `models/a.mdl` -> succeeds.
        // `models/b.mdl.bz2` and raw `models/b.mdl` both fail -> falls back to `dlfile`.
        http.responses = vec![
            HttpFetchResult {
                path: "models/a.mdl".into(),
                is_bz2: true,
                data: Err("404".into()),
            },
            HttpFetchResult {
                path: "models/b.mdl".into(),
                is_bz2: true,
                data: Err("404".into()),
            },
        ];
        let (done, act) = m.poll_http(&mut http, &mut files);
        assert!(act && done.is_empty());
        assert!(!m.take_http_needs_fallback());
        assert_eq!(http.started.len(), 4);
        assert!(!http.started[2].2 && !http.started[3].2);

        http.responses = vec![
            HttpFetchResult {
                path: "models/a.mdl".into(),
                is_bz2: false,
                data: Ok(b"IDST".to_vec()),
            },
            HttpFetchResult {
                path: "models/b.mdl".into(),
                is_bz2: false,
                data: Err("404".into()),
            },
        ];
        let (done, _) = m.poll_http(&mut http, &mut files);
        assert_eq!(done, vec!["models/a.mdl"]);
        assert!(files.exists("models/a.mdl"));
        assert!(m.take_http_needs_fallback());

        let fallback = m.batch_request_with_http(true, true, Some(&mut http));
        assert_eq!(fallback.dlfiles, vec!["models/b.mdl"]);
    }

    #[test]
    fn file_failed_settles_optional_and_marks_fatal_required() {
        let list = [
            entry(res_type::MODEL, "models/req.mdl", 2, 10, RES_FATALIFMISSING),
            entry(res_type::MODEL, "models/opt.mdl", 3, 10, 0),
        ];
        let mut m = ResourceManager::default();
        m.begin(&list, &[], 1).unwrap();
        m.estimate_missing(&WritableFiles::default());
        let out = m.batch_request(true);
        assert_eq!(out.dlfiles.len(), 2);

        assert!(m.file_failed("models/opt.mdl"));
        assert_eq!(m.pending_downloads(), 1);
        assert!(m.batch_request_with_http(true, true, None).fatal.is_none());

        assert!(m.file_failed("models/req.mdl"));
        assert_eq!(
            m.batch_request_with_http(true, true, None).fatal.as_deref(),
            Some("models/req.mdl")
        );
    }
}
