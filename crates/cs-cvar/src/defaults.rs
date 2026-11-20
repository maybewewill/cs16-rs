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
    reg.register("sv_cheats", "0", srv, "Allow cheat commands (0 or 1)");
    reg.register("mp_roundtime", "1.75", srv_arch, "Round duration in minutes");
    reg.register("mp_freezetime", "6", srv_arch, "Freeze time at round start (seconds)");
    reg.register("mp_startmoney", "800", srv_arch, "Starting money per half");
    reg.register("mp_c4timer", "45", srv_arch, "C4 detonation timer (seconds)");
    reg.register("mp_friendlyfire", "0", srv_arch, "Friendly fire (0 or 1)");
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
