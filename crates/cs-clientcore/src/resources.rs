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
//! Assumptions: a model's consistency check type lives in 32 reserved bytes of its list entry,
//! which `cs-proto48` does not keep, so every consistency item is answered as `exactfile`
//! (an md5 of the file); a server that asks for model bounds gets a wrong answer for those.
//! `COM_IsSafeFileToDownload` is not in the fetched sources, [`is_safe_to_download`] is a
//! conservative stand-in.

use cs_proto48::bitbuf::MsgWriter;
use cs_proto48::compress::decompress_stream_default;
use cs_proto48::crypto::com_munge;
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
/// Bits of a resource index in a consistency answer (`MAX_GOLDSRC_MODEL_BITS`).
const CONSISTENCY_INDEX_BITS: u32 = 10;
/// A resource the server could not find reports this size (the 24-bit field read as -1).
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
    /// Stores a downloaded file. Returns `false` when it was not kept.
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
            let entry = resources.get(usize::from(index)).ok_or_else(|| {
                format!(
                    "malformed consistency info packet (index {index}, {} resources)",
                    resources.len()
                )
            })?;
            self.consistency.push(ConsistencyItem {
                filename: entry.filename.clone(),
                is_sound: entry.res_type == res_type::SOUND,
                orig_index: index,
                // Assumption: see the module doc.
                check: CheckType::ExactFile,
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
    pub fn add_download_locations(&mut self, text: &str) {
        if text.len() > 256 {
            return;
        }
        self.download_locations
            .extend(text.split_whitespace().map(str::to_string));
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
                    if r.entry.filename.starts_with('*') {
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
    pub fn file_received(&mut self, name: &str) -> bool {
        let without_dir = name.strip_prefix(SOUND_PATH).unwrap_or(name);
        let Some(pos) = self.needed.iter().position(|r| {
            r.requested
                && if r.entry.res_type == res_type::SOUND {
                    r.entry.filename.eq_ignore_ascii_case(without_dir)
                } else {
                    r.entry.filename.eq_ignore_ascii_case(name)
                }
        }) else {
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
            w.write_bits(u32::from(item.orig_index), CONSISTENCY_INDEX_BITS);
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
}

/// `CL_ResourcePath`: sounds live under `sound/`.
pub fn resource_path(entry: &ResourceEntry) -> String {
    if entry.res_type == res_type::SOUND {
        format!("{SOUND_PATH}{}", entry.filename)
    } else {
        entry.filename.clone()
    }
}

/// Whether a server may make the client write `path`. Stand-in for `COM_IsSafeFileToDownload`
/// (not in the fetched sources): no parent directories, no absolute or drive paths, no
/// executable or library extensions.
pub fn is_safe_to_download(path: &str) -> bool {
    if path.is_empty()
        || path.contains("..")
        || path.contains(':')
        || path.starts_with(['/', '\\'])
        || path.contains("//")
    {
        return false;
    }
    let lower = path.to_ascii_lowercase();
    ![
        ".dll", ".exe", ".so", ".dylib", ".bat", ".cmd", ".com", ".scr", ".sh", ".cfg", ".rc",
        ".lua",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext))
}

/// Parses an assembled GoldSrc file stream (`Netchan_CopyFileFragments`): the file name, the
/// compressor name (`bz2` or empty), the uncompressed size (`u32`), then the data.
/// Names the original refuses (empty, `..`, unsafe) give `None`.
pub fn parse_file_stream(stream: &[u8]) -> Option<ReceivedFile> {
    let mut pos = 0;
    let cstr =|pos: &mut usize| -> Option<String> {
        let rest = stream.get(*pos..)?;
        let end = rest.iter().position(|&b| b == 0)?;
        let s = String::from_utf8_lossy(&rest[..end]).into_owned();
        *pos += end + 1;
        Some(s)
    };
    let name = cstr(&mut pos)?;
    let compressor = cstr(&mut pos)?;
    let size_bytes = stream.get(pos..pos + 4)?;
    let _uncompressed = u32::from_le_bytes([size_bytes[0], size_bytes[1], size_bytes[2], size_bytes[3]]);
    pos += 4;
    // `!MD5...` names are customizations; the name check applies to real files only.
    if name.is_empty() || (!name.starts_with('!') && !is_safe_to_download(&name)) {
        return None;
    }
    let body = &stream[pos..];
    let data = if compressor.eq_ignore_ascii_case("bz2") {
        let mut framed = b"BZ2\0".to_vec();
        framed.extend_from_slice(body);
        decompress_stream_default(&framed).ok()?
    } else {
        body.to_vec()
    };
    Some(ReceivedFile { name, data })
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
        m.begin(&list, &[]).unwrap();
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
        m.begin(&list, &[]).unwrap();
        m.estimate_missing(&Files(vec![]));
        let out = m.batch_request(false);
        assert_eq!(out.fatal.as_deref(), Some("models/a.mdl"));
        assert!(out.dlfiles.is_empty());
        assert!(out.done);
    }

    #[test]
    fn unsafe_paths_are_never_requested() {
        assert!(is_safe_to_download("models/a.mdl"));
        assert!(!is_safe_to_download("../x.mdl"));
        assert!(!is_safe_to_download("C:/x.mdl"));
        assert!(!is_safe_to_download("/etc/passwd"));
        assert!(!is_safe_to_download("addons/x.dll"));
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
        m.begin(&list, &[0, 1]).unwrap();
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
            seen.push((r.read_bits(10), r.read_bits(32)));
        }
        let first = u32::from(b"models/a.mdl".len() as u8);
        // The missing file repeats the previous md5.
        assert_eq!(seen, vec![(0, first), (1, first)]);
    }

    #[test]
    fn bad_consistency_position_is_an_error() {
        let mut m = ResourceManager::default();
        let list = [entry(res_type::MODEL, "models/a.mdl", 2, 1, 0)];
        assert!(m.begin(&list, &[1]).is_err());
    }

    #[test]
    fn file_stream_header_is_name_compressor_size() {
        let mut s = b"models/a.mdl\0".to_vec();
        s.extend_from_slice(b"\0");
        s.extend_from_slice(&3u32.to_le_bytes());
        s.extend_from_slice(b"abc");
        assert_eq!(
            parse_file_stream(&s),
            Some(ReceivedFile {
                name: "models/a.mdl".to_string(),
                data: b"abc".to_vec()
            })
        );
        let mut bad = b"../a.mdl\0\0".to_vec();
        bad.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(parse_file_stream(&bad), None);
        assert_eq!(parse_file_stream(b"x"), None);
    }

    #[test]
    fn download_locations_split_on_whitespace() {
        let mut m = ResourceManager::default();
        m.add_download_locations("http://a/ http://b/");
        assert_eq!(m.download_locations().len(), 2);
        m.add_download_locations(&"x".repeat(300));
        assert_eq!(m.download_locations().len(), 2);
    }
}
