//! `sound/materials.txt` texture-type table (`PM_InitTextureTypes`, `PM_SortTextures`,
//! `PM_SwapTextures`, `PM_FindTextureType`). Ported from ReGameDLL_CS
//! `regamedll/pm_shared/pm_shared.cpp` (vanilla), read 2026-11-22.

use std::cmp::Ordering;

/// `MAX_TEXTURES`.
const MAX_TEXTURES: usize = 1024;
/// `MAX_TEXTURENAME_LENGHT` (sic): 16 characters plus the terminator.
const MAX_TEXTURENAME_LENGHT: usize = 17;
/// `sizeof(buffer) - 1` of `PM_InitTextureTypes`: the `bufferSize` passed to `memfgets`.
const LINE_BUFFER_SIZE: usize = 511;

/// The parsed table: `pm_gcTextures`, `pm_grgszTextureName`, `pm_grgchTextureType`, sorted by
/// name (case-insensitive) after parsing. Names are the raw bytes (at most 16).
#[derive(Debug, Clone, Default)]
pub struct TextureTable {
    names: Vec<Vec<u8>>,
    types: Vec<u8>,
}

/// C `isspace` in the "C" locale.
fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// `memfgets` of the engine: the next line of `file` starting at `*pos`, newline included, at most
/// `buffer_size - 1` bytes; `None` at the end of the file. Ported from ReHLDS
/// `rehlds/engine/tmessage.cpp` `memfgets`, lines 62-100, read 2026-11-22 (verified equivalent):
/// stop after the first `\n` or when the buffer is full, advance the position by what was copied,
/// return NULL when nothing was copied.
fn memfgets<'a>(file: &'a [u8], pos: &mut usize, buffer_size: usize) -> Option<&'a [u8]> {
    let start = *pos;
    if start >= file.len() {
        return None;
    }
    let mut last = file.len();
    if last - start >= buffer_size {
        last = start + buffer_size - 1;
    }
    let mut i = start;
    let mut stop = false;
    while i < last && !stop {
        if file[i] == b'\n' {
            stop = true;
        }
        i += 1;
    }
    if i == start {
        return None;
    }
    *pos = i;
    Some(&file[start..i])
}

/// `Q_stricmp` / `Q_strnicmp` over at most `n` bytes of two C strings (`a`, `b` are the bytes
/// before the terminator; the end of a slice is the NUL).
fn strnicmp(a: &[u8], b: &[u8], n: usize) -> Ordering {
    for k in 0..n {
        let ca = a.get(k).map_or(0, u8::to_ascii_lowercase);
        let cb = b.get(k).map_or(0, u8::to_ascii_lowercase);
        if ca != cb {
            return ca.cmp(&cb);
        }
        if ca == 0 {
            break;
        }
    }
    Ordering::Equal
}

impl TextureTable {
    /// `PM_InitTextureTypes` over the bytes of `sound/materials.txt`, then `PM_SortTextures`.
    /// (A failed load never gets here: the caller keeps `TextureTable::default()`, an empty table.)
    ///
    /// Quirks kept, each as in the source:
    /// - lines come from `memfgets` (511-byte buffer, newline kept in the line, an embedded NUL
    ///   ends the C string);
    /// - blank lines, lines starting with `/` and lines whose first non-space is not a letter are
    ///   skipped; the type letter is upper-cased;
    /// - a line with the type letter but no name, or whose name runs to the end of the line
    ///   buffer (the last line of a file without a trailing newline), is skipped;
    /// - names are cut to 16 characters;
    /// - at most 1024 entries are read;
    /// - the sort is the source's exchange sort with `Q_stricmp`.
    pub fn parse(bytes: &[u8]) -> TextureTable {
        let mut names: Vec<Vec<u8>> = Vec::new();
        let mut types: Vec<u8> = Vec::new();
        let mut pos = 0usize;

        while names.len() < MAX_TEXTURES {
            let Some(raw) = memfgets(bytes, &mut pos, LINE_BUFFER_SIZE) else {
                break;
            };
            // The line is a C string: it ends at an embedded NUL. `at(i)` is `buffer[i]`, 0 past
            // the end (the terminator).
            let line = raw.split(|&b| b == 0).next().unwrap_or(&[]);
            let at = |i: usize| line.get(i).copied().unwrap_or(0);

            // skip whitespace
            let mut i = 0;
            while at(i) != 0 && is_space(at(i)) {
                i += 1;
            }
            if at(i) == 0 {
                continue;
            }

            // skip comment lines
            if at(i) == b'/' || !at(i).is_ascii_alphabetic() {
                continue;
            }

            // get texture type
            let ty = at(i).to_ascii_uppercase();
            i += 1;

            // skip whitespace
            while at(i) != 0 && is_space(at(i)) {
                i += 1;
            }
            if at(i) == 0 {
                continue;
            }

            // get sentence name
            let mut j = i;
            while at(j) != 0 && !is_space(at(j)) {
                j += 1;
            }
            if at(j) == 0 {
                continue;
            }

            // null-terminate name and save in sentences array
            j = j.min(MAX_TEXTURENAME_LENGHT - 1 + i);
            names.push(line[i..j].to_vec());
            types.push(ty);
        }

        let mut table = TextureTable { names, types };
        table.sort();
        table
    }

    /// `PM_SortTextures`: the source's "bubble sort" (an exchange sort), `Q_stricmp(i, j) > 0`
    /// swaps (`PM_SwapTextures`). Kept as written: equal names keep whatever order it leaves.
    fn sort(&mut self) {
        let n = self.names.len();
        for i in 0..n {
            for j in (i + 1)..n {
                if strnicmp(&self.names[i], &self.names[j], usize::MAX) == Ordering::Greater {
                    self.names.swap(i, j);
                    self.types.swap(i, j);
                }
            }
        }
    }

    /// `PM_FindTextureType`: the `CHAR_TEX_*` letter of `name`; a miss is `b'C'`. Binary search
    /// with `Q_strnicmp(name, table[pivot], 16)`.
    pub fn find(&self, name: &str) -> u8 {
        let name = name.as_bytes();
        let name = name.split(|&b| b == 0).next().unwrap_or(&[]);
        let mut left: isize = 0;
        let mut right: isize = self.names.len() as isize - 1;

        while left <= right {
            let pivot = (left + right) / 2;
            match strnicmp(name, &self.names[pivot as usize], MAX_TEXTURENAME_LENGHT - 1) {
                Ordering::Equal => return self.types[pivot as usize],
                Ordering::Greater => left = pivot + 1,
                Ordering::Less => right = pivot - 1,
            }
        }

        b'C'
    }

    /// `pm_gcTextures`.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// True when the table has no entries (a failed load, or an empty file).
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}
