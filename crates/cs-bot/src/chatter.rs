// SPDX-License-Identifier: GPL-3.0-or-later
//! Condition Zero / CS 1.6 `BotChatter.db` parser and radio throttling metadata
//! (`ReGameDLL_CS` `bot/cs_bot_chatter.cpp`).
//!
//! Parses Valve's `BotChatter.db` when mounted in VFS (`BotChatter.db` / `cstrike/BotChatter.db`)
//! and provides authentic built-in CS 1.6 radio fallback priorities and repeat cooldowns so bot
//! teams communicate tactical events (`SpotEnemy`, `BombPlanted`, `BombDefusing`, `KilledEnemy`,
//! `PinnedDown`, `Clear`, etc.) without spamming every tick.

use std::collections::HashMap;

use crate::tactics::{BotChatterEvent, RadioMessage};

/// Priority level of a `BotChatter.db` event (`BotChatterInterface`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChatterPriority {
    Low = 0,
    Normal = 1,
    High = 2,
    Critical = 3,
}

/// Configured scheduling rules for a single [`BotChatterEvent`].
#[derive(Debug, Clone, PartialEq)]
pub struct ChatterRule {
    pub event: BotChatterEvent,
    pub priority: ChatterPriority,
    /// Minimum seconds between two broadcasts of this event by the same team.
    pub repeat_cooldown: f32,
    /// Canonical CS 1.6 radio fallback (`%!MRAD_*`).
    pub radio_fallback: RadioMessage,
    /// Optional sample phrases parsed from `BotChatter.db` (`Speak` lines).
    pub phrases: Vec<String>,
}

/// Parsed `BotChatter.db` ruleset and place-chatter dictionary.
#[derive(Debug, Clone)]
pub struct BotChatterDb {
    rules: HashMap<BotChatterEvent, ChatterRule>,
    places: HashMap<String, Vec<String>>,
}

impl Default for BotChatterDb {
    fn default() -> Self {
        Self::builtin()
    }
}

impl BotChatterDb {
    /// Built-in CS 1.6 / Condition Zero chatter priorities, cooldowns, and `%!MRAD_*` mappings.
    pub fn builtin() -> Self {
        let mut rules = HashMap::new();
        for (event, priority, repeat_cooldown, radio_fallback) in [
            (
                BotChatterEvent::SpotEnemy,
                ChatterPriority::High,
                5.0,
                RadioMessage::EnemySpotted,
            ),
            (
                BotChatterEvent::BombPlanted,
                ChatterPriority::Critical,
                6.0,
                RadioMessage::GoGoGo,
            ),
            (
                BotChatterEvent::BombDefusing,
                ChatterPriority::Critical,
                4.5,
                RadioMessage::CoverMe,
            ),
            (
                BotChatterEvent::GuardingBomb,
                ChatterPriority::Normal,
                8.0,
                RadioMessage::InPosition,
            ),
            (
                BotChatterEvent::HostageTaken,
                ChatterPriority::High,
                5.0,
                RadioMessage::FollowMe,
            ),
            (
                BotChatterEvent::VipSpotted,
                ChatterPriority::High,
                5.0,
                RadioMessage::EnemySpotted,
            ),
            (
                BotChatterEvent::HeardNoise,
                ChatterPriority::Normal,
                6.0,
                RadioMessage::NeedBackup,
            ),
            (
                BotChatterEvent::PinnedDown,
                ChatterPriority::High,
                4.5,
                RadioMessage::TakingFire,
            ),
            (
                BotChatterEvent::Blind,
                ChatterPriority::Normal,
                4.0,
                RadioMessage::TakingFire,
            ),
            (
                BotChatterEvent::KilledEnemy,
                ChatterPriority::Normal,
                3.5,
                RadioMessage::EnemyDown,
            ),
            (
                BotChatterEvent::Clear,
                ChatterPriority::Low,
                8.0,
                RadioMessage::SectorClear,
            ),
            (
                BotChatterEvent::ReportingIn,
                ChatterPriority::Low,
                6.0,
                RadioMessage::ReportingIn,
            ),
        ] {
            rules.insert(
                event,
                ChatterRule {
                    event,
                    priority,
                    repeat_cooldown,
                    radio_fallback,
                    phrases: Vec::new(),
                },
            );
        }
        Self {
            rules,
            places: HashMap::new(),
        }
    }

    /// Parses a Valve `BotChatter.db` text file, merging discovered `Chatter`/`Radio` and `Place`
    /// blocks over [`Self::builtin`]. Unknown blocks/keys are safely skipped so retail CZ and
    /// custom `BotChatter.db` files never fail to load.
    pub fn parse(input: &str) -> Self {
        let mut db = Self::builtin();
        enum Block {
            Chatter(BotChatterEvent),
            Place(String),
            Other,
        }
        let mut current: Option<Block> = None;

        for raw in input.lines() {
            let line = match raw.find("//") {
                Some(idx) => &raw[..idx],
                None => raw,
            }
            .trim();
            if line.is_empty() {
                continue;
            }
            if line.eq_ignore_ascii_case("End") {
                current = None;
                continue;
            }
            if let Some(block) = current.as_mut() {
                match block {
                    Block::Chatter(ev) => {
                        if let Some((k, v)) = split_kv(line) {
                            if let Some(rule) = db.rules.get_mut(ev) {
                                if k.eq_ignore_ascii_case("Priority") {
                                    rule.priority = parse_priority(v);
                                } else if k.eq_ignore_ascii_case("Repeat")
                                    && let Ok(secs) = v.parse::<f32>()
                                    && secs.is_finite()
                                    && secs >= 0.0
                                {
                                    rule.repeat_cooldown = secs;
                                } else if k.eq_ignore_ascii_case("Radio")
                                    && let Some(msg) = RadioMessage::from_alias(v)
                                {
                                    rule.radio_fallback = msg;
                                }
                            }
                        } else if let Some(phrase) = parse_quoted_sample(line)
                            && let Some(rule) = db.rules.get_mut(ev)
                        {
                            rule.phrases.push(phrase);
                        }
                    }
                    Block::Place(place_name) => {
                        if let Some(phrase) = parse_quoted_sample(line) {
                            db.places
                                .entry(place_name.to_ascii_lowercase())
                                .or_default()
                                .push(phrase);
                        }
                    }
                    Block::Other => {}
                }
                continue;
            }

            let mut parts = line.split_whitespace();
            let Some(kind) = parts.next() else {
                continue;
            };
            let rest = parts.collect::<Vec<_>>().join(" ");
            let name = rest.trim_matches('"').trim();
            if kind.eq_ignore_ascii_case("Chatter") || kind.eq_ignore_ascii_case("Radio") {
                current =
                    map_chatter_name(name).map_or(Some(Block::Other), |e| Some(Block::Chatter(e)));
            } else if kind.eq_ignore_ascii_case("Place") && !name.is_empty() {
                current = Some(Block::Place(name.to_string()));
            } else if !kind.contains('=') {
                current = Some(Block::Other);
            }
        }
        db
    }

    /// Returns the scheduling rule for `event`.
    pub fn rule(&self, event: BotChatterEvent) -> &ChatterRule {
        &self.rules[&event]
    }

    /// Returns parsed place-callout audio paths for `place_name`, if defined in `BotChatter.db`.
    pub fn place_samples(&self, place_name: &str) -> &[String] {
        self.places
            .get(&place_name.to_ascii_lowercase())
            .map_or(&[], Vec::as_slice)
    }

    /// Minimum team-wide cooldown before repeating `event` (or default `3.0s` for raw radio).
    pub fn team_repeat_cooldown(&self, event: Option<BotChatterEvent>) -> f32 {
        event.map_or(3.0, |e| self.rule(e).repeat_cooldown)
    }

    /// Priority of `event` (`Normal` for raw radio calls without a chatter event).
    pub fn priority(&self, event: Option<BotChatterEvent>) -> ChatterPriority {
        event.map_or(ChatterPriority::Normal, |e| self.rule(e).priority)
    }
}

fn split_kv(line: &str) -> Option<(&str, &str)> {
    let (k, v) = line.split_once('=')?;
    Some((k.trim(), v.trim().trim_matches('"')))
}

fn parse_quoted_sample(line: &str) -> Option<String> {
    let first = line.find('"')?;
    let rest = &line[first + 1..];
    let second = rest.find('"')?;
    let sample = rest[..second].trim();
    (!sample.is_empty()).then(|| sample.to_string())
}

fn parse_priority(raw: &str) -> ChatterPriority {
    if raw.eq_ignore_ascii_case("Critical") || raw.eq_ignore_ascii_case("Essential") {
        ChatterPriority::Critical
    } else if raw.eq_ignore_ascii_case("High") {
        ChatterPriority::High
    } else if raw.eq_ignore_ascii_case("Low") {
        ChatterPriority::Low
    } else {
        ChatterPriority::Normal
    }
}

fn map_chatter_name(name: &str) -> Option<BotChatterEvent> {
    let key = name.trim().to_ascii_lowercase();
    Some(match key.as_str() {
        "spottedenemy" | "spottingtheenemy" | "enemy" | "spotenemy" | "spottedbomber" => {
            BotChatterEvent::SpotEnemy
        }
        "bombplanted" | "plantedbomb" | "thebombisplanted" => BotChatterEvent::BombPlanted,
        "defusingbomb" | "bombdefusing" | "coverme" => BotChatterEvent::BombDefusing,
        "guardingbomb" | "guardingdroppedc4" | "guardingbombsite" => BotChatterEvent::GuardingBomb,
        "hostagetaken" | "escortinghostages" => BotChatterEvent::HostageTaken,
        "vipspotted" | "spottedvip" => BotChatterEvent::VipSpotted,
        "heardnoise" | "heard" => BotChatterEvent::HeardNoise,
        "pinneddown" | "takingfire" | "help" => BotChatterEvent::PinnedDown,
        "blinded" | "blind" => BotChatterEvent::Blind,
        "killedmyenemy" | "killedenemy" | "enemydown" => BotChatterEvent::KilledEnemy,
        "clear" | "sectorclear" => BotChatterEvent::Clear,
        "reportingin" | "whereisthebomb" => BotChatterEvent::ReportingIn,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bot_chatter_db_blocks_and_preserves_builtin_fallbacks() {
        let sample = r#"
            // Sample Condition Zero BotChatter.db excerpt
            Chatter SpottedEnemy
                Priority = Critical
                Repeat = 7.5
                "radio/bot/spotted_a_bad_guy.wav"
            End

            Place BombsiteA
                "radio/bot/bombsite_a.wav"
            End
        "#;
        let db = BotChatterDb::parse(sample);
        let spot = db.rule(BotChatterEvent::SpotEnemy);
        assert_eq!(spot.priority, ChatterPriority::Critical);
        assert!((spot.repeat_cooldown - 7.5).abs() < 0.001);
        assert_eq!(spot.phrases, vec!["radio/bot/spotted_a_bad_guy.wav"]);
        assert_eq!(db.place_samples("bombsitea"), &["radio/bot/bombsite_a.wav"]);
        // Unmentioned events still keep authentic built-in rules.
        assert_eq!(
            db.rule(BotChatterEvent::BombDefusing).radio_fallback,
            RadioMessage::CoverMe
        );
    }
}
