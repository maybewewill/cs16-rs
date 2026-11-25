//! Server-side resource download serving (`dlfile`, `sv_allowdownload`, `sv_downloadurl`),
//! `mp_consistency` enforcement (`clc_fileconsistency`), and custom player spray uploads/caching
//! (`!MD5<hex>`, `svc_customization`).
//!
//! Ported from ReHLDS `rehlds/engine/sv_user.cpp` and `rehlds/engine/sv_upld.cpp` (GPL-3,
//! read 2026-11-25).

use std::collections::{BTreeMap, HashMap};

use cs_assets::hpak::validate_spray_wad;
use cs_proto48::bitbuf::{MsgReader, MsgWriter};
use cs_proto48::compress::decompress_stream;
use cs_proto48::crypto::{com_munge, com_unmunge, md5_digest, md5_digest_hex};
use cs_proto48::messages::{
    CustomizationMsg, ResourceEntry, write_svc_customization, write_svc_filetxferfailed,
    write_svc_resourcelocation, write_svc_stufftext,
};

use crate::client::{ClientState, SignonStage};
use crate::game::GameDll;
use crate::server::Server;
use crate::signon::write_sendres_stage_with_consistency;

/// Bits per resource index in `clc_fileconsistency` (`SV_ParseConsistencyResponse`,
/// `sv_user.cpp:173` — 12 bits, whereas `SV_SendConsistencyList` writes 10 bits).
pub const CONSISTENCY_RESPONSE_INDEX_BITS: u32 = 12;

/// Server file storage abstraction for `dlfile` NetChan transfers and consistency hashing.
pub trait ServerFileProvider: Send {
    fn read_file(&self, rel_path: &str) -> Option<Vec<u8>>;

    fn md5(&self, rel_path: &str) -> Option<[u8; 16]> {
        self.read_file(rel_path).map(|b| md5_digest(&b))
    }

    fn studio_bounds(&self, rel_path: &str) -> Option<([f32; 3], [f32; 3])> {
        self.read_file(rel_path)
            .and_then(|b| cs_assets::mdl::studio_bounds(&b))
    }
}

/// Validates that a client `dlfile` path cannot escape the game directory or fetch executables/configs
/// (`SV_IsSafeFileToDownload` + strict allowlist matching client `is_safe_to_download`).
pub fn is_safe_server_download(path: &str) -> bool {
    if path.is_empty()
        || path.len() > 255
        || path.contains('\\')
        || path.contains('\0')
        || path.starts_with('/')
        || path.contains(':')
        || path.contains("..")
    {
        return false;
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.iter().any(|p| p.is_empty() || *p == "." || *p == "..") {
        return false;
    }
    let Some((_, ext)) = path.rsplit_once('.') else {
        return false;
    };
    let ext = ext.to_ascii_lowercase();
    if matches!(
        ext.as_str(),
        "cfg" | "rc" | "exe" | "dll" | "so" | "dylib" | "bat" | "cmd" | "vbs" | "ini" | "log" | "lst" | "sys"
    ) {
        return false;
    }
    if parts.len() == 1 {
        return ext == "wad";
    }
    let top = parts[0].to_ascii_lowercase();
    if top == "sound" && ext == "mp3" {
        return true;
    }
    matches!(
        top.as_str(),
        "models" | "sound" | "sprites" | "maps" | "gfx" | "events" | "resource" | "overviews"
    ) && matches!(
        ext.as_str(),
        "mdl" | "wav" | "spr" | "bsp" | "tga" | "bmp" | "sc" | "txt" | "res"
    )
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConsistencyRule {
    ExactFile { md5: [u8; 16] },
    ModelSameBounds { mins: [f32; 3], maxs: [f32; 3] },
    ModelSpecifyBounds { mins: [f32; 3], maxs: [f32; 3] },
    ModelSpecifyBoundsIfAvail { mins: [f32; 3], maxs: [f32; 3] },
}

impl ConsistencyRule {
    pub fn encode_reserved(&self, spawn_count: u32) -> Option<[u8; 32]> {
        let mut buf = [0u8; 32];
        let (kind, mins, maxs) = match *self {
            Self::ExactFile { .. } => return None,
            Self::ModelSameBounds { mins, maxs } => (1u8, mins, maxs),
            Self::ModelSpecifyBounds { mins, maxs } => (2u8, mins, maxs),
            Self::ModelSpecifyBoundsIfAvail { mins, maxs } => (3u8, mins, maxs),
        };
        buf[0] = kind;
        for a in 0..3 {
            buf[1 + a * 4..5 + a * 4].copy_from_slice(&mins[a].to_le_bytes());
            buf[13 + a * 4..17 + a * 4].copy_from_slice(&maxs[a].to_le_bytes());
        }
        com_munge(&mut buf, spawn_count as i32);
        Some(buf)
    }
}

/// Applies registered `ConsistencyRule`s to `resources` and returns sorted 10-bit resource list
/// indices to check.
pub fn attach_consistency_rules(
    resources: &mut [ResourceEntry],
    rules: &BTreeMap<String, ConsistencyRule>,
    spawn_count: u32,
) -> Vec<u16> {
    let mut indices = Vec::new();
    for (idx, r) in resources.iter_mut().enumerate() {
        if idx >= (1 << 10) {
            break;
        }
        let lookup = if r.res_type == crate::precache::res_type::SOUND {
            format!("sound/{}", r.filename)
        } else {
            r.filename.clone()
        };
        if let Some(rule) = rules
            .get(&lookup.to_ascii_lowercase())
            .or_else(|| rules.get(&r.filename.to_ascii_lowercase()))
        {
            r.flags |= 0x08; // RES_CHECKFILE
            match rule {
                ConsistencyRule::ExactFile { md5 } => {
                    r.md5 = Some(*md5);
                    r.reserved = None;
                }
                _ => {
                    r.reserved = rule.encode_reserved(spawn_count);
                }
            }
            indices.push(idx as u16);
        }
    }
    indices
}

fn bounds_exceed(cmins: [f32; 3], cmaxs: [f32; 3], mins: [f32; 3], maxs: [f32; 3]) -> bool {
    for a in 0..3 {
        if !cmins[a].is_finite() || !cmaxs[a].is_finite() {
            return true;
        }
        if cmins[a] < mins[a] || cmaxs[a] > maxs[a] {
            return true;
        }
    }
    false
}

/// Verifies a client's `clc_fileconsistency` payload against `expected_indices` and `rules`
/// (`SV_ParseConsistencyResponse`, ReHLDS `rehlds/engine/sv_user.cpp:148-246`).
pub fn verify_consistency_response(
    raw_payload: &[u8],
    spawn_count: u32,
    resources: &[ResourceEntry],
    expected_indices: &[u16],
    rules: &BTreeMap<String, ConsistencyRule>,
) -> Result<(), String> {
    if raw_payload.is_empty() {
        return Err("Invalid length".into());
    }
    let mut buf = raw_payload.to_vec();
    com_unmunge(&mut buf, spawn_count as i32);
    let mut r = MsgReader::new(&buf);
    r.start_bit_reading();
    let mut seen_count = 0usize;
    let mut offending: Option<String> = None;

    while r.read_one_bit() {
        let idx = r.read_bits(CONSISTENCY_RESPONSE_INDEX_BITS) as usize;
        let Some(res) = resources.get(idx) else {
            return Err("Bad file data".into());
        };
        if !expected_indices.contains(&(idx as u16)) {
            return Err("Bad file data".into());
        }
        let lookup = if res.res_type == crate::precache::res_type::SOUND {
            format!("sound/{}", res.filename)
        } else {
            res.filename.clone()
        };
        let rule = rules
            .get(&lookup.to_ascii_lowercase())
            .or_else(|| rules.get(&res.filename.to_ascii_lowercase()))
            .ok_or_else(|| "Bad file data".to_string())?;

        match rule {
            ConsistencyRule::ExactFile { md5 } => {
                let got_hash = r.read_bits(32);
                let expected = u32::from_le_bytes([md5[0], md5[1], md5[2], md5[3]]);
                if got_hash != expected {
                    offending = Some(res.filename.clone());
                }
            }
            ConsistencyRule::ModelSameBounds { mins, maxs } => {
                let cmins = [
                    f32::from_bits(r.read_bits(32)),
                    f32::from_bits(r.read_bits(32)),
                    f32::from_bits(r.read_bits(32)),
                ];
                let cmaxs = [
                    f32::from_bits(r.read_bits(32)),
                    f32::from_bits(r.read_bits(32)),
                    f32::from_bits(r.read_bits(32)),
                ];
                if cmins != *mins || cmaxs != *maxs {
                    offending = Some(res.filename.clone());
                }
            }
            ConsistencyRule::ModelSpecifyBounds { mins, maxs } => {
                let cmins = [
                    f32::from_bits(r.read_bits(32)),
                    f32::from_bits(r.read_bits(32)),
                    f32::from_bits(r.read_bits(32)),
                ];
                let cmaxs = [
                    f32::from_bits(r.read_bits(32)),
                    f32::from_bits(r.read_bits(32)),
                    f32::from_bits(r.read_bits(32)),
                ];
                if bounds_exceed(cmins, cmaxs, *mins, *maxs) {
                    offending = Some(res.filename.clone());
                }
            }
            ConsistencyRule::ModelSpecifyBoundsIfAvail { mins, maxs } => {
                let cmins = [
                    f32::from_bits(r.read_bits(32)),
                    f32::from_bits(r.read_bits(32)),
                    f32::from_bits(r.read_bits(32)),
                ];
                let cmaxs = [
                    f32::from_bits(r.read_bits(32)),
                    f32::from_bits(r.read_bits(32)),
                    f32::from_bits(r.read_bits(32)),
                ];
                let sentinel = cmins == [-1.0; 3] && cmaxs == [-1.0; 3];
                if !sentinel && bounds_exceed(cmins, cmaxs, *mins, *maxs) {
                    offending = Some(res.filename.clone());
                }
            }
        }
        seen_count += 1;
    }
    if seen_count != expected_indices.len() {
        return Err("Bad file data".into());
    }
    if let Some(file) = offending {
        return Err(format!(
            "Bad file Server is enforcing file consistency for {file}"
        ));
    }
    Ok(())
}

/// Server-side cache of validated client custom sprays (`!MD5<hex>`, `SV_CreateCustomizationList`
/// in `sv_upld.cpp`).
#[derive(Debug, Default)]
pub struct CustomizationStore {
    sprays: HashMap<[u8; 16], Vec<u8>>,
    by_slot: BTreeMap<u8, CustomizationMsg>,
}

impl CustomizationStore {
    pub fn contains_md5(&self, md5: &[u8; 16]) -> bool {
        self.sprays.contains_key(md5)
    }

    pub fn get_spray(&self, md5: &[u8; 16]) -> Option<&[u8]> {
        self.sprays.get(md5).map(Vec::as_slice)
    }

    pub fn get_by_md5_filename(&self, filename: &str) -> Option<&[u8]> {
        let hex_str = filename.strip_prefix("!MD5")?.trim();
        if hex_str.len() != 32 {
            return None;
        }
        let mut md5 = [0u8; 16];
        for (i, slot) in md5.iter_mut().enumerate() {
            *slot = u8::from_str_radix(&hex_str[i * 2..i * 2 + 2], 16).ok()?;
        }
        self.get_spray(&md5)
    }

    pub fn register_spray(
        &mut self,
        player_slot: u8,
        expected_md5: Option<[u8; 16]>,
        data: Vec<u8>,
        max_bytes: usize,
    ) -> Result<CustomizationMsg, String> {
        validate_spray_wad(&data, max_bytes).map_err(|e| e.to_string())?;
        let actual_md5 = md5_digest(&data);
        if let Some(exp) = expected_md5
            && exp != actual_md5
        {
            return Err("uploaded spray MD5 mismatch".into());
        }
        let name = format!("!MD5{}", md5_digest_hex(&actual_md5));
        let msg = CustomizationMsg {
            player: player_slot,
            res_type: crate::precache::res_type::DECAL,
            filename: name,
            index: 0,
            download_size: data.len() as u32,
            flags: 4, // RES_CUSTOM
            md5: Some(actual_md5),
        };
        if self.sprays.len() >= 256 && !self.sprays.contains_key(&actual_md5) {
            self.sprays.clear();
        }
        self.sprays.insert(actual_md5, data);
        self.by_slot.insert(player_slot, msg.clone());
        Ok(msg)
    }

    pub fn record_existing_for_slot(&mut self, player_slot: u8, md5: [u8; 16]) -> Option<CustomizationMsg> {
        let data = self.sprays.get(&md5)?;
        let msg = CustomizationMsg {
            player: player_slot,
            res_type: crate::precache::res_type::DECAL,
            filename: format!("!MD5{}", md5_digest_hex(&md5)),
            index: 0,
            download_size: data.len() as u32,
            flags: 4,
            md5: Some(md5),
        };
        self.by_slot.insert(player_slot, msg.clone());
        Some(msg)
    }

    pub fn remove_slot(&mut self, player_slot: u8) {
        self.by_slot.remove(&player_slot);
    }

    pub fn active_customizations(&self) -> impl Iterator<Item = &CustomizationMsg> {
        self.by_slot.values()
    }
}

/// Maximum size of a client-uploaded custom decal spray (GoldSrc `tempdecal.wad` limit).
pub const MAX_CUSTOM_SPRAY_BYTES: usize = 65536;
/// Maximum file size served via NetChan `dlfile`.
pub const MAX_SERVER_DLFILE_BYTES: usize = 64 * 1024 * 1024;

impl<G: GameDll> Server<G> {
    /// Installs the server-side file provider used for `dlfile` transfers and consistency checks.
    pub fn set_file_provider(&mut self, provider: Box<dyn ServerFileProvider>) {
        self.file_provider = Some(provider);
    }

    /// Registers a `mp_consistency` rule for `filename` (e.g. `"models/p_ak47.mdl"` or `"sprites/smoke.spr"`).
    pub fn set_consistency_rule(&mut self, filename: impl Into<String>, rule: ConsistencyRule) {
        self.consistency_rules
            .insert(filename.into().to_ascii_lowercase(), rule);
    }

    pub fn clear_consistency_rules(&mut self) {
        self.consistency_rules.clear();
    }

    pub fn customization_store(&self) -> &CustomizationStore {
        &self.customizations
    }

    pub(crate) fn on_stage_sendres(&mut self, slot: u8) {
        let dlurl = self.cvars.get_str("sv_downloadurl").trim().to_string();
        let mut resources = self.precache.resource_list(&|path| {
            self.file_provider
                .as_ref()
                .and_then(|p| p.read_file(path))
                .map_or(0, |b| b.len() as u32)
        });
        let consistency = if self.cvars.get_f32("mp_consistency") > 0.0
            && !self.consistency_rules.is_empty()
        {
            attach_consistency_rules(&mut resources, &self.consistency_rules, self.spawn_count)
        } else {
            Vec::new()
        };
        let mut w = MsgWriter::new();
        if !dlurl.is_empty() {
            let mut end = dlurl.len().min(255);
            while !dlurl.is_char_boundary(end) {
                end -= 1;
            }
            write_svc_resourcelocation(&mut w, &dlurl[..end]);
        }
        write_sendres_stage_with_consistency(&mut w, self.spawn_count, &resources, &consistency);
        self.resource_cache = resources;
        let index = usize::from(slot);
        let client = &mut self.clients[index];
        client.expected_consistency = consistency;
        client.consistency_verified = client.expected_consistency.is_empty();
        client.chan.write_reliable(w.as_slice());
        client.state = ClientState::Signon(SignonStage::AwaitSpawn);
    }

    pub(crate) fn handle_dlfile(&mut self, slot: u8, argv: &[String]) {
        let Some(path) = argv.get(1).map(|s| s.trim()).filter(|s| !s.is_empty()) else {
            return;
        };
        let send_failed = |server: &mut Self| {
            let mut w = MsgWriter::new();
            write_svc_filetxferfailed(&mut w, path);
            server.clients[usize::from(slot)]
                .chan
                .write_reliable(w.as_slice());
        };
        if self.cvars.get_f32("sv_allowdownload") <= 0.0 {
            send_failed(self);
            return;
        }
        let has_fastdl = !self.cvars.get_str("sv_downloadurl").trim().is_empty();
        if has_fastdl && self.cvars.get_f32("sv_allow_dlfile") <= 0.0 && !path.starts_with("!MD5") {
            send_failed(self);
            return;
        }
        let payload = if path.starts_with("!MD5") {
            self.customizations
                .get_by_md5_filename(path)
                .map(|b| b.to_vec())
        } else if is_safe_server_download(path) {
            self.file_provider.as_ref().and_then(|p| p.read_file(path))
        } else {
            None
        };
        let Some(bytes) = payload.filter(|b| !b.is_empty() && b.len() <= MAX_SERVER_DLFILE_BYTES)
        else {
            send_failed(self);
            return;
        };
        if !self.clients[usize::from(slot)]
            .chan
            .create_file_fragments(path, &bytes, false)
        {
            send_failed(self);
        }
    }

    pub(crate) fn handle_file_consistency(&mut self, slot: u8, payload: &[u8]) {
        if self.cvars.get_f32("mp_consistency") <= 0.0 {
            return;
        }
        let expected = self.clients[usize::from(slot)].expected_consistency.clone();
        if expected.is_empty() {
            self.clients[usize::from(slot)].consistency_verified = true;
            return;
        }
        match verify_consistency_response(
            payload,
            self.spawn_count,
            &self.resource_cache,
            &expected,
            &self.consistency_rules,
        ) {
            Ok(()) => {
                self.clients[usize::from(slot)].consistency_verified = true;
            }
            Err(reason) => {
                self.kick(slot, &reason);
            }
        }
    }

    pub(crate) fn stage_spawn_check_consistency(&mut self, slot: u8) -> bool {
        if self.cvars.get_f32("mp_consistency") > 0.0 {
            let client = &self.clients[usize::from(slot)];
            if !client.expected_consistency.is_empty() && !client.consistency_verified {
                self.kick(slot, "Bad file Server is enforcing file consistency");
                return false;
            }
        }
        true
    }

    pub(crate) fn replay_customizations_to(&mut self, slot: u8) {
        let msgs: Vec<CustomizationMsg> = self
            .customizations
            .active_customizations()
            .cloned()
            .collect();
        if msgs.is_empty() {
            return;
        }
        let mut w = MsgWriter::new();
        for msg in &msgs {
            write_svc_customization(&mut w, msg);
        }
        self.clients[usize::from(slot)]
            .chan
            .write_reliable(w.as_slice());
    }

    pub(crate) fn handle_clc_resourcelist(&mut self, slot: u8, entries: &[ResourceEntry]) {
        let Some(entry) = entries
            .iter()
            .find(|e| e.res_type == crate::precache::res_type::DECAL && (e.flags & 4) != 0)
        else {
            return;
        };
        let Some(md5) = entry.md5 else {
            return;
        };
        if self.customizations.contains_md5(&md5) {
            if let Some(msg) = self.customizations.record_existing_for_slot(slot, md5) {
                self.broadcast_customization(&msg);
            }
            return;
        }
        if self.cvars.get_f32("sv_allowupload") > 0.0 {
            let client = &mut self.clients[usize::from(slot)];
            client.expected_spray_md5 = Some(md5);
            let mut w = MsgWriter::new();
            write_svc_stufftext(&mut w, "upload\n");
            client.chan.write_reliable(w.as_slice());
        }
    }

    pub(crate) fn handle_incoming_file_stream(&mut self, slot: u8, stream: &[u8]) {
        if self.cvars.get_f32("sv_allowupload") <= 0.0 {
            return;
        }
        let Some((name, raw)) = parse_uploaded_customization(stream, MAX_CUSTOM_SPRAY_BYTES) else {
            tracing::warn!(slot, "ignored malformed incoming client file stream");
            return;
        };
        if !name.starts_with("!MD5") {
            tracing::warn!(slot, %name, "rejected non-customization upload");
            return;
        }
        let expected_md5 = self.clients[usize::from(slot)].expected_spray_md5.take();
        match self
            .customizations
            .register_spray(slot, expected_md5, raw, MAX_CUSTOM_SPRAY_BYTES)
        {
            Ok(msg) => self.broadcast_customization(&msg),
            Err(err) => tracing::warn!(slot, %err, "rejected invalid custom spray upload"),
        }
    }

    fn broadcast_customization(&mut self, msg: &CustomizationMsg) {
        let mut w = MsgWriter::new();
        write_svc_customization(&mut w, msg);
        let bytes = w.into_bytes();
        for client in &mut self.clients {
            if client.state == ClientState::Active && !client.fake {
                client.chan.write_reliable(&bytes);
            }
        }
    }
}

fn parse_uploaded_customization(stream: &[u8], max_bytes: usize) -> Option<(String, Vec<u8>)> {
    let mut pos = 0usize;
    let mut cstr = || -> Option<String> {
        let rest = stream.get(pos..)?;
        let end = rest.iter().position(|&b| b == 0)?;
        let s = String::from_utf8_lossy(&rest[..end]).into_owned();
        pos += end + 1;
        Some(s)
    };
    let name = cstr()?;
    let compressor = cstr()?;
    let size_bytes = stream.get(pos..pos + 4)?;
    let declared = u32::from_le_bytes([size_bytes[0], size_bytes[1], size_bytes[2], size_bytes[3]]) as usize;
    pos += 4;
    if declared == 0 || declared > max_bytes {
        return None;
    }
    let body = &stream[pos..];
    let data = if compressor.eq_ignore_ascii_case("bz2") {
        let mut framed = b"BZ2\0".to_vec();
        framed.extend_from_slice(body);
        decompress_stream(&framed, declared).ok()?
    } else if compressor.is_empty() {
        body.to_vec()
    } else {
        return None;
    };
    (data.len() == declared).then_some((name, data))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs_proto48::bitbuf::MsgWriter;

    #[test]
    fn safe_server_download_blocks_traversal_and_executables() {
        assert!(is_safe_server_download("maps/de_dust2.bsp"));
        assert!(is_safe_server_download("models/player/gign/gign.mdl"));
        assert!(is_safe_server_download("cs_dust.wad"));
        assert!(is_safe_server_download("sound/ambience/wind.mp3"));

        assert!(!is_safe_server_download("server.cfg"));
        assert!(!is_safe_server_download("../cstrike/server.cfg"));
        assert!(!is_safe_server_download("/etc/passwd"));
        assert!(!is_safe_server_download("models/bad.dll"));
        assert!(!is_safe_server_download("C:/secret.txt"));
    }

    #[test]
    fn consistency_rule_accepts_exact_and_bounds_and_rejects_mismatch() {
        let hash = [0xAAu8; 16];
        let mut rules = BTreeMap::new();
        rules.insert("sprites/smoke.spr".into(), ConsistencyRule::ExactFile { md5: hash });
        rules.insert(
            "models/p_ak47.mdl".into(),
            ConsistencyRule::ModelSpecifyBounds {
                mins: [-31.0, -8.0, -21.0],
                maxs: [31.0, 12.0, 31.0],
            },
        );
        let mut resources = vec![
            ResourceEntry {
                res_type: 2,
                filename: "sprites/smoke.spr".into(),
                index: 1,
                download_size: 100,
                flags: 0,
                md5: None,
                reserved: None,
            },
            ResourceEntry {
                res_type: 2,
                filename: "models/p_ak47.mdl".into(),
                index: 2,
                download_size: 200,
                flags: 0,
                md5: None,
                reserved: None,
            },
        ];
        let indices = attach_consistency_rules(&mut resources, &rules, 7);
        assert_eq!(indices, vec![0, 1]);

        let encode = |h: [u8; 16], mn: [f32; 3], mx: [f32; 3]| -> Vec<u8> {
            let mut w = MsgWriter::new();
            w.start_bit_writing();
            w.write_one_bit(true);
            w.write_bits(0, CONSISTENCY_RESPONSE_INDEX_BITS);
            w.write_bits(u32::from_le_bytes([h[0], h[1], h[2], h[3]]), 32);
            w.write_one_bit(true);
            w.write_bits(1, CONSISTENCY_RESPONSE_INDEX_BITS);
            for v in mn.into_iter().chain(mx) {
                w.write_bits(v.to_bits(), 32);
            }
            w.write_one_bit(false);
            w.end_bit_writing();
            let mut b = w.into_bytes();
            com_munge(&mut b, 7);
            b
        };

        let good = encode(hash, [-1.0, -1.0, -7.0], [1.0, 11.0, 28.0]);
        assert!(verify_consistency_response(&good, 7, &resources, &indices, &rules).is_ok());

        let bad_hash = encode([0x11; 16], [-1.0, -1.0, -7.0], [1.0, 11.0, 28.0]);
        assert!(verify_consistency_response(&bad_hash, 7, &resources, &indices, &rules).is_err());

        let bad_bounds = encode(hash, [-50.0, 0.0, 0.0], [1.0, 11.0, 28.0]);
        assert!(verify_consistency_response(&bad_bounds, 7, &resources, &indices, &rules).is_err());
    }

    struct MapFiles(BTreeMap<String, Vec<u8>>);

    impl ServerFileProvider for MapFiles {
        fn read_file(&self, rel_path: &str) -> Option<Vec<u8>> {
            self.0.get(&rel_path.to_ascii_lowercase()).cloned()
        }
    }

    struct EmptyGame;

    impl GameDll for EmptyGame {
        fn usermsgs(&self) -> cs_proto48::usermsg::UserMsgRegistry {
            cs_proto48::usermsg::UserMsgRegistry::cs16_default()
        }
        fn server_activate(&mut self, eng: &mut crate::EngineApi<'_>) {
            let _ = eng.precache_model("models/test.mdl");
        }
        fn server_deactivate(&mut self, _eng: &mut crate::EngineApi<'_>) {}
        fn spawn_entity(
            &mut self,
            _eng: &mut crate::EngineApi<'_>,
            _id: crate::EdictId,
            _classname: &str,
            _keys: &[(String, String)],
        ) -> bool {
            false
        }
        fn client_connect(&mut self, _eng: &mut crate::EngineApi<'_>, _slot: u8) -> Result<(), String> {
            Ok(())
        }
        fn client_put_in_server(&mut self, _eng: &mut crate::EngineApi<'_>, _slot: u8) {}
        fn client_disconnect(&mut self, _eng: &mut crate::EngineApi<'_>, _slot: u8) {}
        fn client_command(&mut self, _eng: &mut crate::EngineApi<'_>, _slot: u8, _argv: &[String]) {}
        fn client_userinfo_changed(&mut self, _eng: &mut crate::EngineApi<'_>, _slot: u8) {}
        fn run_usercmd(
            &mut self,
            _eng: &mut crate::EngineApi<'_>,
            _slot: u8,
            _cmd: &cs_proto48::delta::GoldSrcUserCmd,
            _random_seed: u32,
        ) {
        }
        fn start_frame(&mut self, _eng: &mut crate::EngineApi<'_>, _frametime: f32) {}
        fn update_client_data(
            &mut self,
            _eng: &mut crate::EngineApi<'_>,
            _slot: u8,
        ) -> cs_proto48::delta::GoldSrcClientData {
            Default::default()
        }
    }

    fn dummy_spray_wad() -> Vec<u8> {
        let (width, height) = (32u32, 32u32);
        let mip0 = (width * height) as usize;
        let mip1 = ((width / 2) * (height / 2)) as usize;
        let mip2 = ((width / 4) * (height / 4)) as usize;
        let mip3 = ((width / 8) * (height / 8)) as usize;
        let miptex_len = 40 + mip0 + mip1 + mip2 + mip3 + 2 + 256 * 3;
        let mut miptex = vec![0u8; miptex_len];
        miptex[0..6].copy_from_slice(b"{logo\0");
        miptex[16..20].copy_from_slice(&width.to_le_bytes());
        miptex[20..24].copy_from_slice(&height.to_le_bytes());
        let off0 = 40u32;
        let off1 = off0 + mip0 as u32;
        let off2 = off1 + mip1 as u32;
        let off3 = off2 + mip2 as u32;
        miptex[24..28].copy_from_slice(&off0.to_le_bytes());
        miptex[28..32].copy_from_slice(&off1.to_le_bytes());
        miptex[32..36].copy_from_slice(&off2.to_le_bytes());
        miptex[36..40].copy_from_slice(&off3.to_le_bytes());
        let pal_hdr = off3 as usize + mip3;
        miptex[pal_hdr..pal_hdr + 2].copy_from_slice(&256u16.to_le_bytes());
        let last_rgb = pal_hdr + 2 + 255 * 3;
        miptex[last_rgb..last_rgb + 3].copy_from_slice(&[0, 0, 255]);

        let dir_off = 12 + miptex.len();
        let mut wad = Vec::with_capacity(dir_off + 32);
        wad.extend_from_slice(b"WAD3");
        wad.extend_from_slice(&1u32.to_le_bytes());
        wad.extend_from_slice(&(dir_off as u32).to_le_bytes());
        wad.extend_from_slice(&miptex);
        let mut dir = [0u8; 32];
        dir[0..4].copy_from_slice(&12u32.to_le_bytes());
        dir[4..8].copy_from_slice(&(miptex.len() as u32).to_le_bytes());
        dir[8..12].copy_from_slice(&(miptex.len() as u32).to_le_bytes());
        dir[12] = 0x43;
        dir[16..22].copy_from_slice(b"{logo\0");
        wad.extend_from_slice(&dir);
        assert!(validate_spray_wad(&wad, MAX_CUSTOM_SPRAY_BYTES).is_ok());
        wad
    }

    fn dummy_server() -> Server<EmptyGame> {
        let mut files = BTreeMap::new();
        files.insert("models/test.mdl".into(), vec![b'I', b'D', b'S', b'T', 1, 2, 3, 4]);
        let mut srv = Server::new(
            crate::ServerConfig::default(),
            crate::ServerMap::stub("de_dust2"),
            EmptyGame,
            cs_proto48::delta::DeltaRegistry::cs16_default(),
            cs_cvar::CVarRegistry::new_with_defaults(),
        );
        srv.set_file_provider(Box::new(MapFiles(files)));
        srv
    }

    fn connect_slot0(srv: &mut Server<EmptyGame>) {
        srv.clients[0] = crate::client::Client::connected(cs_transport::PeerAddr::Loopback(1), 1, 0.0);
        srv.views[0].in_use = true;
        srv.views[0].userinfo.set("name", "Player");
    }

    #[test]
    fn server_dlfile_serves_safe_files_and_rejects_missing_or_unsafe() {
        let mut srv = dummy_server();
        connect_slot0(&mut srv);
        srv.handle_dlfile(0, &["dlfile".into(), "models/test.mdl".into()]);
        assert!(srv.clients[0].chan.is_file_transfer_active("models/test.mdl"));

        srv.handle_dlfile(0, &["dlfile".into(), "models/missing.mdl".into()]);
        assert!(srv.clients[0].chan.pending_reliable_bytes() > 0);
        srv.handle_dlfile(0, &["dlfile".into(), "../server.cfg".into()]);
    }

    #[test]
    fn server_mp_consistency_accepts_match_and_kicks_mismatch() {
        let mut srv = dummy_server();
        let model_bytes = vec![b'I', b'D', b'S', b'T', 1, 2, 3, 4];
        let hash = md5_digest(&model_bytes);
        srv.set_consistency_rule("models/test.mdl", ConsistencyRule::ExactFile { md5: hash });
        connect_slot0(&mut srv);
        srv.on_stage_sendres(0);
        assert_eq!(srv.clients[0].expected_consistency.len(), 1);
        let idx = srv.clients[0].expected_consistency[0];
        let spawn_count = srv.spawn_count() as i32;

        let build_reply = |h: [u8; 16]| {
            let mut w = MsgWriter::new();
            w.start_bit_writing();
            w.write_one_bit(true);
            w.write_bits(u32::from(idx), CONSISTENCY_RESPONSE_INDEX_BITS);
            w.write_bits(u32::from_le_bytes([h[0], h[1], h[2], h[3]]), 32);
            w.write_one_bit(false);
            w.end_bit_writing();
            let mut b = w.into_bytes();
            com_munge(&mut b, spawn_count);
            b
        };

        srv.handle_file_consistency(0, &build_reply(hash));
        assert!(srv.clients[0].consistency_verified);
        assert!(srv.stage_spawn_check_consistency(0));

        // Mismatch kicks client.
        srv.clients[0].consistency_verified = false;
        srv.handle_file_consistency(0, &build_reply([0xFF; 16]));
        assert!(srv.clients[0].is_free());
    }

    #[test]
    fn server_caches_uploaded_custom_spray_and_broadcasts_customization() {
        let mut srv = dummy_server();
        connect_slot0(&mut srv);
        srv.clients[0].state = ClientState::Active;
        let wad = dummy_spray_wad();
        let md5 = md5_digest(&wad);
        let name = format!("!MD5{}", md5_digest_hex(&md5));
        srv.handle_clc_resourcelist(
            0,
            &[ResourceEntry {
                res_type: crate::precache::res_type::DECAL,
                filename: name.clone(),
                index: 0,
                download_size: wad.len() as u32,
                flags: 4,
                md5: Some(md5),
                reserved: None,
            }],
        );
        assert_eq!(srv.clients[0].expected_spray_md5, Some(md5));

        let mut stream = Vec::new();
        stream.extend_from_slice(name.as_bytes());
        stream.push(0);
        stream.push(0); // empty compressor
        stream.extend_from_slice(&(wad.len() as u32).to_le_bytes());
        stream.extend_from_slice(&wad);
        srv.handle_incoming_file_stream(0, &stream);
        assert!(srv.customization_store().contains_md5(&md5));
        assert_eq!(
            srv.customization_store().get_by_md5_filename(&name),
            Some(wad.as_slice())
        );
    }
}
