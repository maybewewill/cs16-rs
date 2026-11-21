//! The map cycle of `mapcycle.txt` (`mapcycle_t`, `ReloadMapCycleFile` and the item walk of
//! `CHalfLifeMultiplay::ChangeLevel`, ReGameDLL_CS `multiplay_gamerules.cpp`, read 2026-11-22).
//! It outlives the maps: [`crate::GameState::map_cycle`] is not reset when the map changes.

use crate::menus;

/// Most maps a cycle keeps, and most bytes of `mapcycle.txt` read (the original has no bound;
/// a file beyond these is cut).
const MAX_ITEMS: usize = 256;
const MAX_FILE_BYTES: usize = 64 * 1024;

/// One line of `mapcycle.txt` (`mapcycle_item_t`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MapCycleItem {
    pub(crate) map: String,
    pub(crate) min_players: i32,
    pub(crate) max_players: i32,
}

/// The maps `mapcycle.txt` lists, in order, and the one the cycle is at (`mapcycle_t`).
#[derive(Debug, Clone, Default)]
pub struct MapCycle {
    pub(crate) items: Vec<MapCycleItem>,
    pub(crate) next: usize,
}

/// The tokens of a Half-Life text file (`SharedParse`): words split by white space, `"quoted"`
/// strings, `//` comments to the end of the line.
fn tokenize_line(line: &str) -> Vec<String> {
    let line = line.split("//").next().unwrap_or("");
    let mut tokens = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() || c.is_control() {
            chars.next();
        } else if c == '"' {
            chars.next();
            let mut token = String::new();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                token.push(c);
            }
            tokens.push(token);
        } else {
            let mut token = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() || c.is_control() {
                    break;
                }
                token.push(c);
                chars.next();
            }
            tokens.push(token);
        }
    }
    tokens
}

/// The value of `key` in an info string `\key\value\key2\value2` (`GET_KEY_VALUE`), as a number
/// (`atoi`), 0 when it is missing.
fn info_number(info: &str, key: &str) -> i32 {
    let mut parts = info.split('\\').skip(1);
    while let (Some(k), Some(v)) = (parts.next(), parts.next()) {
        if k == key {
            return menus::atoi(v);
        }
    }
    0
}

impl MapCycle {
    /// `ReloadMapCycleFile` on `mapcycle.txt` (the `mapcyclefile` cvar is not read): every line
    /// names a map, optionally followed by `\minplayers\N\maxplayers\N`. A map that is not
    /// valid (`IS_MAP_VALID`: it has no `maps/<name>.bsp`) is skipped. The first map of the
    /// file is where the cycle starts. Nothing is listed for a file that is missing or empty.
    pub fn load(vfs: &cs_vfs::Vfs) -> Self {
        let text = match vfs.read_string_lossy("mapcycle.txt") {
            Ok(text) => text,
            Err(cs_vfs::VfsError::NotFound(_)) => String::new(),
            Err(err) => {
                tracing::warn!(%err, "mapcycle.txt could not be read");
                String::new()
            }
        };
        let text = match text.char_indices().nth(MAX_FILE_BYTES) {
            Some((cut, _)) => &text[..cut],
            None => &text[..],
        };
        let mut items = Vec::new();
        for line in text.lines() {
            if items.len() >= MAX_ITEMS {
                tracing::warn!("mapcycle.txt: more than {MAX_ITEMS} maps, the rest is ignored");
                break;
            }
            let tokens = tokenize_line(line);
            let Some(map) = tokens.first() else { continue };
            if !cs_engine::valid_map_name(map) || !vfs.exists(&format!("maps/{map}.bsp")) {
                tracing::info!(map = %map, "skipping a map of the map cycle: not a valid map");
                continue;
            }
            let rules = tokens.get(1).map_or("", String::as_str);
            items.push(MapCycleItem {
                map: map.clone(),
                min_players: info_number(rules, "minplayers").max(0),
                max_players: info_number(rules, "maxplayers").max(0),
            });
        }
        Self { items, next: 0 }
    }

    /// The map the cycle is at (where [`advance`](Self::advance) goes next when no player range
    /// holds it back), `None` for an empty cycle.
    pub fn next_map(&self) -> Option<&str> {
        self.items.get(self.next).map(|i| i.map.as_str())
    }

    /// The map the cycle goes to next with `players` players on the server, and moves on past it
    /// (the item walk of `ChangeLevel`): the first map from where the cycle stands whose player
    /// range holds `players`; the last map of the walk is never looked at, as in the original.
    /// `max_clients` caps the ranges. `None` for an empty cycle.
    pub(crate) fn advance(&mut self, players: i32, max_clients: i32) -> Option<String> {
        let n = self.items.len();
        if n == 0 {
            return None;
        }
        let mut found = false;
        let mut chosen = None;
        for k in 0..n - 1 {
            let i = (self.next + k) % n;
            let item = &self.items[i];
            let mut keep_looking = false;
            if item.min_players != 0 {
                if players >= item.min_players.min(max_clients) {
                    found = true;
                } else {
                    keep_looking = true;
                }
            }
            if item.max_players != 0 {
                if players <= item.max_players.min(max_clients) {
                    found = true;
                } else {
                    keep_looking = true;
                }
            }
            if keep_looking {
                continue;
            }
            found = true;
            chosen = Some(i);
            break;
        }
        let index = match (chosen, found) {
            (Some(i), _) => i,
            // The walk ended: the last item of it, or where the cycle stood if nothing fitted.
            (None, true) => (self.next + n - 1) % n,
            (None, false) => self.next,
        };
        self.next = (index + 1) % n;
        Some(self.items[index].map.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cycle(lines: &[(&str, i32, i32)]) -> MapCycle {
        MapCycle {
            items: lines
                .iter()
                .map(|&(map, min_players, max_players)| MapCycleItem {
                    map: map.to_string(),
                    min_players,
                    max_players,
                })
                .collect(),
            next: 0,
        }
    }

    #[test]
    fn the_cycle_goes_round_in_order() {
        let mut c = cycle(&[("a", 0, 0), ("b", 0, 0), ("c", 0, 0)]);
        let got: Vec<_> = (0..5).filter_map(|_| c.advance(4, 32)).collect();
        assert_eq!(got, ["a", "b", "c", "a", "b"]);
        assert_eq!(MapCycle::default().advance(1, 32), None);
        // A cycle of one map goes to it again and again.
        let mut one = cycle(&[("only", 0, 0)]);
        assert_eq!(one.advance(1, 32).as_deref(), Some("only"));
        assert_eq!(one.advance(1, 32).as_deref(), Some("only"));
    }

    #[test]
    fn the_cycle_honours_the_player_range_of_a_line() {
        // `big` wants 8 players: with 2 the cycle skips it.
        let mut c = cycle(&[("a", 0, 0), ("big", 8, 0), ("c", 0, 0)]);
        assert_eq!(c.advance(2, 32).as_deref(), Some("a"));
        assert_eq!(c.advance(2, 32).as_deref(), Some("c"));
        assert_eq!(c.advance(2, 32).as_deref(), Some("a"));
        // With 9 it is taken.
        let mut c = cycle(&[("a", 0, 0), ("big", 8, 0), ("c", 0, 0)]);
        c.next = 1;
        assert_eq!(c.advance(9, 32).as_deref(), Some("big"));
        // `small` takes at most 4.
        let mut c = cycle(&[("small", 0, 4), ("c", 0, 0), ("d", 0, 0)]);
        assert_eq!(c.advance(6, 32).as_deref(), Some("c"));
        assert_eq!(c.advance(3, 32).as_deref(), Some("d"));
        // Nothing fits: the map the cycle stood at.
        let mut c = cycle(&[("x", 9, 0), ("y", 9, 0), ("z", 9, 0)]);
        assert_eq!(c.advance(1, 32).as_deref(), Some("x"));
    }

    #[test]
    fn map_cycle_lines_are_read_like_the_original() {
        assert_eq!(tokenize_line("de_dust2"), ["de_dust2"]);
        assert_eq!(
            tokenize_line("  cs_office  \\minplayers\\4\\maxplayers\\8 // comment"),
            ["cs_office", "\\minplayers\\4\\maxplayers\\8"]
        );
        assert_eq!(tokenize_line("\"de_nuke\""), ["de_nuke"]);
        assert!(tokenize_line("// only a comment").is_empty());
        assert_eq!(info_number("\\minplayers\\4\\maxplayers\\8", "maxplayers"), 8);
        assert_eq!(info_number("\\minplayers\\4", "maxplayers"), 0);
        assert_eq!(info_number("", "minplayers"), 0);
    }

    #[test]
    fn a_map_cycle_loads_the_valid_maps_only() {
        let vfs = cs_vfs::Vfs::from_memory([
            (
                "mapcycle.txt".to_string(),
                b"de_dust2\ncs_gone\n\\bad\nde_nuke \\minplayers\\2\n".to_vec(),
            ),
            ("maps/de_dust2.bsp".to_string(), Vec::new()),
            ("maps/de_nuke.bsp".to_string(), Vec::new()),
        ]);
        let c = MapCycle::load(&vfs);
        let maps: Vec<_> = c.items.iter().map(|i| i.map.as_str()).collect();
        assert_eq!(maps, ["de_dust2", "de_nuke"]);
        assert_eq!(c.items[1].min_players, 2);
        assert!(MapCycle::load(&cs_vfs::Vfs::from_memory([])).items.is_empty());
    }
}
