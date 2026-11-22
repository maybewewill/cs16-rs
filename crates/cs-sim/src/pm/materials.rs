//! `sound/materials.txt` texture-type table (`PM_InitTextureTypes`, `PM_FindTextureType`).
//! stub: Task 7.

/// The parsed table: `pm_gcTextures`, `pm_grgszTextureName`, `pm_grgchTextureType`.
/// stub: Task 7.
#[derive(Debug, Clone, Default)]
pub struct TextureTable {}

impl TextureTable {
    /// `PM_InitTextureTypes` over the bytes of `sound/materials.txt`. stub: Task 7.
    pub fn parse(_bytes: &[u8]) -> TextureTable {
        TextureTable {}
    }

    /// `PM_FindTextureType`: the `CHAR_TEX_*` letter of `name`; a miss is `b'C'`.
    /// stub: Task 7 (always concrete).
    pub fn find(&self, _name: &str) -> u8 {
        b'C'
    }
}
