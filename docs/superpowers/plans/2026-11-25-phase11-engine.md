# Phase 11 (`phase11-engine`): Engine Completeness, Demos, Downloads, Admin/RCON/Bans/Logs, Master Server, HLTV & AMXX

## Waves
1. **Wave 1 — Wire Completeness (`cs-proto48`) & Full CVar/Command Protection (`cs-cvar`, `cs-clientcore`)**
   - Complete all remaining `svc_*` and `clc_*` opcodes (`svc_bad`, `svc_killedmonster`, `svc_foundsecret`, `svc_intermission`, `svc_finale`, `svc_restore`, `svc_cutscene`, `svc_filetxferfailed`, `svc_hltv`, `svc_director`, `svc_exec`, `clc_bad`, `clc_nop`, `clc_tmove`, `clc_hltv`), `write_svc_resourcelist_with_consistency`, and writers for `svc_nop`, `svc_version`, `svc_setpause`, `svc_resourcelocation`, `svc_sendcvarvalue`, `svc_sendcvarvalue2`, `clc_cvarvalue`.
   - OOB RCON packets (`ChallengeRcon`, `S2cChallengeRcon`, `Rcon`, `encode_a2a_print`).
   - `NetChan` file-stream fragmentation (`create_file_fragments`, `is_file_transfer_active`, `incoming_file_progress`).
   - Complete all 12 missing CS 1.6 `UserMsg` codecs + tests for all 8 implemented-only usermsgs.
   - Full `CVarFlags` (`PROTECTED`, `SPONLY`, `PRINTABLEONLY`, `UNLOGGED`, `NOEXTRAWHITESPACE`, `ARCHIVE`, `USERINFO`, `SERVER`, `CHEAT`, `REPLICATED`, `EXTDLL`, `CLIENTDLL`) and Phase 11 cvars/commands.
2. **Wave 2 — GoldSrc Protocol 5 `.dem` (`HLDEMO`) Recording, Playback, `timedemo`, Seeking/Rewind**
   - `cs-proto48::demo`: exact 544B header, 92B directory entries, 436B `DemoNetMsgInfo` (with exact `DemoUserCmd` C alignment padding), all 9 frame types, bounded safe parser/writer.
   - `cs-clientcore::demo`: `DemoRecorder`, `DemoPlayer` (with `timedemo` and keyframe-based seeking/rewind), integrated into `Connection<T>`.
   - `cs-client::session` + `DemoPlayerUi`: full in-client demo playback transport, seek slider, speed control, `record`/`stopdemo`/`playdemo`/`viewdemo`/`timedemo`.
3. **Wave 3 — Resource Downloads (NetChan + HTTP FastDL), Consistency Enforcement & Custom Sprays**
   - Server/client `mp_consistency` (`clc_fileconsistency`, `write_svc_resourcelist_with_consistency`, verification against live `hlds.exe` build 10210).
   - NetChan `dlfile` + `svc_filetxferfailed` + HTTP FastDL (`sv_downloadurl`, `.bz2` + raw + netchan fallback).
   - `cs-assets::hpak` + WAD3 spray validation + `tempdecal.wad` upload (`upload` stufftext, `!MD5<hex>`) + `svc_customization` broadcast.
4. **Wave 4 — Server Admin: RCON, Bans, HL Standard Logs, `users`/`status`, Cvar Queries, Slowhack Defense**
   - OOB RCON (`challenge rcon` -> `rcon`, rate limiting & brute-force lockout per ReHLDS `sv_main.cpp`) + `RconClient` & live UDP RCON verification against retail `hlds.exe`.
   - `banid`/`removeid`/`listid`/`writeid` & `addip`/`removeip`/`listip`/`writeip`, `sv_password`, `FCVAR_SERVER`/`FCVAR_PROTECTED` A2S_RULES masking.
   - HL standard logging (`sv_log.cpp`: `L MM/DD/YYYY - HH:MM:SS: ...`, `log on/off`, `logaddress_add/del`, UDP streaming).
   - `users`, extended `status full`, `svc_sendcvarvalue`/`2` roundtrips, `sv_cheats`, `pausable`, `cl_filterstuffcmd` hardening across `svc_stufftext`/`svc_director`/`svc_exec`, `sendinfo_time_spacing`, `msgdest_broadcast_datagram`.
5. **Wave 5 — `cs-master`: Master Server, Registration, Client Browser & Web UI**
   - New `crates/cs-master` (`0x31` pagination/filters, heartbeats `q`/`0`/`b`, TTL expiry, UDP + HTTP `/servers` JSON).
   - Server `setmaster` heartbeat registration + `--master` CLI flag on `cs-server`.
   - `cs-client` Internet/Spectate browser refresh + `web/` Server Browser list.
6. **Wave 6 — Stretch Deliverables: `cs-hltv` & `cs-amxx`**
   - New `crates/cs-hltv`: delayed spectator proxy (`tv_delay` ring, `svc_hltv`, `svc_director`, `clc_hltv`, `A2S_INFO` `server_type = b'p'`, optional `.dem` recording).
   - New `crates/cs-amxx`: `.amxx` zlib loader, Pawn 3.2 32-bit VM (all 144 opcodes, bounded fuel, zero `unsafe`), core/`cstrike`/`fun`/`engine`/`fakemeta`/`hamsandwich` natives, `AmxxGame<G>` wrapper wired to `cs-server --amxx-dir`.
7. **Wave 7 — Live Exit Verification, Parity Registries, Docs & Rule-7 Gate**
   - Live RCON (`RconClient -> hlds.exe` and external Python RCON `-> cs-server`), live `mp_consistency 1` against `hlds.exe` build 10210, `.dem` structural & roundtrip playback verification.
   - Update `THIRD-PARTY-NOTICES.md`, `docs/LICENSING.md`, `docs/parity/{engine,wire,usermsg,cvars}.toml`, create `docs/parity/checklists/phase11-engine.md`.
   - Full Rule-7 verification gate, `graphify update .`, `.superpowers/reports/phase11-engine.md`, and local `--no-ff` merge to `main`.
