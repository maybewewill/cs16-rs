//! Client-side mod auto-download manager: parses `CsModManifest` / `CsModChunk`, enforces an
//! approval prompt (Risk R6), reassembles chunks, verifies SHA-256 digests, unpacks `.csmod`
//! assets into `Vfs`, and feeds verified WASM binaries to [`ClientModHost`](crate::ClientModHost).

use std::collections::{BTreeMap, VecDeque};

use cs_mod_api::{
    CMD_MOD_REQUEST, ClientModEntry, ManifestError, ModChunkPacket, ModManifest,
    ModManifestPacket, MOD_CHUNK_MAX_DATA, Permission, Permissions, USERMSG_MOD_CHUNK,
    USERMSG_MOD_MANIFEST, sha256_hex,
};
use cs_vfs::Vfs;
use thiserror::Error;

use crate::client_host::ClientModHost;
use crate::package::load_package_csmod;

pub const MAX_MOD_TRANSFER_BYTES: u32 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ApprovalPolicy {
    #[default]
    PromptRequired,
    AutoApproveTrusted,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TransferError {
    #[error("SHA-256 mismatch for downloaded mod `{id}` (expected {expected}, got {actual})")]
    ShaMismatch {
        id: String,
        expected: String,
        actual: String,
    },
    #[error("download size limit exceeded for mod `{0}`")]
    SizeLimit(String),
    #[error("failed to unpack downloaded mod `{0}`: {1}")]
    Unpack(String, String),
    #[error(transparent)]
    Manifest(#[from] ManifestError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingModApproval {
    pub mod_index: u8,
    pub entry: ClientModEntry,
    pub permission_names: Vec<&'static str>,
}

struct ActiveDownload {
    mod_index: u8,
    entry: ClientModEntry,
    approved: bool,
    buffer: Vec<u8>,
    received_offset: u32,
    completed: bool,
}

/// Manages client mod discovery, user approval prompts, chunked retrieval, and SHA-256 checks.
pub struct ClientModTransfer {
    policy: ApprovalPolicy,
    entries: BTreeMap<u8, ActiveDownload>,
    outgoing_cmds: VecDeque<String>,
    errors: Vec<TransferError>,
}

impl Default for ClientModTransfer {
    fn default() -> Self {
        Self::new(ApprovalPolicy::PromptRequired)
    }
}

impl ClientModTransfer {
    pub fn new(policy: ApprovalPolicy) -> Self {
        Self {
            policy,
            entries: BTreeMap::new(),
            outgoing_cmds: VecDeque::new(),
            errors: Vec::new(),
        }
    }

    pub fn pending_approvals(&self) -> Vec<PendingModApproval> {
        self.entries
            .values()
            .filter(|d| !d.approved && !d.completed)
            .map(|d| PendingModApproval {
                mod_index: d.mod_index,
                permission_names: Permissions::from_bits_truncate(d.entry.permissions_mask)
                    .names()
                    .into_iter()
                    .collect(),
                entry: d.entry.clone(),
            })
            .collect()
    }

    pub fn approve(&mut self, mod_id: &str) -> bool {
        let Some(dl) = self.entries.values_mut().find(|d| d.entry.id == mod_id) else {
            return false;
        };
        if !dl.approved {
            dl.approved = true;
            self.outgoing_cmds
                .push_back(format!("{CMD_MOD_REQUEST} {} 0", dl.mod_index));
        }
        true
    }

    pub fn deny(&mut self, mod_id: &str) -> bool {
        let keys: Vec<u8> = self
            .entries
            .iter()
            .filter(|(_, d)| d.entry.id == mod_id)
            .map(|(k, _)| *k)
            .collect();
        let removed = !keys.is_empty();
        for k in keys {
            self.entries.remove(&k);
        }
        removed
    }

    pub fn take_outgoing_commands(&mut self) -> Vec<String> {
        self.outgoing_cmds.drain(..).collect()
    }

    pub fn take_errors(&mut self) -> Vec<TransferError> {
        std::mem::take(&mut self.errors)
    }

    /// Processes a raw custom usermsg (`name`, `payload`) from `HudState::unknown_messages`.
    /// Returns `Some(true)` if handled internally (`CsModManifest`/`CsModChunk`).
    pub fn ingest_usermsg(
        &mut self,
        name: &str,
        payload: &[u8],
        client_host: &mut ClientModHost,
        vfs: Option<&mut Vfs>,
    ) -> bool {
        if name == USERMSG_MOD_MANIFEST {
            if let Some(pkt) = ModManifestPacket::decode(payload) {
                self.on_manifest(pkt);
            }
            return true;
        }
        if name == USERMSG_MOD_CHUNK {
            if let Some(pkt) = ModChunkPacket::decode(payload) {
                self.on_chunk(pkt, client_host, vfs);
            }
            return true;
        }
        false
    }

    fn on_manifest(&mut self, pkt: ModManifestPacket) {
        if pkt.entry.byte_len > MAX_MOD_TRANSFER_BYTES {
            self.errors.push(TransferError::SizeLimit(pkt.entry.id));
            return;
        }
        let approved = self.policy == ApprovalPolicy::AutoApproveTrusted;
        let idx = pkt.mod_index;
        self.entries.insert(
            idx,
            ActiveDownload {
                mod_index: idx,
                entry: pkt.entry,
                approved,
                buffer: Vec::new(),
                received_offset: 0,
                completed: false,
            },
        );
        if approved {
            self.outgoing_cmds
                .push_back(format!("{CMD_MOD_REQUEST} {idx} 0"));
        }
    }

    fn on_chunk(
        &mut self,
        pkt: ModChunkPacket,
        client_host: &mut ClientModHost,
        vfs: Option<&mut Vfs>,
    ) {
        let Some(dl) = self.entries.get_mut(&pkt.mod_index) else {
            return;
        };
        if !dl.approved || dl.completed || pkt.offset != dl.received_offset {
            return;
        }
        if dl.buffer.len() + pkt.data.len() > dl.entry.byte_len as usize {
            let id = dl.entry.id.clone();
            dl.completed = true;
            self.errors.push(TransferError::SizeLimit(id));
            return;
        }
        dl.buffer.extend_from_slice(&pkt.data);
        dl.received_offset = dl.buffer.len() as u32;

        if dl.received_offset < dl.entry.byte_len {
            if pkt.data.len() <= MOD_CHUNK_MAX_DATA {
                self.outgoing_cmds.push_back(format!(
                    "{CMD_MOD_REQUEST} {} {}",
                    dl.mod_index, dl.received_offset
                ));
            }
            return;
        }

        dl.completed = true;
        let actual = sha256_hex(&dl.buffer);
        if actual != dl.entry.sha256_hex {
            self.errors.push(TransferError::ShaMismatch {
                id: dl.entry.id.clone(),
                expected: dl.entry.sha256_hex.clone(),
                actual,
            });
            return;
        }

        if let Err(err) = install_verified_download(dl, client_host, vfs) {
            self.errors.push(err);
        }
    }
}

fn install_verified_download(
    dl: &ActiveDownload,
    client_host: &mut ClientModHost,
    vfs: Option<&mut Vfs>,
) -> Result<(), TransferError> {
    if dl.buffer.starts_with(b"PK\x03\x04") {
        let pkg = load_package_csmod(&dl.buffer, None)
            .map_err(|e| TransferError::Unpack(dl.entry.id.clone(), e.to_string()))?;
        if let (Some(vfs_mut), false) = (vfs, pkg.assets.is_empty()) {
            let _ = vfs_mut.mount_mod_assets(
                &pkg.manifest.id,
                pkg.assets.iter().map(|(k, v)| (k.clone(), v.clone())),
            );
        }
        if let Some(ref wasm) = pkg.client_wasm {
            client_host
                .load_client_mod(pkg.manifest, wasm)
                .map_err(|e| TransferError::Unpack(dl.entry.id.clone(), e))?;
        }
        return Ok(());
    }

    // Fallback: direct raw client WASM module without wrapper ZIP.
    let perms = Permissions::from_bits_truncate(dl.entry.permissions_mask);
    let mut manifest = ModManifest::parse_toml(&format!(
        "id = \"{}\"\nversion = \"{}\"\napi_version = 1\nside = \"client\"\n",
        dl.entry.id, dl.entry.version
    ))?;
    manifest.permissions = Permissions::NONE;
    for p in Permission::ALL_VARIANTS {
        if perms.contains(p) {
            manifest.permissions.insert(p);
        }
    }
    client_host
        .load_client_mod(manifest, &dl.buffer)
        .map_err(|e| TransferError::Unpack(dl.entry.id.clone(), e))
}
