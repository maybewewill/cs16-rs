//! Standard Counter-Strike 1.6 CVars and classic default key bindings.

use crate::{CVarFlags, CVarRegistry};

pub fn register_default_cvars(reg: &mut CVarRegistry) {
    let arch = CVarFlags::ARCHIVE;
    let srv = CVarFlags::SERVER;
    let srv_arch = CVarFlags::SERVER | CVarFlags::ARCHIVE;
    let user_arch = CVarFlags::USERINFO | CVarFlags::ARCHIVE;

    // Player / Multiplayer
    reg.register("name", "Player", user_arch, "Player display name");
    reg.register("_cl_autowepswitch", "1", user_arch, "Auto-switch to picked up weapons if more powerful (0 or 1)");
    reg.register("cl_autowepswitch", "1", arch, "Auto-switch to picked up weapons if more powerful (0 or 1)");
    reg.register("model", "gordon", user_arch, "Player model name");
    reg.register("topcolor", "30", user_arch, "Player top color index");
    reg.register("bottomcolor", "6", user_arch, "Player bottom color index");
    reg.register("team", "", user_arch, "Player team info string");
    reg.register("skin", "", user_arch, "Player skin info string");
    reg.register("cl_updaterate", "60", user_arch, "Requested server snapshot updates per second");
    reg.register("cl_cmdrate", "60", arch, "Client usercmd packets sent per second");
    reg.register("rate", "25000", user_arch, "Maximum bytes/sec client receives from server");
    reg.register("cl_dlmax", "128", user_arch, "Maximum custom decal/resource download fragment size");
    reg.register("cl_lc", "1", user_arch, "Enable server-side lag compensation (0 or 1)");
    reg.register("cl_lw", "1", user_arch, "Enable client-side weapon prediction (0 or 1)");
    reg.register("cl_crosshair_size", "medium", arch, "Crosshair size: auto, small, medium, large");
    reg.register("cl_crosshair_color", "50 250 50", arch, "Crosshair RGB color (0..255)");
    reg.register("cl_crosshair_translucent", "1", arch, "Translucent crosshair (0 or 1)");
    reg.register("cl_dynamiccrosshair", "1", arch, "Dynamic crosshair spread (0 or 1)");
    reg.register("crosshair", "1", arch, "Draw weapon crosshair (0 or 1)");
    reg.register("cl_righthand", "1", arch, "Hold weapon in right hand (1) or left hand (0)");
    reg.register("cl_hd_models", "0", arch, "Enable cstrike_hd high-definition models (0 or 1)");
    reg.register("hud_fastswitch", "1", arch, "Fast weapon switch on slot press (0 or 1)");
    reg.register("_vgui_menus", "1", user_arch, "Ask the server for VGUI menus (1) or the text menus this client draws (0)");
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
    reg.register("spec_xray", "0", arch, "Show team-colored X-Ray player silhouettes through walls in spectator or demo playback (0 or 1)");
    reg.register("spec_hud_bars", "0", arch, "Show HLTV-style CT/T team sidebar cards in spectator or demo playback (0 or 1)");
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
    reg.register(
        "cl_cmd_100hz",
        "1",
        arch,
        "Emit usercmds on a 100 Hz (10 ms) cadence when fps_override 1 runs above 105 FPS (0 or 1)",
    );
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
    reg.register("s_occfactor", "0", arch, "Wall sound occlusion attenuation factor (0..1; 0 = classic unmuffled GoldSrc)");
    reg.register("s_lerping", "1", arch, "Linear interpolation of sound samples during resampling (0 or 1)");
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

    // Task 3 Group A: Keyboard turn / pitch / speed scaling
    reg.register("cl_yawspeed", "210", CVarFlags::NONE, "Keyboard turn speed for +left/+right (deg/s)");
    reg.register("cl_pitchspeed", "225", CVarFlags::NONE, "Keyboard pitch speed for +lookup/+lookdown (deg/s)");
    reg.register("cl_anglespeedkey", "0.67", CVarFlags::NONE, "Turn/pitch speed multiplier while +speed is held");

    // Task 3 Group C: Server config lifecycle, admin, and gameplay rules
    reg.register("servercfgfile", "server.cfg", CVarFlags::NONE, "Dedicated server config executed at startup");
    reg.register("lservercfgfile", "listenserver.cfg", CVarFlags::NONE, "Listen server config executed at local game start");
    reg.register("mapchangecfgfile", "", CVarFlags::NONE, "Config file executed on every map change");
    reg.register("mapcyclefile", "mapcycle.txt", CVarFlags::NONE, "Map rotation file path");
    reg.register("motdfile", "motd.txt", CVarFlags::NONE, "Message of the day file path");
    reg.register("mp_infinite_ammo", "0", srv, "Infinite ammo mode (0=off, 1=infinite clip, 2=infinite reserve)");
    reg.register("mp_infinite_grenades", "0", srv, "Allow infinite grenades (0 or 1)");
    reg.register("mp_timeleft", "0", srv, "Remaining map time in seconds (updated live)");
    reg.register("mp_fragsleft", "0", srv, "Remaining frags to fraglimit (updated live)");
    reg.register("mp_autokick_timeout", "-1", srv, "Idle player kick timeout in seconds (-1=disabled)");
    reg.register("mp_mirrordamage", "0", srv, "Reflect friendly fire damage back onto attacker (0 or 1)");
    reg.register("mp_decals", "300", arch, "Maximum multiplayer decals");
    reg.register("mp_scoreboard_showhealth", "3", srv, "Show player health on scoreboard (0..5)");
    reg.register("mp_scoreboard_showmoney", "3", srv, "Show player money on scoreboard (0..5)");
    reg.register("mp_scoreboard_showdefkit", "1", srv, "Show defuse kit column on scoreboard (0 or 1)");
    reg.register("mp_allowmonsters", "0", srv, "Allow monster entities in multiplayer (0 or 1)");
    reg.register("mp_ghostfrequency", "0.1", srv, "Spectator ghost update interval (seconds)");
    reg.register("sv_timeout", "60", CVarFlags::NONE, "Seconds without packets before client timeout");
    reg.register("sv_visiblemaxplayers", "-1", CVarFlags::NONE, "Reported maxplayers in A2S_INFO (-1 = actual)");
    reg.register("sv_tags", "", srv, "Server search tags");
    reg.register("sv_proxies", "1", CVarFlags::NONE, "Maximum HLTV proxies allowed on server");
    reg.register("sv_reconnect_limit", "0", CVarFlags::NONE, "Maximum rapid reconnects per IP (0 = unlimited)");
    reg.register("sv_aim", "0", srv_arch, "[compat] Auto-aim toggle (0 or 1)");
    reg.register("sv_allow_autoaim", "0", srv, "[compat] Allow client auto-aim (0 or 1)");
    reg.register("sv_allow_joystick", "1", CVarFlags::NONE, "[compat] Allow joystick input on clients");
    reg.register("sv_allow_mouse", "1", CVarFlags::NONE, "[compat] Allow mouse input on clients");
    reg.register("sv_allow_noinputdevices", "1", CVarFlags::NONE, "[compat] Allow clients with no input devices");
    reg.register("sv_allow_physx", "0", CVarFlags::NONE, "[compat] Legacy physics extension toggle");
    reg.register("sv_allow_touch", "0", CVarFlags::NONE, "[compat] Allow touch input on clients");
    reg.register("sv_allow_vr", "0", CVarFlags::NONE, "[compat] Allow VR input on clients");
    reg.register("sv_background_freeze", "1", CVarFlags::NONE, "Pause simulation when in background menu");
    reg.register("sv_check_errors", "0", CVarFlags::NONE, "Verbose entity/model validation checks");
    reg.register("sv_enableoldqueries", "0", CVarFlags::NONE, "Reply to legacy pre-Source A2S queries");
    reg.register("sv_failuretime", "0.5", CVarFlags::NONE, "Time after failed frame before server reset");
    reg.register("sv_force_ent_intersection", "0", CVarFlags::NONE, "Force entity-entity bounding box intersection");
    reg.register("sv_instancedbaseline", "1", CVarFlags::NONE, "Send instanced entity baselines");
    reg.register("sv_newunit", "0", CVarFlags::NONE, "Clear saved state on map change");
    reg.register("sv_outofdatetime", "1800", CVarFlags::NONE, "Outdated connection time threshold");
    reg.register("sv_precache_meshes", "0", CVarFlags::NONE, "Precache physics meshes");
    reg.register("sv_version", "1.1.2.7/Stdio,48,10210", CVarFlags::NONE, "Engine version string");
    reg.register("hostmap", "de_dust2", CVarFlags::NONE, "Default startup map name");
    reg.register("hostage_ai_enable", "1", srv, "Enable hostage AI pathfinding (0 or 1)");
    reg.register("hostage_debug", "0", srv, "Hostage AI debug output level");

    // Task 3 Group D: Client HUD, console, render, prediction & video CVars
    reg.register("cl_showfps", "0", arch, "Show classic top-left FPS counter (0 or 1)");
    reg.register("cl_showpos", "0", arch, "Show player position, angles, and velocity (0 or 1)");
    reg.register("cl_shadows", "1", arch, "Draw player drop shadows (0 or 1)");
    reg.register("cl_minmodels", "0", arch, "Force uniform per-team player models (0 or 1)");
    reg.register("cl_min_ct", "2", arch, "Forced CT player model index when cl_minmodels is 1");
    reg.register("cl_min_t", "1", arch, "Forced T player model index when cl_minmodels is 1");
    reg.register("con_notifytime", "4", arch, "Seconds recent console lines appear in top-left overlay");
    reg.register("scr_conspeed", "600", arch, "Console open/close scroll speed");
    reg.register("con_fontsize", "1", CVarFlags::NONE, "Console font size preset");
    reg.register("con_fontscale", "1.0", arch, "Console font scale multiplier");
    reg.register("con_fontnum", "-1", arch, "Console font index (-1 = scheme default)");
    reg.register("con_mono", "0", arch, "Use monospace font in console (0 or 1)");
    reg.register("con_charset", "1252", arch, "Console character encoding codepage");
    reg.register("con_mapfilter", "1", arch, "Filter map list in console tab completion");
    reg.register("console", "1.0", arch, "Enable developer console (~) (0 or 1)");
    reg.register("cl_corpsestay", "600", arch, "Duration dead player corpses remain visible (seconds)");
    reg.register("cl_weather", "1", arch, "Enable map weather effects such as rain/snow (0 or 1)");
    reg.register("fastsprites", "0", arch, "Smoke sprite quality level (0=normal, 1=simple, 2=flat)");
    reg.register("max_shells", "120", arch, "Maximum simultaneous ejected brass shell tempents");
    reg.register("max_smokepuffs", "120", arch, "Maximum simultaneous smoke puff tempents");
    reg.register("gl_fog", "1", arch, "Enable world fog rendering (0 or 1)");
    reg.register("gl_clear", "0", CVarFlags::NONE, "Clear color buffer between frames (0 or 1)");
    reg.register("gl_dither", "1", arch, "Enable 16-bit style color dithering (0 or 1)");
    reg.register("fullscreen", "0", arch, "Fullscreen window mode (0 or 1)");
    reg.register("vid_mode", "0", CVarFlags::NONE, "Video display mode index");
    reg.register("width", "1280", arch, "Window/viewport width in pixels");
    reg.register("height", "720", arch, "Window/viewport height in pixels");
    reg.register("cl_timeout", "60", arch, "Client connection inactivity timeout (seconds)");
    reg.register("cl_allow_download", "1", arch, "Allow client downloads from server (alias for cl_allowdownload)");
    reg.register("cl_allow_upload", "1", arch, "Allow client uploads to server (alias for cl_allowupload)");
    reg.register("cl_nopred", "0", CVarFlags::NONE, "Disable client-side movement prediction (0 or 1)");
    reg.register("cl_nosmooth", "0", arch, "Disable prediction error smoothing (0 or 1)");
    reg.register("cl_smoothtime", "0.1", arch, "Prediction error smoothing window (seconds)");
    reg.register("ex_interp", "0.1", arch, "Client entity interpolation period (seconds)");
    reg.register("interp", "0.1", CVarFlags::NONE, "Interpolation period mirror");
    reg.register("net_graphheight", "64", arch, "net_graph height in pixels");
    reg.register("net_graphsolid", "1", arch, "Fill net_graph latency bars solid (0 or 1)");
    reg.register("net_graphwidth", "192", arch, "net_graph width in pixels");
    reg.register("net_scale", "5", arch, "net_graph vertical byte scale");
    reg.register("net_speeds", "0", CVarFlags::NONE, "Show network bandwidth counters");
    reg.register("r_speeds", "0", CVarFlags::NONE, "Show renderer batch and triangle counts");
    reg.register("spec_autodirector_internal", "1", arch, "Internal spectator auto-director state");
    reg.register("spec_drawcone_internal", "1", arch, "Internal spectator overview cone state");
    reg.register("spec_drawnames_internal", "1", arch, "Internal spectator overview names state");
    reg.register("spec_drawstatus_internal", "1", arch, "Internal spectator status bar state");
    reg.register("spec_mode_internal", "1", arch, "Internal spectator mode state");

    // Task 3 Group E: Remaining GoldSrc / Xash3D / Hardware / HL1 sk_* compatibility CVars
    reg.register("_snd_mixahead", "0.1", arch, "Sound mixer lookahead buffer (seconds)");
    reg.register("_window_xpos", "0", arch, "Window X coordinate");
    reg.register("_window_ypos", "0", arch, "Window Y coordinate");
    reg.register("_windowed_mouse", "0", arch, "[compat] Windowed mouse grab toggle");
    reg.register("allow_levelshots", "0", CVarFlags::NONE, "[compat] Allow levelshot screen captures");
    reg.register("ambient_fade", "100", CVarFlags::NONE, "Ambient sound fade distance");
    reg.register("ambient_level", "0.3", CVarFlags::NONE, "Ambient sound volume scale");
    reg.register("ati_npatch", "1.0", arch, "[compat] ATI TruForm N-patch tessellation");
    reg.register("cl_bmodelinterp", "1", arch, "Interpolate moving brush entities");
    reg.register("cl_charset", "utf-8", arch, "Client character encoding");
    reg.register("cl_clockreset", "0.1", CVarFlags::NONE, "Client clock drift reset threshold");
    reg.register("cl_cmdbackup", "2", arch, "Number of backup usercmds per packet");
    reg.register("cl_envshot_size", "256", arch, "Cubemap environment snapshot size");
    reg.register("cl_fixtimerate", "7.5", CVarFlags::NONE, "Client clock drift correction rate");
    reg.register("cl_himodels", "0", arch, "[compat] High-quality player models");
    reg.register("cl_idealpitchscale", "0.8", arch, "Ideal pitch look scaling");
    reg.register("cl_laddermode", "2", arch, "Ladder movement mode");
    reg.register("cl_lightstyle_lerping", "1", arch, "Smoothly interpolate lightstyle transitions");
    reg.register("cl_mousegrab", "1", arch, "Grab mouse cursor while in game");
    reg.register("cl_scoreboard_bg", "1", arch, "Draw scoreboard translucent background");
    reg.register("cl_showerror", "0", CVarFlags::NONE, "Log client prediction errors");
    reg.register("cl_showevents", "0", CVarFlags::NONE, "Log incoming game events");
    reg.register("cl_showpacketloss", "0", arch, "Show packet loss percentage");
    reg.register("cl_test_bandwidth", "0", CVarFlags::NONE, "Network bandwidth diagnostic toggle");
    reg.register("cl_trace_messages", "0", CVarFlags::NONE, "Trace incoming SVC messages");
    reg.register("cl_upmax", "0", CVarFlags::NONE, "Maximum upload bandwidth limit");
    reg.register("cl_viewbob", "1", arch, "Enable first-person view bobbing");
    reg.register("cl_vsmoothing", "0.05", arch, "View smoothing window");
    reg.register("cmd_scripting", "0", arch, "Allow cvar expansion in console scripts");
    reg.register("dsp_coeff_table", "0", CVarFlags::NONE, "DSP room coefficient table selector");
    reg.register("dsp_off", "0", CVarFlags::NONE, "Disable room DSP processing");
    reg.register("gl_flipmatrix", "0", arch, "[compat] Flip projection matrix for legacy 3dfx MiniGL");
    reg.register("gl_keeptjunctions", "1", arch, "[compat] Keep T-junction vertices on BSP faces");
    reg.register("gl_lightholes", "1", arch, "[compat] Dynamic light holes");
    reg.register("gl_lowlatency", "0", arch, "Minimize GPU frame queue latency");
    reg.register("gl_lowlatency_debugoutput", "0", CVarFlags::NONE, "Low-latency debug logging");
    reg.register("gl_lowlatency_maxslop_ms", "0", CVarFlags::NONE, "Low-latency max slop (ms)");
    reg.register("gl_lowlatency_minslop_ms", "0", CVarFlags::NONE, "Low-latency min slop (ms)");
    reg.register("gl_monolights", "0", CVarFlags::NONE, "Monochrome lightmap lighting");
    reg.register("gl_polyoffset", "4", arch, "Decal polygon depth bias");
    reg.register("gl_spriteblend", "1", arch, "Additive sprite blending");
    reg.register("gl_use_shaders", "1", arch, "[compat] Use GPU shaders");
    reg.register("gl_vbo_dlightmode", "0", arch, "[compat] VBO dynamic light mode");
    reg.register("gl_vsync", "0", arch, "Vertical synchronization");
    reg.register("gl_wateramp", "0.3", CVarFlags::NONE, "Client water surface wave amplitude");
    reg.register("gl_widescreen_yfov", "1", arch, "Hor+ widescreen FOV scaling");
    reg.register("graphheight", "64", arch, "Legacy netgraph height");
    reg.register("hpk_maxsize", "4", arch, "Maximum custom.hpk cache size (MB)");
    reg.register("http_autoremove", "1", CVarFlags::NONE, "Auto-remove completed HTTP FastDL jobs");
    reg.register("http_maxconnections", "4", CVarFlags::NONE, "Max concurrent HTTP FastDL connections");
    reg.register("http_timeout", "45", CVarFlags::NONE, "HTTP FastDL request timeout (seconds)");
    reg.register("http_useragent", "cs16-rs", CVarFlags::NONE, "HTTP FastDL User-Agent header");
    reg.register("hud_capturemouse", "1", arch, "Capture mouse for HUD");
    reg.register("hud_classautokill", "1", arch, "Suicide when switching class while alive");
    reg.register("hud_scale", "0", arch, "HUD scaling factor (0 = automatic)");
    reg.register("hud_textmode", "0", arch, "HUD text rendering mode");
    reg.register("hud_utf8", "1", arch, "Enable UTF-8 HUD text");
    reg.register("joy_axis_binding", "sfpy0", arch, "[compat] Joystick axis binding");
    reg.register("joy_enable", "0", arch, "[compat] Enable joystick input");
    reg.register("joy_forward", "1.0", arch, "[compat] Joystick forward sensitivity");
    reg.register("joy_forward_deadzone", "0", arch, "[compat] Joystick forward deadzone");
    reg.register("joy_forward_key_threshold", "24576", arch, "[compat] Joystick forward key threshold");
    reg.register("joy_lt_threshold", "-16384", arch, "[compat] Joystick left trigger threshold");
    reg.register("joy_pitch", "100.0", arch, "[compat] Joystick pitch sensitivity");
    reg.register("joy_pitch_deadzone", "0", arch, "[compat] Joystick pitch deadzone");
    reg.register("joy_rt_threshold", "-16384", arch, "[compat] Joystick right trigger threshold");
    reg.register("joy_side", "1.0", arch, "[compat] Joystick side sensitivity");
    reg.register("joy_side_deadzone", "0", arch, "[compat] Joystick side deadzone");
    reg.register("joy_side_key_threshold", "24576", arch, "[compat] Joystick side key threshold");
    reg.register("joy_yaw", "100.0", arch, "[compat] Joystick yaw sensitivity");
    reg.register("joy_yaw_deadzone", "0", arch, "[compat] Joystick yaw deadzone");
    reg.register("joystick", "0", arch, "[compat] Enable joystick");
    reg.register("look_filter", "0", arch, "Smooth look input");
    reg.register("lookspring", "0", arch, "Auto-center pitch when +mlook is released");
    reg.register("lookstrafe", "0", arch, "Mouse horizontal movement strafes instead of turning");
    reg.register("m_forward", "1", arch, "Mouse forward move scale");
    reg.register("m_ignore", "0", arch, "Ignore all mouse events");
    reg.register("m_mousethread_sleep", "10", arch, "[compat] Mouse thread poll sleep (ms)");
    reg.register("m_side", "0.8", arch, "Mouse strafe move scale");
    reg.register("menu_mp_firsttime", "0", arch, "First-time multiplayer menu prompt flag");
    reg.register("osk_enable", "0", arch, "[compat] On-screen keyboard toggle");
    reg.register("pm_showclip", "0", CVarFlags::NONE, "Debug show player movement clip hulls");
    reg.register("r_adjust_fov", "1", arch, "Adjust vertical FOV for aspect ratio");
    reg.register("r_cachestudio", "1", arch, "Cache decoded studio models");
    reg.register("r_detailtextures", "0", arch, "Enable detail textures on BSP surfaces");
    reg.register("r_lighting_modulate", "0.6", CVarFlags::NONE, "Studio lighting modulation scale");
    reg.register("r_prefertexturefiltering", "1", arch, "Prefer linear texture filtering");
    reg.register("r_refdll", "gl", arch, "[compat] Renderer backend DLL selector");
    reg.register("r_showtree", "0", CVarFlags::NONE, "Debug show BSP tree traversal");
    reg.register("r_sprite_lerping", "1", arch, "Interpolate sprite animation frames");
    reg.register("r_sprite_lighting", "1", arch, "Apply world lighting to sprites");
    reg.register("r_studiocache", "1", arch, "Enable studio mesh cache");
    reg.register("room_hires", "2", arch, "High-resolution room DSP interpolation");
    reg.register("s_combine_sounds", "0", arch, "Combine nearby identical static sounds");
    reg.register("s_samplecount", "0", CVarFlags::NONE, "Audio buffer sample count override");
    reg.register("s_warn_late_precache", "0", CVarFlags::NONE, "Warn on late sound precache");
    reg.register("sleeptime", "1", arch, "Frame idle sleep duration (ms)");
    reg.register("snd_mute_losefocus", "1", arch, "Mute game audio when window loses focus");
    reg.register("sp_decals", "4096", arch, "Maximum singleplayer world decals");
    reg.register("touch_config_file", "touch.cfg", arch, "[compat] Touch layout config file");
    reg.register("touch_emulate", "0", arch, "[compat] Emulate touch controls with mouse");
    reg.register("touch_enable", "0", arch, "[compat] Enable touch controls");
    reg.register("ui_borderclip", "0", arch, "Clip UI elements to window border");
    reg.register("ui_language", "english", arch, "UI language override");
    reg.register("ui_renderworld", "0", arch, "Render 3D world behind menu");
    reg.register("ui_show_window_stack", "0", CVarFlags::NONE, "Debug display VGUI window stack");
    reg.register("ui_showmodels", "1", arch, "Show 3D models in UI dialogs");
    reg.register("vgui_utf8", "1", arch, "Use UTF-8 encoding in VGUI2 panels");
    reg.register("vibration_enable", "0", arch, "[compat] Controller vibration toggle");
    reg.register("vibration_length", "1.0", arch, "[compat] Controller vibration length multiplier");
    reg.register("vid_displayfrequency", "0", arch, "Display refresh rate override (0 = desktop)");
    reg.register("vid_highdpi", "1", arch, "Enable High-DPI scaling");
    reg.register("vid_rotate", "0", arch, "Display rotation mode");
    reg.register("vid_scale", "1.0", arch, "Display scale factor");
    reg.register("viewsize", "120", arch, "Legacy viewport size (120 = full screen)");
    reg.register("tv_delay", "30", CVarFlags::NONE, "HLTV broadcast delay (seconds)");
    reg.register("tv_name", "HLTV Proxy", CVarFlags::NONE, "HLTV proxy display name");
    reg.register("tv_maxclients", "128", CVarFlags::NONE, "Maximum HLTV spectator clients");
    reg.register("tv_password", "", prot, "HLTV spectator password");

    // Half-Life 1 skill.cfg compatibility CVars (`sk_*`)
    for (name, val) in [
        ("sk_12mm_bullet1", "8"), ("sk_12mm_bullet2", "10"), ("sk_12mm_bullet3", "10"),
        ("sk_9mm_bullet1", "5"), ("sk_9mm_bullet2", "5"), ("sk_9mm_bullet3", "8"),
        ("sk_9mmar_bullet1", "3"), ("sk_9mmar_bullet2", "4"), ("sk_9mmar_bullet3", "5"),
        ("sk_battery1", "15"), ("sk_battery2", "15"), ("sk_battery3", "10"),
        ("sk_healthcharger1", "50"), ("sk_healthcharger2", "40"), ("sk_healthcharger3", "25"),
        ("sk_healthkit1", "15"), ("sk_healthkit2", "15"), ("sk_healthkit3", "10"),
        ("sk_plr_357_bullet1", "40"), ("sk_plr_357_bullet2", "40"), ("sk_plr_357_bullet3", "40"),
        ("sk_plr_9mm_bullet1", "8"), ("sk_plr_9mm_bullet2", "8"), ("sk_plr_9mm_bullet3", "8"),
        ("sk_plr_9mmar_bullet1", "5"), ("sk_plr_9mmar_bullet2", "5"), ("sk_plr_9mmar_bullet3", "5"),
        ("sk_plr_9mmar_grenade1", "100"), ("sk_plr_9mmar_grenade2", "100"), ("sk_plr_9mmar_grenade3", "100"),
        ("sk_plr_buckshot1", "5"), ("sk_plr_buckshot2", "5"), ("sk_plr_buckshot3", "5"),
        ("sk_plr_rpg1", "100"), ("sk_plr_rpg2", "100"), ("sk_plr_rpg3", "100"),
        ("sk_scientist_heal1", "25"), ("sk_scientist_heal2", "25"), ("sk_scientist_heal3", "25"),
        ("sk_suitcharger1", "75"), ("sk_suitcharger2", "50"), ("sk_suitcharger3", "35"),
    ] {
        reg.register(name, val, CVarFlags::NONE, "[compat] Half-Life 1 skill table value");
    }

    // Task 9: Opt-in "Beyond Original" practice, QoL, and scrim CVars (all disabled/"0" by default
    // except `sv_grenade_trajectory_time = "8"`).
    reg.register("sv_showimpacts", "0", srv, "Show bullet impact boxes (0=off, 1=both, 2=client, 3=server; requires sv_cheats 1)");
    reg.register("sv_grenade_trajectory", "0", srv, "Draw 3D grenade flight trails and bounce markers (0 or 1; requires sv_cheats 1)");
    reg.register("sv_grenade_trajectory_time", "8", srv, "Duration in seconds grenade trajectories remain visible");
    reg.register("sv_showhitboxes", "0", srv, "Draw live player/bot StudioModel hitbox 3D boxes (0 or 1; requires sv_cheats 1)");
    reg.register("mp_buy_anywhere", "0", srv, "Allow buying anywhere on the map regardless of buyzone or buytime (0 or 1)");
    reg.register("mp_warmup_mode", "0", srv, "Warmup mode: instant respawn, $16000 refill, and buy anywhere (0 or 1)");
    reg.register("cl_showspeed", "0", arch, "Show horizontal XY movement speed above HUD (0 or 1)");
    reg.register("cl_showkeys", "0", arch, "Show live movement/jump/duck key indicator on HUD (0 or 1)");
    reg.register("cl_crosshair_dot", "0", arch, "Draw a 1px center dot inside the crosshair (0 or 1)");
    reg.register("cl_crosshair_outline", "0", arch, "Draw a 1px dark outline around crosshair bars (0 or 1)");
    reg.register("cl_crosshair_t", "0", arch, "Use T-shaped crosshair without top bar (0 or 1)");
    reg.register("cl_crosshair_gap", "0", arch, "Additional crosshair gap offset in pixels (0 = vanilla)");
    reg.register("cl_mute_enemy_team", "0", arch, "Mute opposing team chat and voice (0 or 1)");
    reg.register("sv_damage_print", "0", srv, "Print round Damage Given/Taken summary to player console on death/round end (0 or 1)");
    reg.register("mp_knife_round", "0", srv, "Start a knife-only round with .stay/.switch before lo3 (0 or 1)");

    // Task 10: Built-in KZ/bhop mode and Kreedz JumpStats (all disabled/"0" by default).
    reg.register("sv_kz_mode", "0", srv, "Built-in KZ mode: checkpoints, func_button map timers, and non-lethal fall damage (0 or 1)");
    reg.register("sv_kz_keep_teleport_vel", "0", srv, "Preserve player velocity across trigger_teleport in KZ/bhop mode (0 or 1)");
    reg.register("sv_ljstats", "0", srv, "Enable server-side Kreedz JumpStats console reports (0 or 1)");
    reg.register("cl_ljstats", "0", arch, "Enable client-side Kreedz JumpStats HUD and console reports (0 or 1)");
    reg.register("cl_ljstats_min", "215", arch, "Minimum jump distance in units to report in JumpStats");
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
