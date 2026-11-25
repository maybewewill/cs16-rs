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
    reg.register("_vgui_menus", "1", arch, "Ask the server for VGUI menus (1) or the text menus this client draws (0)");
    reg.register("hud_centerid", "0", arch, "Center player target ID status bar text (0 or 1)");
    reg.register("hud_draw", "1", CVarFlags::NONE, "Draw in-game HUD overlay (0 or 1)");
    reg.register("hud_saytext_internal", "1", arch, "Show in-game chat saytext lines (0 or 1)");
    reg.register("hud_saytext_time", "5", arch, "Duration chat saytext lines stay visible (seconds)");
    reg.register("hud_deathnotice_time", "6", CVarFlags::NONE, "Duration death notice entries stay visible (seconds)");
    reg.register("hud_takesshots", "0", arch, "Automatically take end-of-map scoreboard screenshots (0 or 1)");
    reg.register("scr_centertime", "2", CVarFlags::NONE, "Duration centerprint messages stay visible (seconds)");
    reg.register("scoreboard_showhealth", "1", arch, "Show player health column on scoreboard (0 or 1)");
    reg.register("scoreboard_showmoney", "1", arch, "Show teammate money column on scoreboard (0 or 1)");
    reg.register("scoreboard_shortheaders", "0", arch, "Use short column headers on scoreboard (0 or 1)");
    reg.register("scoreboard_showavatars", "1", arch, "Show Steam avatars on scoreboard (0 or 1)");
    reg.register("con_color", "255 180 30", arch, "Console text RGB color");
    reg.register("cl_radartype", "0", arch, "Radar background style (0=translucent, 1=solid/overview)");
    reg.register("cl_observercrosshair", "1", arch, "Show crosshair while spectating (0 or 1)");
    reg.register("spec_autodirector", "1", arch, "Spectator auto-director target selection (0 or 1)");
    reg.register("spec_drawstatus", "1", arch, "Draw spectator status bar (0 or 1)");
    reg.register("spec_drawcone", "1", arch, "Draw spectator overview view cones (0 or 1)");
    reg.register("spec_drawnames", "1", arch, "Draw player names in spectator overview (0 or 1)");
    reg.register("spec_pip", "0", arch, "Spectator picture-in-picture mode (0..3)");
    reg.register("cl_language", "english", arch, "Client localization language (english, russian, german, ...)");
    reg.register("violence_hblood", "1", arch, "Show human blood effects (0 or 1)");
    reg.register("violence_ablood", "1", arch, "Show alien blood effects (0 or 1)");
    reg.register("violence_hgibs", "1", arch, "Show human gib effects (0 or 1)");
    reg.register("violence_agibs", "1", arch, "Show alien gib effects (0 or 1)");
    reg.register("cl_forwardspeed", "400", arch, "Forward move speed (+forward)");
    reg.register("cl_backspeed", "400", arch, "Backward move speed (+back)");
    reg.register("cl_sidespeed", "400", arch, "Side move speed (+moveleft, +moveright)");
    reg.register("cl_upspeed", "320", arch, "Vertical move speed (+moveup, +movedown)");
    reg.register("cl_movespeedkey", "0.52", arch, "Factor of the move speeds while +speed (walk) is held");

    // Mouse / Input
    reg.register("sensitivity", "3.0", arch, "Mouse sensitivity");
    reg.register("zoom_sensitivity_ratio", "1.2", arch, "Zoom sensitivity multiplier relative to hipfire sensitivity");
    reg.register("m_rawinput", "1", arch, "Use raw mouse input (0 or 1)");
    reg.register("m_yaw", "0.022", arch, "Mouse yaw multiplier (degrees per count)");
    reg.register("m_pitch", "0.022", arch, "Mouse pitch multiplier (negative inverts Y)");
    reg.register("m_filter", "0", arch, "Mouse smoothing filter (0 or 1)");
    reg.register("m_customaccel", "0", arch, "Custom mouse acceleration mode (0=off, 1=linear, 2=separate X/Y, 3=pow)");
    reg.register("m_customaccel_scale", "0.04", arch, "Custom mouse acceleration scale");
    reg.register("m_customaccel_max", "0", arch, "Maximum custom mouse acceleration speed (0=unlimited)");
    reg.register("m_customaccel_exponent", "1", arch, "Custom mouse acceleration exponent");
    reg.register("cl_pitchup", "89", CVarFlags::NONE, "Maximum upward pitch angle in degrees");
    reg.register("cl_pitchdown", "89", CVarFlags::NONE, "Maximum downward pitch angle in degrees");

    // Video / Renderer (GoldSrc look; Xash3D FWGS `gamma.c` & retail `cstrike/config.cfg`)
    reg.register("fps_max", "100", arch, "Maximum framerate limit (0 = unlimited)");
    reg.register("fps_override", "0", arch, "Allow fps_max above 100 when set to 1");
    reg.register("host_timescale", "1", CVarFlags::NONE, "Host simulation time scale multiplier");
    reg.register("sv_use_steam_networking", "1", arch, "Use Steam networking transport when available (0 or 1)");
    reg.register("gamma", "2.2", arch, "Display gamma (GoldSrc hw.dll:0x1031e138 default 2.2)");
    reg.register("texgamma", "2.0", CVarFlags::NONE, "Texture gamma (GoldSrc default 2.0)");
    reg.register("lightgamma", "2.5", CVarFlags::NONE, "Lightmap gamma (GoldSrc default 2.5)");
    reg.register("brightness", "0.0", arch, "Display brightness factor (GoldSrc default 0.0)");
    reg.register("gl_overbright", "0", arch, "Enable GL overbright 2x lighting (0 or 1)");
    reg.register("direct", "0.9", CVarFlags::NONE, "Direct studio lighting fraction");
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
    reg.register("default_fov", "90", arch, "Default horizontal field of view (degrees)");
    reg.register("viewmodel_fov", "90", arch, "First-person viewmodel horizontal field of view");
    reg.register("net_graph", "0", arch, "Performance overlay (0=off, 1=stats, 3=compact)");
    reg.register("net_graphpos", "1", arch, "net_graph screen alignment (1=right, 2=center, 3=left)");
    reg.register("r_drawviewmodel", "1", arch, "Draw first-person weapon model (0 or 1)");
    reg.register("r_vis", "1", CVarFlags::NONE, "Enable BSP PVS visibility culling (0 or 1)");
    reg.register("r_lightmap", "0", CVarFlags::CHEAT, "Render lightmaps only (0 or 1)");
    reg.register("r_lightmap_only", "0", CVarFlags::CHEAT, "Render lightmaps only (0 or 1)");
    reg.register("r_fullbright", "0", CVarFlags::CHEAT, "Disable lightmaps (0 or 1)");
    reg.register("r_decals", "300", arch, "Maximum simultaneous world decals");
    reg.register("gl_ansio", "1", arch, "Anisotropic texture filtering level (1=OFF..16)");
    reg.register("gl_picmip", "0", arch, "Texture mipmap downscale bias (0..4)");
    reg.register("gl_max_size", "512", arch, "Maximum texture dimension (pixels)");
    reg.register("gl_round_down", "3", arch, "NPOT texture rounding heuristic (0..3)");
    reg.register("gl_msaa", "1", arch, "Multisample anti-aliasing samples (1 or 4)");
    reg.register("r_software_mode", "0", arch, "Emulate classic GoldSrc software renderer look (0 or 1)");
    reg.register("cl_bob", "0.01", arch, "View bobbing amplitude");
    reg.register("cl_bobcycle", "0.8", arch, "View bobbing cycle duration (seconds)");
    reg.register("cl_bobup", "0.5", arch, "View bobbing upward bias");
    reg.register("cl_rollangle", "0.0", arch, "Maximum view roll angle when strafing (degrees)");
    reg.register("cl_rollspeed", "200", arch, "Sideways speed for full view roll");
    reg.register("cl_viewmodel_lag", "1", arch, "Viewmodel inertia lag (0 or 1)");
    reg.register("r_noclip_speed", "450", CVarFlags::NONE, "Free-camera flight speed (units/sec)");

    // Audio & Voice
    reg.register("volume", "0.8", arch, "Master sound effects volume (0.0..1.0)");
    reg.register("mp3volume", "0.5", arch, "Menu / MP3 music volume (0.0..1.0)");
    reg.register("bgmvolume", "1", arch, "CD / background music toggle (0 or 1)");
    reg.register("hisound", "1", arch, "High-quality 22/44 kHz audio mixing (0 or 1)");
    reg.register("mp3fadetime", "2.0", arch, "MP3 music crossfade/fade-out time (seconds)");
    reg.register("room_off", "0", arch, "Disable EAX/DSP room reverb presets (0 or 1)");
    reg.register("s_show", "0", CVarFlags::NONE, "Show active mixer sound channels (0 or 1)");
    reg.register("s_occfactor", "0.25", arch, "Wall sound occlusion attenuation factor (0..1)");
    reg.register("suitvolume", "0.25", arch, "HEV suit/VOX speech volume multiplier (0..1)");
    reg.register("voice_enable", "1", arch, "Enable incoming/outgoing voice chat (0 or 1)");
    reg.register("voice_scale", "1.0", arch, "Incoming voice receive gain multiplier");
    reg.register("voice_loopback", "0", CVarFlags::NONE, "Hear own voice transmission locally (0 or 1)");
    reg.register("voice_modenable", "1", arch, "Enable mod voice support (0 or 1)");
    reg.register("voice_forcemicrecord", "1", arch, "Force microphone recording device init (0 or 1)");
    reg.register("voice_avggain", "0.5", arch, "Voice AGC average target gain");
    reg.register("voice_maxgain", "5", arch, "Voice AGC maximum gain");
    reg.register("voice_inputfromfile", "0", CVarFlags::NONE, "Read outgoing voice from voice_input.wav (0 or 1)");
    reg.register("voice_recordtofile", "0", CVarFlags::NONE, "Record mic/decoded voice to WAV file (0 or 1)");
    reg.register("sv_voiceenable", "1", srv_arch, "Allow voice communication on server (0 or 1)");
    reg.register("sv_alltalk", "0", srv, "Allow both teams to hear each other over voice (0 or 1)");
    reg.register("sv_voicecodec", "voice_opus", srv, "Server voice codec name");
    reg.register("sv_voicequality", "3", srv, "Server voice codec quality level (1..5)");
    reg.register("voice_serverdebug", "0", srv, "Log server voice relay diagnostics (0 or 1)");

    // Server / Physics (exact CS 1.6 defaults; engine `swds.dll:0x103133a4` `sv_accelerate` is "10",
    // overridden to "5" by `cs-game` `RestartRound`/`Think`)
    reg.register("hostname", "Counter-Strike 1.6 Rust", srv_arch, "Server name");
    reg.register("sv_contact", "", srv, "Server administrator contact email/URL");
    reg.register("sys_ticrate", "100.0", CVarFlags::NONE, "Target server frame rate (Hz, swds.dll:0x1031154c)");
    reg.register("sv_maxupdaterate", "30.0", CVarFlags::NONE, "Maximum client update rate (swds.dll:0x10312b7c)");
    reg.register("sv_minupdaterate", "10.0", CVarFlags::NONE, "Minimum client update rate (swds.dll:0x10312b90)");
    reg.register("sv_minrate", "0", CVarFlags::NONE, "Minimum client bandwidth rate in bytes/sec (swds.dll:0x10312bcc)");
    reg.register("sv_maxrate", "0", CVarFlags::NONE, "Maximum client bandwidth rate in bytes/sec (swds.dll:0x10312bb8)");
    reg.register("sv_lan", "0", CVarFlags::NONE, "LAN server mode (0 or 1)");
    reg.register("sv_lan_rate", "20000.0", CVarFlags::NONE, "LAN client rate in bytes/sec");
    reg.register("max_queries_sec", "3.0", CVarFlags::NONE, "Max A2S queries per second per IP (swds.dll:FUN_1020b250)");
    reg.register("max_queries_sec_global", "30", CVarFlags::NONE, "Max A2S queries per second globally (swds.dll:FUN_1020b250)");
    reg.register("max_queries_window", "60", CVarFlags::NONE, "Window in seconds for A2S query rate limits (swds.dll:FUN_1020b250)");
    reg.register("sv_gravity", "800", srv, "World gravity (units/sec^2)");
    reg.register("sv_maxspeed", "320", srv, "Maximum player movement speed");
    reg.register("sv_accelerate", "10", srv, "Ground acceleration");
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
    // ReHLDS `sv_main.cpp` (read 2026-11-22): "1", FCVAR_SERVER. 0 traces players as boxes; the
    // value times 0.5 is how much a bullet's hitbox hull is grown.
    reg.register("sv_clienttrace", "1", srv, "Size of the hitbox hull a bullet traces a player with (0: the box)");
    // The rest of the movevars (`SV_SetMoveVars`), defaults and flags from ReHLDS `sv_main.cpp`,
    // `sv_user.cpp` and `sv_phys.cpp` (read 2026-11-22). `edgefriction` and `mp_footsteps` are
    // the engine's own cvars (the names are `sv_edgefriction` and `sv_footsteps` in its source);
    // the game only reads them (`PM_PlayStepSound`, "FIXME mp_footsteps needs to be a movevar").
    // `sv_rollspeed` is "0.0" there (the message default of `MoveVarsMsg` says 200: not the
    // engine's value). The sky colour and vector cvars are flagless 0, as there.
    reg.register("sv_spectatormaxspeed", "500", CVarFlags::NONE, "Movement speed of a spectator");
    reg.register("sv_wateraccelerate", "10", srv, "Acceleration in water");
    reg.register("edgefriction", "2", srv, "Friction multiplier near a ledge");
    reg.register("sv_waterfriction", "1", srv, "Friction in water");
    reg.register("sv_rollangle", "0", CVarFlags::NONE, "Largest view roll when strafing (degrees)");
    reg.register("sv_rollspeed", "0", CVarFlags::NONE, "Sideways speed at which the view reaches the full roll");
    reg.register("mp_footsteps", "1", srv, "Players' footsteps are heard (0 or 1)");
    reg.register("sv_skycolor_r", "0", CVarFlags::NONE, "Sky colour, red");
    reg.register("sv_skycolor_g", "0", CVarFlags::NONE, "Sky colour, green");
    reg.register("sv_skycolor_b", "0", CVarFlags::NONE, "Sky colour, blue");
    reg.register("sv_skyvec_x", "0", CVarFlags::NONE, "Sky light direction, x");
    reg.register("sv_skyvec_y", "0", CVarFlags::NONE, "Sky light direction, y");
    reg.register("sv_skyvec_z", "0", CVarFlags::NONE, "Sky light direction, z");
    reg.register("sv_cheats", "0", srv, "Allow cheat commands (0 or 1)");
    reg.register("sv_unlag", "1", CVarFlags::NONE, "Lag compensation of the players' shots (0 or 1)");
    reg.register("sv_maxunlag", "0.5", CVarFlags::NONE, "Longest time lag compensation goes back (seconds, 0 for no limit)");
    reg.register("sv_unlagpush", "0", CVarFlags::NONE, "Seconds added to the time lag compensation goes back to");
    reg.register("sv_unlagsamples", "1", CVarFlags::NONE, "Updates averaged for the latency lag compensation uses (1..16)");
    reg.register("mp_roundtime", "1.75", srv_arch, "Round duration in minutes");
    reg.register("mp_freezetime", "6", srv_arch, "Freeze time at round start (seconds)");
    reg.register("mp_startmoney", "800", srv_arch, "Starting money per half");
    // ReGameDLL `game.cpp` (read 2026-11-22): `mp_buytime` "1.5", in minutes.
    reg.register("mp_buytime", "1.5", srv_arch, "Minutes of a round the players may buy in (at least 15 seconds)");
    reg.register("mp_c4timer", "45", srv_arch, "C4 detonation timer (seconds)");
    reg.register("mp_friendlyfire", "0", srv_arch, "Friendly fire (0 or 1)");
    // ReGameDLL `game.cpp` (read 2026-11-22): the team rules.
    reg.register("mp_limitteams", "2", srv, "Most players one team may have over the other (0 for no limit)");
    reg.register("mp_autoteambalance", "1", srv, "Move players between teams at the end of a round to even them out");
    // ReGameDLL `game.cpp` (read 2026-11-22): the team-kill rules.
    reg.register("mp_tkpunish", "0", srv, "Kill a player at the start of the next round for killing a teammate (0 or 1)");
    reg.register("mp_autokick", "1", srv, "Kick a player that kills three teammates (0 or 1)");
    reg.register("mp_hostagepenalty", "13", srv, "Terrorist hostages killed before kick (0 disables)");
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
    reg.register("mp_mapvoteratio", "0.66", srv, "Fraction of players needed to pass a votemap");
    reg.register("mp_kickpercent", "0.66", srv, "Fraction of teammates needed to kick a player via vote");
    // The cvars `InitHUD` sends to a joining client (ReGameDLL `game.cpp`, read 2026-11-22).
    reg.register("mp_forcecamera", "0", srv, "Spectator camera of the dead: 0 anyone, including enemies; 1 teammates only; 2 first person only");
    reg.register("mp_forcechasecam", "0", srv, "Chase camera of the dead: 0 anyone (then mp_forcecamera decides); 1 teammates only; 2 first person only");
    // `game.cpp`: `{ "mp_flashlight", "0", FCVAR_SERVER }`; `FAllowFlashlight`.
    reg.register("mp_flashlight", "0", srv, "Allow players to use the flashlight (0 or 1)");
    // The engine's `decalfrequency` (`CVAR_GET_FLOAT` in `ImpulseCommands`, impulse 201): seconds between two sprays.
    reg.register("decalfrequency", "30", srv, "Seconds between two sprays of a player's logo");
    reg.register("mp_fadetoblack", "0", srv, "Fade a dead player's screen to black (1 stays black)");
    // `game.cpp`: `{ "mp_playerid", "0", FCVAR_SERVER }`; 0 names everyone, 1 teammates only, 2 nobody.
    reg.register("mp_playerid", "0", srv, "Who the status bar names (0 all, 1 teammates, 2 nobody)");

    // Phase 12: Condition Zero / ZBot console variables
    reg.register("bot_enable", "1", srv, "Enable ZBot AI (0 or 1)");
    reg.register("bot_quota", "9", srv_arch, "Target number of bots in local match (0..31)");
    reg.register("bot_quota_match", "0", srv, "Bots per human player ratio (0 = disabled)");
    reg.register("bot_difficulty", "1", srv_arch, "Bot skill level (0=Easy, 1=Normal, 2=Hard, 3=Expert)");
    reg.register("bot_prefix", "", srv_arch, "Prefix added to bot names");
    reg.register("bot_profile_db", "BotProfile.db", srv, "Filename for ZBot personality database");
    reg.register("bot_join_after_player", "1", srv_arch, "Bots wait for a human player to join before entering (0 or 1)");
    reg.register("bot_join_team", "any", srv_arch, "Team bots must join: any, T, or CT");
    reg.register("bot_auto_vacate", "1", srv_arch, "Kick a bot when a human joins a full server (0 or 1)");
    reg.register("bot_chatter", "normal", srv_arch, "Bot radio/voice chatter mode: off, radio, minimal, normal");
    reg.register("bot_defer_to_human", "1", srv_arch, "Bots defer bomb/hostage objectives to human teammates (0 or 1)");
    reg.register("bot_walk", "0", srv, "Force bots to walk silently (0 or 1)");
    reg.register("bot_stop", "0", srv, "Freeze bot thinking and movement (0 or 1)");
    reg.register("bot_zombie", "0", srv, "Put bots into idle zombie mode (0 or 1)");
    reg.register("bot_debug", "0", srv, "Bot debug overlay verbosity (0..4)");
    reg.register("bot_nav_edit", "0", srv, "Enable navigation mesh editing overlay (0 or 1)");
    reg.register("bot_nav_zdraw", "4", srv, "Height offset for navmesh debug rendering");
    reg.register("bot_quicksave", "0", srv, "Skip sniper/encounter analysis when generating navmesh (0 or 1)");
    reg.register("bot_show_danger", "0", srv, "Show navmesh danger overlay (0 or 1)");
    reg.register("bot_show_nav", "0", srv, "Show bot navigation paths (0 or 1)");
    reg.register("bot_traceview", "0", srv, "Show bot vision/LOS debug traces (0..2)");
    reg.register("bot_allow_pistols", "1", srv, "Allow bots to buy and use pistols (0 or 1)");
    reg.register("bot_allow_shotguns", "1", srv, "Allow bots to buy and use shotguns (0 or 1)");
    reg.register("bot_allow_sub_machine_guns", "1", srv, "Allow bots to buy and use SMGs (0 or 1)");
    reg.register("bot_allow_rifles", "1", srv, "Allow bots to buy and use rifles (0 or 1)");
    reg.register("bot_allow_snipers", "1", srv, "Allow bots to buy and use sniper rifles (0 or 1)");
    reg.register("bot_allow_machine_guns", "1", srv, "Allow bots to buy and use machine guns (0 or 1)");
    reg.register("bot_allow_grenades", "1", srv, "Allow bots to buy and use grenades (0 or 1)");
    reg.register("bot_allow_shield", "1", srv, "Allow CT bots to buy and use the tactical shield (0 or 1)");
    reg.register("bot_allow_rogues", "1", srv, "Allow low-teamwork bots to go rogue and ignore radio orders (0 or 1)");

    // Phase 11: RCON, passwords, bans, logs, downloads, consistency, slowhack filter, master
    let prot = CVarFlags::PROTECTED;
    let sponly = CVarFlags::SPONLY;
    let srv_prot = srv | prot;
    reg.register("rcon_password", "", prot, "Remote console password");
    reg.register("rcon_address", "", CVarFlags::NONE, "Target IP:port for client RCON");
    reg.register("rcon_port", "27015", CVarFlags::NONE, "Target port for client RCON");
    reg.register("sv_rcon_banpenalty", "0", srv, "Minutes to ban attackers on failed RCON (0=permanent)");
    reg.register("sv_rcon_maxfailures", "10", srv, "Max RCON failures before ban");
    reg.register("sv_rcon_minfailures", "5", srv, "Min RCON failures within window before ban");
    reg.register("sv_rcon_minfailuretime", "30", srv, "Window in seconds for RCON minfailure tracking");
    reg.register("sv_password", "", srv_prot, "Password required to join the server");
    reg.register("password", "", CVarFlags::USERINFO | prot, "Client password for sv_password servers");
    reg.register("sv_filterban", "1", CVarFlags::NONE, "Enable addip IP packet filtering (0 or 1)");
    reg.register("bannedcfgfile", "banned.cfg", CVarFlags::NONE, "Filename used by writeid");
    reg.register("listipcfgfile", "listip.cfg", CVarFlags::NONE, "Filename used by writeip");
    reg.register("mp_logecho", "1", CVarFlags::NONE, "Echo server log lines to console (0 or 1)");
    reg.register("mp_logfile", "1", srv, "Write server log lines to log file (0 or 1)");
    reg.register("mp_logdetail", "0", srv, "Log hit/damage detail level (0..3)");
    reg.register("mp_logmessages", "1", srv, "Log player chat messages (0 or 1)");
    reg.register("sv_logbans", "0", srv, "Log server ban events (0 or 1)");
    reg.register("sv_log_onefile", "0", CVarFlags::NONE, "Write all maps to a single log file (0 or 1)");
    reg.register("sv_log_singleplayer", "0", CVarFlags::NONE, "Enable logging when maxplayers == 1");
    reg.register("sv_logrelay", "0", CVarFlags::NONE, "Accept remote OOB log lines (0 or 1)");
    reg.register("sv_logsecret", "0", prot, "Secret prefix for remote logaddress packets");
    reg.register("logsdir", "logs", CVarFlags::NONE, "Directory where server log files are stored");
    reg.register("sv_allowdownload", "1", srv, "Allow clients to download missing files (0 or 1)");
    reg.register("sv_allowupload", "1", srv, "Allow clients to upload custom decals (0 or 1)");
    reg.register("sv_allow_dlfile", "1", CVarFlags::NONE, "Allow netchan dlfile fallback when sv_downloadurl is set");
    reg.register("sv_downloadurl", "", sponly, "Fast HTTP download base URL");
    reg.register("sv_send_logos", "1", CVarFlags::NONE, "Broadcast custom spray logos to clients (0 or 1)");
    reg.register("sv_send_resources", "1", CVarFlags::NONE, "Send resource list to connecting clients (0 or 1)");
    reg.register("sv_uploadmax", "0.5", CVarFlags::NONE, "Maximum custom upload size in megabytes");
    reg.register("mp_consistency", "1", srv, "Enforce model and file consistency on connect (0 or 1)");
    reg.register("cl_allowdownload", "1", arch, "Allow client to download missing files from server");
    reg.register("cl_allowupload", "1", arch, "Allow client to upload custom decals");
    reg.register("cl_download_ingame", "1", arch, "Allow downloading custom decals while in game");
    reg.register("cl_filterstuffcmd", "1", arch, "Filter dangerous svc_stufftext commands from servers");
    reg.register("cl_logofile", "lambda", arch, "Custom spray logo name");
    reg.register("cl_logocolor", "orange", arch, "Custom spray logo color");
    reg.register("scr_downloading", "0", CVarFlags::NONE, "Non-zero while client download screen is active");
    reg.register("pausable", "1", srv, "Allow players to pause the server (0 or 1)");
    reg.register("master_server", "127.0.0.1:27010", arch, "Master server host:port for server browser");
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
        ("f", "impulse 100"),
        ("k", "+voicerecord"),
        ("h", "+commandmenu"),
        (",", "buyammo1"),
        (".", "buyammo2"),
        ("1", "slot1"),
        ("2", "slot2"),
        ("3", "slot3"),
        ("4", "slot4"),
        ("5", "slot5"),
        ("mouse1", "+attack"),
        ("mouse2", "+attack2"),
        ("mwheelup", "invprev"),
        ("mwheeldown", "invnext"),
        ("f5", "screenshot"),
        ("`", "toggleconsole"),
        ("escape", "cancelselect"),
    ];
    for (key, cmd) in defaults {
        reg.bind(key, cmd);
    }
}
