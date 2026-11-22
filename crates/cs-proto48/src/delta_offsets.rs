//! Real C struct offsets for the fields a `delta.lst` describes.
//!
//! The original client stores every decoded delta field into its own C struct at the
//! `fieldOffset` the server sent in `svc_deltadescription`, so the offsets must be the real
//! `offsetof` values of the 32-bit (x86) structs, not invented ones. Fake offsets make the
//! client write values into the wrong members (for example `clientdata_t.origin[2]` onto
//! `viewmodel`), after which it crashes in `CL_GetModelByIndex`.
//!
//! Source of the field lists: `g_*DataDefinition` in ReHLDS `rehlds/engine/delta.cpp`
//! (read 2026-11-22), which holds `DELTA_DEF(struct, member)` = `{ name, offsetof(struct, member) }`.
//! Source of the layouts: the headers named at each table (ReHLDS `rehlds/common/`, read
//! 2026-11-22), computed with 32-bit MSVC rules: `int`/`float` 4, `short` 2, `byte`/`char` 1,
//! natural alignment, `vec3_t` = `float[3]`, `color24` = `byte[3]`, `MAX_PHYSINFO_STRING` = 256
//! (`pm_info.h`). Each table entry is `(field name, offset, size of the member in bytes)`;
//! an array element is named with its index (`origin[2]`) and sized as one element.
//!
//! What the server sends as `fieldSize` is not the member size: ReHLDS `DELTA_ParseField`
//! (`delta.cpp`, `pField->delta->fieldSize = 1`) sets it to 1 for every field parsed from a
//! `delta.lst`, so we send 1 too. The sizes here document the layout and are checked by the
//! tests.

/// Real layout of one delta field in its C struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldLayout {
    /// `offsetof(struct, member)` on 32-bit x86.
    pub offset: u16,
    /// `sizeof` the member (or of one array element) in bytes.
    pub size: u16,
}

/// `delta_description_s`, ReHLDS `rehlds/engine/delta.h` (the meta delta): `fieldName` is
/// `char[32]`, `fieldSize` and `flags` are `short`.
const DELTA_DESCRIPTION_FIELDS: &[(&str, u16, u16)] = &[
    ("fieldType", 0, 4),
    ("fieldName", 4, 32),
    ("fieldOffset", 36, 4),
    ("fieldSize", 40, 2),
    ("significant_bits", 44, 4),
    ("premultiply", 48, 4),
    ("postmultiply", 52, 4),
    ("flags", 56, 2),
];

/// `entity_state_s`, rehlds/common/entity_state.h; C struct size 340 bytes.
const ENTITY_STATE_FIELDS: &[(&str, u16, u16)] = &[
    ("startpos[0]", 228, 4),
    ("startpos[1]", 232, 4),
    ("startpos[2]", 236, 4),
    ("endpos[0]", 240, 4),
    ("endpos[1]", 244, 4),
    ("endpos[2]", 248, 4),
    ("impacttime", 252, 4),
    ("starttime", 256, 4),
    ("origin[0]", 16, 4),
    ("origin[1]", 20, 4),
    ("origin[2]", 24, 4),
    ("angles[0]", 28, 4),
    ("angles[1]", 32, 4),
    ("angles[2]", 36, 4),
    ("modelindex", 40, 4),
    ("frame", 48, 4),
    ("movetype", 88, 4),
    ("colormap", 52, 4),
    ("skin", 56, 2),
    ("solid", 58, 2),
    ("scale", 64, 4),
    ("effects", 60, 4),
    ("sequence", 44, 4),
    ("animtime", 92, 4),
    ("framerate", 96, 4),
    ("controller[0]", 104, 1),
    ("controller[1]", 105, 1),
    ("controller[2]", 106, 1),
    ("controller[3]", 107, 1),
    ("blending[0]", 108, 1),
    ("blending[1]", 109, 1),
    ("body", 100, 4),
    ("owner", 152, 4),
    ("rendermode", 72, 4),
    ("renderamt", 76, 4),
    ("renderfx", 84, 4),
    ("rendercolor.r", 80, 1),
    ("rendercolor.g", 81, 1),
    ("rendercolor.b", 82, 1),
    ("weaponmodel", 180, 4),
    ("gaitsequence", 184, 4),
    ("mins[0]", 124, 4),
    ("mins[1]", 128, 4),
    ("mins[2]", 132, 4),
    ("maxs[0]", 136, 4),
    ("maxs[1]", 140, 4),
    ("maxs[2]", 144, 4),
    ("aiment", 148, 4),
    ("basevelocity[0]", 188, 4),
    ("basevelocity[1]", 192, 4),
    ("basevelocity[2]", 196, 4),
    ("friction", 156, 4),
    ("gravity", 160, 4),
    ("spectator", 176, 4),
    ("velocity[0]", 112, 4),
    ("velocity[1]", 116, 4),
    ("velocity[2]", 120, 4),
    ("team", 164, 4),
    ("playerclass", 168, 4),
    ("health", 172, 4),
    ("usehull", 200, 4),
    ("oldbuttons", 204, 4),
    ("onground", 208, 4),
    ("iStepLeft", 212, 4),
    ("flFallVelocity", 216, 4),
    ("weaponanim", 224, 4),
    ("eflags", 68, 1),
    ("iuser1", 260, 4),
    ("iuser2", 264, 4),
    ("iuser3", 268, 4),
    ("iuser4", 272, 4),
    ("fuser1", 276, 4),
    ("fuser2", 280, 4),
    ("fuser3", 284, 4),
    ("fuser4", 288, 4),
    ("vuser1[0]", 292, 4),
    ("vuser1[1]", 296, 4),
    ("vuser1[2]", 300, 4),
    ("vuser2[0]", 304, 4),
    ("vuser2[1]", 308, 4),
    ("vuser2[2]", 312, 4),
    ("vuser3[0]", 316, 4),
    ("vuser3[1]", 320, 4),
    ("vuser3[2]", 324, 4),
    ("vuser4[0]", 328, 4),
    ("vuser4[1]", 332, 4),
    ("vuser4[2]", 336, 4),
];
/// `clientdata_s`, rehlds/common/entity_state.h; C struct size 476 bytes.
const CLIENT_DATA_FIELDS: &[(&str, u16, u16)] = &[
    ("origin[0]", 0, 4),
    ("origin[1]", 4, 4),
    ("origin[2]", 8, 4),
    ("velocity[0]", 12, 4),
    ("velocity[1]", 16, 4),
    ("velocity[2]", 20, 4),
    ("viewmodel", 24, 4),
    ("punchangle[0]", 28, 4),
    ("punchangle[1]", 32, 4),
    ("punchangle[2]", 36, 4),
    ("flags", 40, 4),
    ("waterlevel", 44, 4),
    ("watertype", 48, 4),
    ("view_ofs[0]", 52, 4),
    ("view_ofs[1]", 56, 4),
    ("view_ofs[2]", 60, 4),
    ("health", 64, 4),
    ("bInDuck", 68, 4),
    ("weapons", 72, 4),
    ("flTimeStepSound", 76, 4),
    ("flDuckTime", 80, 4),
    ("flSwimTime", 84, 4),
    ("waterjumptime", 88, 4),
    ("maxspeed", 92, 4),
    ("m_iId", 104, 4),
    ("ammo_nails", 112, 4),
    ("ammo_shells", 108, 4),
    ("ammo_cells", 116, 4),
    ("ammo_rockets", 120, 4),
    ("m_flNextAttack", 124, 4),
    ("physinfo", 140, 256),
    ("fov", 96, 4),
    ("weaponanim", 100, 4),
    ("tfstate", 128, 4),
    ("pushmsec", 132, 4),
    ("deadflag", 136, 4),
    ("iuser1", 396, 4),
    ("iuser2", 400, 4),
    ("iuser3", 404, 4),
    ("iuser4", 408, 4),
    ("fuser1", 412, 4),
    ("fuser2", 416, 4),
    ("fuser3", 420, 4),
    ("fuser4", 424, 4),
    ("vuser1[0]", 428, 4),
    ("vuser1[1]", 432, 4),
    ("vuser1[2]", 436, 4),
    ("vuser2[0]", 440, 4),
    ("vuser2[1]", 444, 4),
    ("vuser2[2]", 448, 4),
    ("vuser3[0]", 452, 4),
    ("vuser3[1]", 456, 4),
    ("vuser3[2]", 460, 4),
    ("vuser4[0]", 464, 4),
    ("vuser4[1]", 468, 4),
    ("vuser4[2]", 472, 4),
];
/// `weapon_data_s`, rehlds/common/weaponinfo.h; C struct size 88 bytes.
const WEAPON_DATA_FIELDS: &[(&str, u16, u16)] = &[
    ("m_iId", 0, 4),
    ("m_iClip", 4, 4),
    ("m_flNextPrimaryAttack", 8, 4),
    ("m_flNextSecondaryAttack", 12, 4),
    ("m_flTimeWeaponIdle", 16, 4),
    ("m_fInReload", 20, 4),
    ("m_fInSpecialReload", 24, 4),
    ("m_flNextReload", 28, 4),
    ("m_flPumpTime", 32, 4),
    ("m_fReloadTime", 36, 4),
    ("m_fAimedDamage", 40, 4),
    ("m_fNextAimBonus", 44, 4),
    ("m_fInZoom", 48, 4),
    ("m_iWeaponState", 52, 4),
    ("iuser1", 56, 4),
    ("iuser2", 60, 4),
    ("iuser3", 64, 4),
    ("iuser4", 68, 4),
    ("fuser1", 72, 4),
    ("fuser2", 76, 4),
    ("fuser3", 80, 4),
    ("fuser4", 84, 4),
];
/// `usercmd_s`, rehlds/common/usercmd.h; C struct size 52 bytes.
const USERCMD_FIELDS: &[(&str, u16, u16)] = &[
    ("lerp_msec", 0, 2),
    ("msec", 2, 1),
    ("lightlevel", 28, 1),
    ("viewangles[0]", 4, 4),
    ("viewangles[1]", 8, 4),
    ("viewangles[2]", 12, 4),
    ("buttons", 30, 2),
    ("forwardmove", 16, 4),
    ("sidemove", 20, 4),
    ("upmove", 24, 4),
    ("impulse", 32, 1),
    ("weaponselect", 33, 1),
    ("impact_index", 36, 4),
    ("impact_position[0]", 40, 4),
    ("impact_position[1]", 44, 4),
    ("impact_position[2]", 48, 4),
];
/// `event_args_s`, rehlds/common/event_args.h; C struct size 72 bytes.
const EVENT_ARGS_FIELDS: &[(&str, u16, u16)] = &[
    ("entindex", 4, 4),
    ("origin[0]", 8, 4),
    ("origin[1]", 12, 4),
    ("origin[2]", 16, 4),
    ("angles[0]", 20, 4),
    ("angles[1]", 24, 4),
    ("angles[2]", 28, 4),
    ("fparam1", 48, 4),
    ("fparam2", 52, 4),
    ("iparam1", 56, 4),
    ("iparam2", 60, 4),
    ("bparam1", 64, 4),
    ("bparam2", 68, 4),
    ("ducking", 44, 4),
];

/// Table for a `delta.lst` section name. `entity_state_t`, `entity_state_player_t` and
/// `custom_entity_state_t` are all the C struct `entity_state_t`; the meta delta is
/// `delta_description_t`. `None` for a name the engine does not describe.
fn table_for(struct_name: &str) -> Option<&'static [(&'static str, u16, u16)]> {
    Some(match struct_name {
        "event_t" => EVENT_ARGS_FIELDS,
        "entity_state_t" | "entity_state_player_t" | "custom_entity_state_t" => ENTITY_STATE_FIELDS,
        "usercmd_t" => USERCMD_FIELDS,
        "weapon_data_t" => WEAPON_DATA_FIELDS,
        "clientdata_t" => CLIENT_DATA_FIELDS,
        "delta_description_t" => DELTA_DESCRIPTION_FIELDS,
        _ => return None,
    })
}

/// Real layout of `field` in the C struct behind the `delta.lst` section `struct_name`.
/// `None` when the struct or the field is unknown (the caller turns that into an error:
/// ReHLDS `DELTA_FindOffset` is a `Sys_Error` too). Field names compare case-insensitively
/// like ReHLDS `Q_stricmp`.
pub fn field_layout(struct_name: &str, field: &str) -> Option<FieldLayout> {
    table_for(struct_name)?
        .iter()
        .find(|(name, _, _)| name.eq_ignore_ascii_case(field))
        .map(|&(_, offset, size)| FieldLayout { offset, size })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn off(s: &str, f: &str) -> u16 {
        field_layout(s, f)
            .unwrap_or_else(|| panic!("{s}.{f} missing"))
            .offset
    }

    /// 32-bit layout of a header's member list `(type, name, count)`, independent of the tables.
    fn layout(members: &[(&str, &str, u16)]) -> Vec<(String, u16, u16)> {
        let mut out = Vec::new();
        let mut at = 0u16;
        for &(ty, name, count) in members {
            let size = match ty {
                "int" | "float" => 4,
                "short" => 2,
                "byte" | "char" => 1,
                other => panic!("type {other}"),
            };
            at = at.div_ceil(size) * size;
            out.push((name.to_string(), at, size * count));
            at += size * count;
        }
        out
    }

    /// Every member of the header struct that the table names must sit at the computed
    /// offset, and every table entry must name a member of the struct.
    fn check_against_header(struct_name: &str, members: &[(&str, &str, u16)]) {
        let computed = layout(members);
        let find = |m: &str| computed.iter().find(|(n, _, _)| n == m).cloned();
        for &(name, offset, size) in table_for(struct_name).unwrap() {
            let (base, index) = if let Some((b, i)) = name.split_once('[') {
                (b, i.trim_end_matches(']').parse::<u16>().unwrap())
            } else if let Some((b, c)) = name.split_once('.') {
                (b, u16::try_from("rgb".find(c).unwrap()).unwrap()) // `color24`: r, g, b
            } else {
                (name, 0)
            };
            let (_, base_off, base_size) =
                find(base).unwrap_or_else(|| panic!("{struct_name}.{name} not in the header"));
            let count = members.iter().find(|m| m.1 == base).unwrap().2;
            let elem = base_size / count;
            if name.contains('[') || name.contains('.') {
                assert_eq!((offset, size), (base_off + index * elem, elem), "{name}");
            } else {
                assert_eq!((offset, size), (base_off, base_size), "{name}");
            }
        }
    }

    // rehlds/common/entity_state.h, `clientdata_s` (read 2026-11-22), `vec3_t` = float[3].
    const CLIENTDATA: &[(&str, &str, u16)] = &[
        ("float", "origin", 3),
        ("float", "velocity", 3),
        ("int", "viewmodel", 1),
        ("float", "punchangle", 3),
        ("int", "flags", 1),
        ("int", "waterlevel", 1),
        ("int", "watertype", 1),
        ("float", "view_ofs", 3),
        ("float", "health", 1),
        ("int", "bInDuck", 1),
        ("int", "weapons", 1),
        ("int", "flTimeStepSound", 1),
        ("int", "flDuckTime", 1),
        ("int", "flSwimTime", 1),
        ("int", "waterjumptime", 1),
        ("float", "maxspeed", 1),
        ("float", "fov", 1),
        ("int", "weaponanim", 1),
        ("int", "m_iId", 1),
        ("int", "ammo_shells", 1),
        ("int", "ammo_nails", 1),
        ("int", "ammo_cells", 1),
        ("int", "ammo_rockets", 1),
        ("float", "m_flNextAttack", 1),
        ("int", "tfstate", 1),
        ("int", "pushmsec", 1),
        ("int", "deadflag", 1),
        ("char", "physinfo", 256),
        ("int", "iuser1", 1),
        ("int", "iuser2", 1),
        ("int", "iuser3", 1),
        ("int", "iuser4", 1),
        ("float", "fuser1", 1),
        ("float", "fuser2", 1),
        ("float", "fuser3", 1),
        ("float", "fuser4", 1),
        ("float", "vuser1", 3),
        ("float", "vuser2", 3),
        ("float", "vuser3", 3),
        ("float", "vuser4", 3),
    ];

    // rehlds/common/weaponinfo.h, `weapon_data_s` (read 2026-11-22).
    const WEAPON_DATA: &[(&str, &str, u16)] = &[
        ("int", "m_iId", 1),
        ("int", "m_iClip", 1),
        ("float", "m_flNextPrimaryAttack", 1),
        ("float", "m_flNextSecondaryAttack", 1),
        ("float", "m_flTimeWeaponIdle", 1),
        ("int", "m_fInReload", 1),
        ("int", "m_fInSpecialReload", 1),
        ("float", "m_flNextReload", 1),
        ("float", "m_flPumpTime", 1),
        ("float", "m_fReloadTime", 1),
        ("float", "m_fAimedDamage", 1),
        ("float", "m_fNextAimBonus", 1),
        ("int", "m_fInZoom", 1),
        ("int", "m_iWeaponState", 1),
        ("int", "iuser1", 1),
        ("int", "iuser2", 1),
        ("int", "iuser3", 1),
        ("int", "iuser4", 1),
        ("float", "fuser1", 1),
        ("float", "fuser2", 1),
        ("float", "fuser3", 1),
        ("float", "fuser4", 1),
    ];

    #[test]
    fn golden_offsets_match_the_ida_confirmed_values() {
        // Confirmed in IDA on the original client's hw.dll.
        assert_eq!(off("entity_state_t", "modelindex"), 40);
        assert_eq!(off("clientdata_t", "viewmodel"), 24);
        // The crash case: `origin[2]` must not land on `viewmodel`.
        assert_eq!(off("clientdata_t", "origin[2]"), 8);
        assert_eq!(off("entity_state_player_t", "modelindex"), 40);
        assert_eq!(off("custom_entity_state_t", "modelindex"), 40);
        assert_eq!(off("entity_state_t", "origin[0]"), 16);
        assert_eq!(off("entity_state_t", "angles[2]"), 36);
        assert_eq!(off("entity_state_t", "rendercolor.b"), 82);
        assert_eq!(off("entity_state_t", "renderfx"), 84);
        assert_eq!(off("entity_state_t", "controller[3]"), 107);
        assert_eq!(off("entity_state_t", "iuser1"), 260);
        assert_eq!(off("entity_state_t", "vuser4[2]"), 336);
        assert_eq!(off("usercmd_t", "msec"), 2);
        assert_eq!(off("usercmd_t", "viewangles[0]"), 4);
        assert_eq!(off("usercmd_t", "buttons"), 30);
        assert_eq!(off("usercmd_t", "impact_position[2]"), 48);
        assert_eq!(off("event_t", "entindex"), 4);
        assert_eq!(off("event_t", "origin[0]"), 8);
        assert_eq!(off("event_t", "ducking"), 44);
        assert_eq!(off("weapon_data_t", "m_flTimeWeaponIdle"), 16);
        assert_eq!(off("clientdata_t", "physinfo"), 140);
        assert_eq!(field_layout("clientdata_t", "physinfo").unwrap().size, 256);
        assert_eq!(off("clientdata_t", "iuser1"), 396);
        assert_eq!(off("delta_description_t", "fieldOffset"), 36);
        // Case-insensitive like Q_stricmp; unknown names are None.
        assert_eq!(off("clientdata_t", "VIEWMODEL"), 24);
        assert!(field_layout("clientdata_t", "no_such_member").is_none());
        assert!(field_layout("no_such_struct_t", "origin[0]").is_none());
    }

    #[test]
    fn clientdata_tables_match_the_header_layout() {
        // The last member, `vuser4`, ends the 476-byte struct.
        assert_eq!(layout(CLIENTDATA).last().unwrap().1, 476 - 12);
        check_against_header("clientdata_t", CLIENTDATA);
        // Members `delta.lst` does not list are still in the table (the engine's definition has them).
        for name in ["viewmodel", "tfstate", "physinfo", "vuser4[2]", "fuser4"] {
            assert!(field_layout("clientdata_t", name).is_some(), "{name}");
        }
    }

    #[test]
    fn weapon_data_table_matches_the_header_layout() {
        check_against_header("weapon_data_t", WEAPON_DATA);
        // Every member of the struct is in the table.
        for &(_, name, _) in WEAPON_DATA {
            assert!(field_layout("weapon_data_t", name).is_some(), "{name}");
        }
    }

    #[test]
    fn entity_usercmd_event_tables_match_the_header_layout() {
        // rehlds/common/usercmd.h `usercmd_s`.
        check_against_header(
            "usercmd_t",
            &[
                ("short", "lerp_msec", 1),
                ("byte", "msec", 1),
                ("float", "viewangles", 3),
                ("float", "forwardmove", 1),
                ("float", "sidemove", 1),
                ("float", "upmove", 1),
                ("byte", "lightlevel", 1),
                ("short", "buttons", 1),
                ("byte", "impulse", 1),
                ("byte", "weaponselect", 1),
                ("int", "impact_index", 1),
                ("float", "impact_position", 3),
            ],
        );
        // rehlds/common/event_args.h `event_args_s`.
        check_against_header(
            "event_t",
            &[
                ("int", "flags", 1),
                ("int", "entindex", 1),
                ("float", "origin", 3),
                ("float", "angles", 3),
                ("float", "velocity", 3),
                ("int", "ducking", 1),
                ("float", "fparam1", 1),
                ("float", "fparam2", 1),
                ("int", "iparam1", 1),
                ("int", "iparam2", 1),
                ("int", "bparam1", 1),
                ("int", "bparam2", 1),
            ],
        );
        // rehlds/common/entity_state.h `entity_state_s` (`color24` = byte[3]).
        check_against_header(
            "entity_state_t",
            &[
                ("int", "entityType", 1),
                ("int", "number", 1),
                ("float", "msg_time", 1),
                ("int", "messagenum", 1),
                ("float", "origin", 3),
                ("float", "angles", 3),
                ("int", "modelindex", 1),
                ("int", "sequence", 1),
                ("float", "frame", 1),
                ("int", "colormap", 1),
                ("short", "skin", 1),
                ("short", "solid", 1),
                ("int", "effects", 1),
                ("float", "scale", 1),
                ("byte", "eflags", 1),
                ("int", "rendermode", 1),
                ("int", "renderamt", 1),
                ("byte", "rendercolor", 3),
                ("int", "renderfx", 1),
                ("int", "movetype", 1),
                ("float", "animtime", 1),
                ("float", "framerate", 1),
                ("int", "body", 1),
                ("byte", "controller", 4),
                ("byte", "blending", 4),
                ("float", "velocity", 3),
                ("float", "mins", 3),
                ("float", "maxs", 3),
                ("int", "aiment", 1),
                ("int", "owner", 1),
                ("float", "friction", 1),
                ("float", "gravity", 1),
                ("int", "team", 1),
                ("int", "playerclass", 1),
                ("int", "health", 1),
                ("int", "spectator", 1),
                ("int", "weaponmodel", 1),
                ("int", "gaitsequence", 1),
                ("float", "basevelocity", 3),
                ("int", "usehull", 1),
                ("int", "oldbuttons", 1),
                ("int", "onground", 1),
                ("int", "iStepLeft", 1),
                ("float", "flFallVelocity", 1),
                ("float", "fov", 1),
                ("int", "weaponanim", 1),
                ("float", "startpos", 3),
                ("float", "endpos", 3),
                ("float", "impacttime", 1),
                ("float", "starttime", 1),
                ("int", "iuser1", 1),
                ("int", "iuser2", 1),
                ("int", "iuser3", 1),
                ("int", "iuser4", 1),
                ("float", "fuser1", 1),
                ("float", "fuser2", 1),
                ("float", "fuser3", 1),
                ("float", "fuser4", 1),
                ("float", "vuser1", 3),
                ("float", "vuser2", 3),
                ("float", "vuser3", 3),
                ("float", "vuser4", 3),
            ],
        );
    }
}
