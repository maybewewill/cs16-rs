//! Standard Counter-Strike 1.6 CVars and classic default key bindings.

use crate::{CVarFlags, CVarRegistry};

pub fn register_default_cvars(reg: &mut CVarRegistry) {
    let arch = CVarFlags::ARCHIVE;
    let srv = CVarFlags::SERVER;
    let srv_arch = CVarFlags::SERVER | CVarFlags::ARCHIVE;

    // Player / Multiplayer
    reg.register("name", "Player", arch, "Player display name");
    reg.register("cl_crosshair_size", "medium", arch, "Crosshair size: auto, small, medium, large");
    reg.register("cl_crosshair_color", "50 250 50", arch, "Crosshair RGB color (0..255)");
    reg.register("cl_crosshair_translucent", "1", arch, "Translucent crosshair (0 or 1)");
    reg.register("cl_dynamiccrosshair", "1", arch, "Dynamic crosshair spread (0 or 1)");
    reg.register("cl_righthand", "1", arch, "Hold weapon in right hand (1) or left hand (0)");
    reg.register("cl_hd_models", "0", arch, "Enable cstrike_hd high-definition models (0 or 1)");
    reg.register("hud_fastswitch", "1", arch, "Fast weapon switch on slot press (0 or 1)");
    reg.register("_vgui_menus", "1", arch, "Use VGUI buy/team menus (1) or old text menus (0)");

    // Mouse / Input
    reg.register("sensitivity", "3.0", arch, "Mouse sensitivity");
    reg.register("m_rawinput", "1", arch, "Use raw mouse input (0 or 1)");
    reg.register("m_yaw", "0.022", arch, "Mouse yaw multiplier (degrees per count)");
    reg.register("m_pitch", "0.022", arch, "Mouse pitch multiplier (negative inverts Y)");
    reg.register("m_filter", "0", arch, "Mouse smoothing filter (0 or 1)");

    // Video / Renderer (GoldSrc look)
    reg.register("fps_max", "100", arch, "Maximum framerate limit (0 = unlimited)");
    reg.register("gamma", "2.5", arch, "Display gamma (GoldSrc default 2.5)");
    reg.register("texgamma", "2.0", arch, "Texture gamma");
    reg.register("brightness", "1.0", arch, "Display brightness multiplier");
    reg.register(
        "gl_texturemode",
        "GL_LINEAR_MIPMAP_LINEAR",
        arch,
        "Texture filtering: GL_LINEAR_MIPMAP_LINEAR or GL_NEAREST",
    );
    reg.register(
        "vid_aspect_mode",
        "widescreen",
        arch,
        "Viewport aspect: widescreen, 4:3_bars, 4:3_stretch",
    );
    reg.register("fov_default", "90", arch, "Horizontal field of view (degrees)");
    reg.register("net_graph", "0", arch, "Performance overlay (0=off, 1=stats, 3=compact)");
    reg.register("net_graphpos", "1", arch, "net_graph screen alignment (1=right, 2=center, 3=left)");
    reg.register("r_drawviewmodel", "1", arch, "Draw first-person weapon model (0 or 1)");
    reg.register("r_vis", "1", CVarFlags::NONE, "Enable BSP PVS visibility culling (0 or 1)");
    reg.register("r_lightmap_only", "0", CVarFlags::CHEAT, "Render lightmaps only (0 or 1)");
    reg.register("r_fullbright", "0", CVarFlags::CHEAT, "Disable lightmaps (0 or 1)");
    reg.register("r_noclip_speed", "450", CVarFlags::NONE, "Free-camera flight speed (units/sec)");

    // Audio
    reg.register("volume", "0.8", arch, "Master sound effects volume (0.0..1.0)");
    reg.register("mp3volume", "0.5", arch, "Menu / MP3 music volume (0.0..1.0)");

    // Server / Physics (exact CS 1.6 defaults)
    reg.register("hostname", "Counter-Strike 1.6 Rust", srv_arch, "Server name");
    reg.register("sv_gravity", "800", srv, "World gravity (units/sec^2)");
    reg.register("sv_maxspeed", "320", srv, "Maximum player movement speed");
    reg.register("sv_accelerate", "5", srv, "Ground acceleration");
    reg.register("sv_airaccelerate", "10", srv, "Air acceleration");
    reg.register("sv_friction", "4", srv, "Ground friction");
    reg.register("sv_stopspeed", "100", srv, "Minimum stopping speed");
    reg.register("sv_stepsize", "18", srv, "Maximum stair step height");
    // Set by the game's world (`CWorld`) from the map's worldspawn keys; the engine sends them in
    // the movevars. ReHLDS `sv_main.cpp` (read 2026-11-22): `sv_skyname` "desert", `sv_zmax`
    // 4096 (FCVAR_SPONLY there, a flag this registry does not have), `sv_wateramp` 0.
    reg.register("sv_skyname", "desert", CVarFlags::NONE, "Sky box name");
    reg.register("sv_zmax", "4096", CVarFlags::NONE, "Far clip distance");
    reg.register("sv_wateramp", "0", CVarFlags::NONE, "Water wave height");
    // ReGameDLL `game.cpp` (read 2026-11-22): `showtriggers` "0". The defaults of `room_type`
    // (client DSP preset) and `sv_language` (assumption, from memory: 0 is English) are not
    // checked against a source.
    reg.register("showtriggers", "0", CVarFlags::NONE, "Show trigger brushes (debug)");
    reg.register("room_type", "0", CVarFlags::NONE, "DSP room type");
    reg.register("sv_language", "0", CVarFlags::NONE, "Server language (1 is German)");
    reg.register("sv_maxvelocity", "2000", CVarFlags::NONE, "Maximum speed of any entity on an axis");
    reg.register("sv_bounce", "1", srv, "Bounce of a walking entity off a wall");
    reg.register("sv_cheats", "0", srv, "Allow cheat commands (0 or 1)");
    reg.register("sv_unlag", "1", CVarFlags::NONE, "Lag compensation of the players' shots (0 or 1)");
    reg.register("sv_maxunlag", "0.5", CVarFlags::NONE, "Longest time lag compensation goes back (seconds, 0 for no limit)");
    reg.register("sv_unlagpush", "0", CVarFlags::NONE, "Seconds added to the time lag compensation goes back to");
    reg.register("sv_unlagsamples", "1", CVarFlags::NONE, "Updates averaged for the latency lag compensation uses (1..16)");
    reg.register("mp_roundtime", "1.75", srv_arch, "Round duration in minutes");
    reg.register("mp_freezetime", "6", srv_arch, "Freeze time at round start (seconds)");
    reg.register("mp_startmoney", "800", srv_arch, "Starting money per half");
    reg.register("mp_c4timer", "45", srv_arch, "C4 detonation timer (seconds)");
    reg.register("mp_friendlyfire", "0", srv_arch, "Friendly fire (0 or 1)");
    // ReGameDLL `game.cpp` (read 2026-11-22): the team rules.
    reg.register("mp_limitteams", "2", srv, "Most players one team may have over the other (0 for no limit)");
    reg.register("mp_autoteambalance", "1", srv, "Move players between teams at the end of a round to even them out");
    // ReGameDLL `game.cpp` (read 2026-11-22): the team-kill rules.
    reg.register("mp_tkpunish", "0", srv, "Kill a player at the start of the next round for killing a teammate (0 or 1)");
    reg.register("mp_autokick", "1", srv, "Kick a player that kills three teammates (0 or 1)");
    reg.register("allow_spectators", "1.0", srv, "Players may join the spectators (0 or 1)");
    reg.register("humans_join_team", "any", srv, "Team humans must join: any, T or CT");
    // ReGameDLL `game.cpp` (read 2026-11-22): the round and match rules. `mp_roundtime` and
    // `mp_freezetime` are registered above (the legacy offline game reads them too).
    reg.register("sv_restart", "0", srv, "Restart the round in this many seconds (the score is reset)");
    reg.register("sv_restartround", "0", srv, "Restart the round in this many seconds (the score is reset)");
    reg.register("mp_maxrounds", "0", srv, "Change the map after this many rounds (0 for no limit)");
    reg.register("mp_winlimit", "0", srv, "Change the map when a team has won this many rounds (0 for no limit)");
    reg.register("mp_windifference", "1", srv, "Rounds a team must lead by to win at the win limit");
    reg.register("mp_timelimit", "0", srv, "Change the map after this many minutes (0 for no limit)");
    reg.register("mp_chattime", "10", srv, "Seconds the scoreboard is shown when the map ends");
    // The cvars `InitHUD` sends to a joining client (ReGameDLL `game.cpp`, read 2026-11-22).
    reg.register("mp_forcecamera", "0", srv, "Dead players see through teammates only");
    reg.register("mp_forcechasecam", "0", srv, "Dead players follow their own team only");
    reg.register("mp_fadetoblack", "0", srv, "Fade a dead player's screen to black (1 stays black)");
    // `game.cpp`: `{ "mp_playerid", "0", FCVAR_SERVER }`; 0 names everyone, 1 teammates only, 2 nobody.
    reg.register("mp_playerid", "0", srv, "Who the status bar names (0 all, 1 teammates, 2 nobody)");
    reg.register("bot_quota", "9", srv_arch, "Target number of bots in local match (0..15)");
    reg.register("bot_difficulty", "1", srv_arch, "Bot skill level (0=Easy, 1=Normal, 2=Hard, 3=Expert)");
}

pub fn register_default_binds(reg: &mut CVarRegistry) {
    let defaults = [
        ("w", "+forward"),
        ("s", "+back"),
        ("a", "+moveleft"),
        ("d", "+moveright"),
        ("space", "+jump"),
        ("ctrl", "+duck"),
        ("shift", "+speed"),
        ("e", "+use"),
        ("r", "+reload"),
        ("g", "drop"),
        ("b", "buy"),
        ("q", "lastinv"),
        ("tab", "+showscores"),
        ("m", "chooseteam"),
        ("y", "messagemode"),
        ("u", "messagemode2"),
        ("n", "nightvision"),
        ("z", "radio1"),
        ("x", "radio2"),
        ("c", "radio3"),
        ("t", "impulse 201"),
        (",", "buyammo1"),
        (".", "buyammo2"),
        ("1", "slot1"),
        ("2", "slot2"),
        ("3", "slot3"),
        ("4", "slot4"),
        ("5", "slot5"),
        ("mouse1", "+attack"),
        ("mouse2", "+attack2"),
        ("f5", "screenshot"),
        ("`", "toggleconsole"),
        ("escape", "cancelselect"),
    ];
    for (key, cmd) in defaults {
        reg.bind(key, cmd);
    }
}
