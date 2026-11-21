# Фаза 2C — игра CS на движке: план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Новый crate `cs-game` — аналог `mp.dll`: игроки, команды и меню выбора, раунды с заморозкой и таймером, деньги, табло, чат, наблюдатель, зоны и меню покупки (без оружия), сообщения HUD — перенос поведения из ReGameDLL_CS; `cs-server --proto48` запускает его вместо `WalkGame`.

**Architecture:** `cs-game` реализует `cs_engine::GameDll` и общается с движком только через `EngineApi`. Энтити игры — трейт `GameEntity` в приватных данных edict'а, создаются по `classname` через реестр (как `LINK_ENTITY_TO_CLASS`); движок зовёт `think`/`touch`/`blocked`, игра раздаёт их своим энтити. Поведение переносится из ReGameDLL_CS (GPL v2-or-later, порт разрешён, `docs/LICENSING.md`): каждая перенесённая функция называет в doc-комментарии файл, функцию и дату чтения. Движение игрока пока на `cs_sim::pmove` (точность — фаза 6). Оружие, бомба, заложники, VIP, боты — фазы 2D/2E.

**Tech Stack:** Rust 1.100.0, `cs-engine`, `cs-proto48` (usermsg), `cs-sim` (pmove), `cs-probe`; ReGameDLL_CS как эталон.

**Spec:** `docs/superpowers/specs/2026-11-20-cs16-rs-master-plan-design.md` (§4.3, §6 «Фаза 2», пп. 1, 3, 4); журналы 2A/2B (`.superpowers/sdd/*/progress.md`) — принятые отличия и отложенное.

**Источник ReGameDLL_CS** (только чтение; один URL на запрос): `https://raw.githubusercontent.com/s1lentq/ReGameDLL_CS/master/regamedll/dlls/<file>` — `client.cpp`, `player.cpp`, `multiplay_gamerules.cpp`, `gamerules.cpp`, `game.cpp`, `world.cpp`, `observer.cpp`, `subs.cpp`, `util.cpp`, `cbase.cpp`, `triggers.cpp` (buyzone), `career_tasks.cpp` — не нужен. ReHLDS — как в 2B.

## Global Constraints

- Всё из `constraints.md` фаз 2A/2B: `unsafe_code = "forbid"`; clippy `-D warnings`, `cargo test --workspace`, wasm-check библиотек зелёные после каждой задачи; cargo только `nice -n 19 env CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 cargo <args>` по одной команде на агента; никакого `cargo fmt`; не release.
- Rust 1.100.0; новый crate — в `[workspace] members` и `[workspace.dependencies]`, `[lints] workspace = true`, `rust-version.workspace = true`, проходит wasm-check (`cs-game` — библиотека).
- Никаких ассетов Valve в репозитории; реальные карты — через VFS, тест без игры возвращается без проверок; игра лежит в `D:\SteamLibrary\steamapps\common\Half-Life` (`CS16_GAMEDIR` при необходимости).
- Сообщения клиенту — только зарегистрированные usermsg в формате Protocol 48 (`cs_proto48::usermsg::CsUserMsg`), строки локализации — ключи `#Cstrike_*`/`#...` как в ReGameDLL (клиент переводит сам).
- Ни один путь игры не паникует на вводе клиента (команды, аргументы, userinfo); строки клиента ограничены по длине.
- Утверждения о поведении оригинала — с файлом ReGameDLL/ReHLDS и датой; по памяти — «assumption».
- Старый путь (`cs-net`, `MatchWorld`, офлайн-клиент) и `WalkGame` (тесты `cs-sim`) не ломаются.

## Review Focus

1. Враждебные команды клиента: `jointeam 999`, `joinclass -1`, `menuselect 0/10/abc`, пустые и 4 КБ аргументы, `say` с `%s`/`#`-ключами и управляющими символами, команда до входа в игру. Тест: задача 4 `hostile_team_commands_are_ignored`, задача 8 `say_is_sanitized_and_bounded`.
2. Игроки входят и выходят посреди раунда, в заморозке, после конца раунда, при смене карты; последний игрок команды выходит — раунд заканчивается правильно. Тест: задача 5 `leaving_players_end_the_round_as_regamedll`.
3. Деньги на границах: 16000, отрицательный баланс за тимкилл, серия поражений. Тест: задача 6 `money_is_clamped_and_loss_bonus_grows`.
4. Табло и сообщения при 32 игроках: `ScoreInfo`/`TeamInfo` каждому входящему, без переполнения надёжного канала на входе. Тест: задача 3 `full_server_init_hud_fits`.
5. Наблюдатель за убитым или вышедшим игроком, `mp_forcecamera`. Тест: задача 9 `observer_target_leaves`.

---

### Task 1: Crate `cs-game`, реестр энтити, мир

**Files:**
- Create: `crates/cs-game/Cargo.toml`, `src/lib.rs` (`CsGame: GameDll`), `src/entity.rs` (трейт и реестр), `src/world.rs` (CWorld, точки спавна, статические браши), `tests/game_entities.rs`
- Modify: корневой `Cargo.toml`, `crates/cs-server/src/proto48.rs` (запуск `CsGame`; флаг `--walk-game` оставляет `WalkGame`)

**Interfaces — Produces:**

```rust
pub trait GameEntity: Send + 'static {
    fn classname(&self) -> &'static str;
    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool { true }
    fn think(&mut self, _g: &mut GameState, _eng: &mut EngineApi<'_>, _id: EdictId) {}
    fn touch(&mut self, _g: &mut GameState, _eng: &mut EngineApi<'_>, _id: EdictId, _other: EdictId) {}
    fn blocked(&mut self, _g: &mut GameState, _eng: &mut EngineApi<'_>, _id: EdictId, _other: EdictId) {}
}
pub fn create_entity(classname: &str) -> Option<Box<dyn GameEntity>>;   // реестр по classname
pub struct CsGame { /* GameState, игроки */ }  impl CsGame { pub fn new() -> Self; }
```

Перенести: `CWorld::Spawn`/`Precache` (`world.cpp`: прекэш общих ресурсов, ключи `skyname`, `WaveHeight`, `MaxRange`, `chaptertitle` — что относится к серверу), `W_Precache`/`ClientPrecache` (`client.cpp`, `weapons.cpp` — только то, что нужно без оружия: модели игроков `models/player/<name>/<name>.mdl`, звуки игрока), точки спавна `info_player_start` (CT) и `info_player_deathmatch` (T) с `SelectSpawnPoint`/`EntSelectSpawnPoint` (`player.cpp`). Статические браш-энтити (`func_wall`, `func_door*`, `func_breakable`, `func_button`, `func_plat`, `func_train`, `func_pushable`, `func_illusionary`, `func_buyzone` — последняя в задаче 10) — как в `WalkGame`: `SOLID_BSP`, `MOVETYPE_PUSH`, без логики; `func_illusionary` — `SOLID_NOT`, видимая. Неизвестный `classname` — `false` (энтити удаляется), одна строка `debug!`. Раздача вызова энтити: `Box<dyn GameEntity>` на время вызова вынимается из приватных данных edict'а и кладётся обратно после (если энтити ещё жива), чтобы энтити, `GameState` и `EngineApi` не заимствовались одновременно.

- [ ] **Step 1: Тесты:** `world_precaches_player_models_and_sounds`, `spawn_points_come_from_the_lump_by_team` (de_dust2, пропуск без игры), `unknown_classes_are_removed`, `brush_entities_stay_solid` (трасса упирается в `func_wall`), `entity_dispatch_reaches_the_owner` (think/touch/blocked тестовой энтити из реестра).
- [ ] **Step 2–4:** FAIL → реализовать → PASS; `cargo build -p cs-server` с `CsGame`.
- [ ] **Step 5: Commit** `feat(game): cs-game crate, entity registry and world`

### Task 2: Игрок — жизненный цикл и движение

**Files:** Create `crates/cs-game/src/player.rs`, `tests/player.rs`; Modify `lib.rs`

**Interfaces — Produces:** `pub struct Player { team: Team, class: u8, health: f32, armor: f32, frags: i32, deaths: i32, money: i32, alive: bool, … }`, `pub enum Team { Unassigned, Terrorist, Ct, Spectator }`; `GameState::players: [Option<Player>; 32]`.

Перенести из `player.cpp`/`client.cpp`: `ClientConnect`, `ClientPutInServer` (игрок входит как `Unassigned`, невидимый, `MOVETYPE_NONE`, без модели — до выбора команды), `CBasePlayer::Spawn` (здоровье 100, хулл, `view_ofs`, `SOLID_SLIDEBOX`, `MOVETYPE_WALK`, модель по команде и классу, `pev->maxspeed` 250 без оружия), `PlayerPreThink`/`PlayerPostThink` (падение: `FallDamage` по `m_flFallVelocity` как в `PlayerPostThink`/`PM_FallDamage`), `CBasePlayer::TakeDamage`/`Killed` (`deadflag`, `DEAD_DYING`→`DEAD_DEAD`, труп), команда `kill` (`ClientKill`: −1 фраг, правила задержки), `UpdateClientData` (поля `clientdata`: здоровье, `maxspeed`, `fov`, `view_ofs`, `flags`, `waterlevel`, `deadflag`). Движение — `cs_sim::pmove` через адаптер `trace_player_move` как в `WalkGame` (перенести адаптер в `cs-game`).

- [ ] **Step 1: Тесты:** `connected_player_waits_unassigned`, `spawned_player_has_regamedll_defaults`, `fall_damage_as_regamedll` (падение с высоты → урон по формуле, на de_dust2 или стабе с полом), `kill_command_kills_and_costs_a_frag`, `client_data_reflects_the_player`.
- [ ] **Step 2–5:** FAIL → реализовать → PASS → Commit `feat(game): player lifecycle, damage and movement`

### Task 3: Сообщения игрока и инициализация HUD

**Files:** Create `crates/cs-game/src/messages.rs`, `tests/hud.rs`; Modify `lib.rs`

Перенести: `LinkUserMessages` (`player.cpp`) — регистрация всех usermsg CS в порядке и с размерами ReGameDLL (через `GameDll::usermsgs`); `CHalfLifeMultiplay::InitHUD` (`multiplay_gamerules.cpp`: что шлётся входящему — `ScoreInfo`/`TeamInfo` всех, `TeamScore`, `ServerName`, `ResetHUD`, `GameMode`, `Money`, `Location`/`StatusIcon` по необходимости), `CBasePlayer::UpdateClientData` части HUD (`Health`, `Battery`, `Damage`, `CurWeapon` пустой, `HideWeapon`, `SetFOV`), `SendMOTDToClient` (`motd.txt` через VFS, `MOTD` кусками по 60 байт как в источнике). Отправка через `EngineApi::send_usermsg`.

- [ ] **Step 1: Тесты:** `usermsgs_registered_in_regamedll_order`, `init_hud_sequence_as_regamedll` (probe получает сообщения в порядке источника), `motd_is_sent_in_chunks`, `health_and_armor_follow_the_player`, `full_server_init_hud_fits` (32 слота заняты, 32-й входит: надёжный поток входа ≤ лимитов движка, без переполнения).
- [ ] **Step 2–5:** … → Commit `feat(game): CS user messages and HUD init`

### Task 4: Команды и меню выбора

**Files:** Create `crates/cs-game/src/teams.rs`, `src/menus.rs`, `tests/teams.rs`; Modify `lib.rs`, `crates/cs-probe/src/main.rs` (`--cmd "<строка>"`, можно несколько, отправка после входа в игру)

Перенести: `ShowVGUIMenu`/`ShowMenu` (`player.cpp`/`client.cpp`: VGUI-меню выбора команды и класса, старое текстовое меню при `_vgui_menus 0`), `HandleMenu_ChooseTeam`, `HandleMenu_ChooseAppearance`, команды `jointeam`, `joinclass`, `chooseteam`, `menuselect` (`ClientCommand_`), автоназначение (5), лимиты `mp_limitteams`, `mp_autoteambalance` (баланс в конце раунда — задача 5), `TeamInfo` всем, модели классов (`urban`, `gsg9`, `sas`, `gign` / `terror`, `leet`, `arctic`, `guerilla`), смена команды живым (смерть, −0 фраг как в источнике), `mp_teamlist`/`m_iJoiningState`.

- [ ] **Step 1: Тесты:** `new_player_sees_the_team_menu`, `jointeam_then_joinclass_spawns_on_the_team` (probe `--cmd`), `team_limits_refuse_as_regamedll`, `auto_assign_balances`, `switching_team_alive_kills_as_regamedll`, `hostile_team_commands_are_ignored`.
- [ ] **Step 2–5:** … → Commit `feat(game): teams, class selection and menus`

### Task 5: Раунды

**Files:** Create `crates/cs-game/src/rules.rs`, `tests/rounds.rs`; Modify `lib.rs`

Перенести из `multiplay_gamerules.cpp`: состояние раунда (`m_bFreezePeriod`, `m_fRoundStartTime`, `m_iRoundTime`), `RestartRound` (возрождение живых и мёртвых по командам, сброс, `RoundTime`, заморозка `mp_freezetime`), `CheckFreezePeriodExpired`/`OnRoundFreezeEnd`, `CheckRoundTimeExpired` (`mp_roundtime` — победа CT на картах без целей, ничья по источнику), `TeamExterminationCheck`, `Round_Ts`/`Round_Cts`/`Round_Draw`, `EndRoundMessage` (`TextMsg` `#Terrorists_Win`/`#CTs_Win`/`#Round_Draw`, `SendAudio` `%!MRAD_terwin`…), задержка конца раунда, «Game Commencing» при входе первых игроков (`NeededPlayersCheck`), `sv_restart`/`sv_restartround`, `CheckWinLimit`/`CheckMaxRounds`/`CheckTimeLimit` → смена карты по `mapcycle.txt` (через `EngineApi`/консоль `changelevel`), автобаланс в конце раунда (`BalanceTeams`). Цели карт (бомба, заложники, VIP, escape) — 2E: ветки в `CheckWinConditions` для них — заглушки с пометкой.

- [ ] **Step 1: Тесты:** `freeze_time_holds_players_then_round_starts`, `elimination_ends_the_round_for_the_other_team`, `round_time_expiry_as_regamedll`, `game_commencing_on_first_players`, `restart_round_cvar`, `win_limit_changes_the_map`, `leaving_players_end_the_round_as_regamedll`.
- [ ] **Step 2–5:** … → Commit `feat(game): round rules as CHalfLifeMultiplay`

### Task 6: Деньги

**Files:** Create `crates/cs-game/src/money.rs`, `tests/money.rs`; Modify `rules.rs`, `player.rs`

Перенести: `CBasePlayer::AddAccount` (ограничение 16000 — `MAX_ACCOUNT` из источника), стартовые деньги `mp_startmoney` (`CheckStartMoney`), награды за раунд (`m_iAccountTerrorist`/`m_iAccountCT`, серии поражений `m_iNumConsecutiveTerroristLoses`… с суммами источника), награда за убийство (300), штраф за тимкилл (−3300) и тимкилл-кик `mp_tkpunish` по источнику, `Money` (+ флаг мигания), `BlinkAccount`.

- [ ] **Step 1: Тесты:** `start_money_from_cvar`, `round_rewards_as_regamedll`, `kill_reward_and_teamkill_penalty`, `money_is_clamped_and_loss_bonus_grows`.
- [ ] **Step 2–5:** … → Commit `feat(game): money and rewards`

### Task 7: Табло и сообщения о смерти

**Files:** Create `crates/cs-game/src/score.rs`, `tests/score.rs`; Modify `player.rs`, `rules.rs`

Перенести: `ScoreInfo` (фраги, смерти, класс, команда), `ScoreAttrib` (флаг мёртв/бомба/VIP — бомба/VIP пустые до 2E), `DeathMsg` (убийца, жертва, хедшот, имя оружия — `"world"`/`"worldspawn"` для падения, `"suicide"`? по источнику), `TeamScore`, `IPointsForKill`, `PlayerKilled`/`DeathNotice` (`multiplay_gamerules.cpp`).

- [ ] **Step 1: Тесты:** `death_updates_scores_for_everyone`, `fall_death_message_as_regamedll`, `team_score_after_round`.
- [ ] **Step 2–5:** … → Commit `feat(game): scoreboard and death notices`

### Task 8: Чат, имена, текст

**Files:** Create `crates/cs-game/src/chat.rs`, `tests/chat.rs`; Modify `lib.rs`

Перенести `Host_Say` (`client.cpp`: `say`, `say_team`, форматы `#Cstrike_Chat_All`, `#Cstrike_Chat_CT`, `#Cstrike_Chat_T`, `#Cstrike_Chat_AllDead`, `#Cstrike_Chat_Spec`, правила, кто видит чат мёртвых `sv_alltalk`? — по источнику, обрезка, запрет `%`), смену имени (`ClientUserInfoChanged`: `#Cstrike_Name_Change`, ограничение частоты по источнику), `ClientPrint`/`TextMsg` уровни `HUD_PRINTNOTIFY/CONSOLE/TALK/CENTER`.

- [ ] **Step 1: Тесты:** `say_reaches_everyone_with_the_regamedll_format`, `say_team_reaches_the_team_only`, `dead_chat_as_regamedll`, `name_change_is_announced_and_limited`, `say_is_sanitized_and_bounded`.
- [ ] **Step 2–5:** … → Commit `feat(game): chat, names and text messages`

### Task 9: Наблюдатель

**Files:** Create `crates/cs-game/src/observer.rs`, `tests/observer.rs`; Modify `player.rs`, `lib.rs`

Перенести из `observer.cpp`/`player.cpp`: `StartDeathCam`, `StartObserver`, `Observer_SetMode` (режимы `OBS_CHASE_LOCKED`, `OBS_CHASE_FREE`, `OBS_ROAMING`, `OBS_IN_EYE`, `OBS_MAP_*`), `Observer_FindNextPlayer`, `Observer_HandleButtons`, `Observer_CheckTarget`, `mp_forcecamera`, `specmode`/`follownext` команды, `pev->iuser1/2/3` и что идёт в `clientdata`/`entity_state` для наблюдателя (поля `iuser*` уже в `EntVars`).

- [ ] **Step 1: Тесты:** `dead_player_becomes_an_observer_after_the_death_cam`, `observer_follows_a_teammate_with_forcecamera`, `spectator_team_roams`, `observer_target_leaves`.
- [ ] **Step 2–5:** … → Commit `feat(game): observer modes`

### Task 10: Зона и меню покупки (без оружия)

**Files:** Create `crates/cs-game/src/buy.rs`, `tests/buy.rs`; Modify `world.rs` (`func_buyzone`), `player.rs`

Перенести: `CBuyZone` (`triggers.cpp`: `func_buyzone` с `team`), зоны вокруг спавнов на картах без `func_buyzone` (`CHalfLifeMultiplay`/`player.cpp` — по источнику), `StatusIcon` `buyzone`, время покупки `mp_buytime`, `CanPlayerBuy`, команды `buy`, `buyequip`, меню покупки (`ShowVGUIMenu` `MENU_BUY*`, `BuyClose`), предметы без оружия: `vest` (кевлар 650), `vesthelm` (1000), `defuser` (только CT и только на картах с бомбой — 2E, пока отказ), `nvgs` (1250); `Battery`/`ArmorType`; покупка оружия — отказ с `TextMsg` по источнику до 2D.

- [ ] **Step 1: Тесты:** `buy_zone_icon_on_spawn`, `buy_armor_as_regamedll`, `buy_time_expires`, `cannot_buy_outside_the_zone_or_without_money`, `weapon_purchase_refused_until_weapons`.
- [ ] **Step 2–5:** … → Commit `feat(game): buy zones and equipment purchases`

### Task 11: Радар, радио, мелкие HUD

**Files:** Create `crates/cs-game/src/hud_misc.rs`, `tests/hud_misc.rs`

Перенести: `Radar` (позиции товарищей с периодом источника), радио `radio1`/`radio2`/`radio3` и алиасы (`Radio1..3`, `HandleRadioAliasCommands`: `TextMsg` `#Game_radio`, `SendAudio` `%!MRAD_*`, ограничение частоты), `StatusValue`/`StatusText` (идентификация игрока под прицелом — `UpdateStatusBar`), `Location` при наличии, `nightvision` (переключение `NVGToggle`, если куплен), `drop` — пустой до 2D.

- [ ] **Step 1: Тесты:** `radar_shows_teammates_only`, `radio_reaches_the_team_with_sound`, `status_bar_names_the_teammate_under_the_crosshair`.
- [ ] **Step 2–5:** … → Commit `feat(game): radar, radio and status bar`

### Task 12: Сквозная проверка, документы, скриншоты

**Files:** Create `crates/cs-game/tests/match_flow.rs`, `docs/parity/checklists/phase2c-game.md`; Modify `docs/parity/*.toml` (новый реестр `docs/parity/game.toml` для правил/HUD/команд, если формат реестров это допускает — иначе в существующие), `README.md`, `THIRD-PARTY-NOTICES.md` (файлы ReGameDLL_CS, что взято)

- [ ] **Step 1: Сквозной тест** `two_teams_play_a_round` (loopback, 2–4 probe с `--cmd`): вход → меню → `jointeam`/`joinclass` → заморозка → раунд → `kill` последнего живого Т → `#CTs_Win`, `TeamScore`, деньги по источнику, табло, следующий раунд с возрождением.
- [ ] **Step 2: Бинарь** (debug, порт 27115): `cs-server --proto48 --map de_dust2` + два `cs-probe --cmd "jointeam 1" --cmd "joinclass 1"`/`"jointeam 2"` — оба в игре, раунд идёт, `status`, `quit` → 0.
- [ ] **Step 3: Реестр и чек-лист владельца** (оригинальный клиент: меню команды и класса, HUD денег/таймера/здоровья, табло Tab, чат, радио, наблюдатель после `kill`, покупка брони в зоне) — точные команды; ничего `verified` без даты.
- [ ] **Step 4: Итоговая проверка** (clippy, `cargo test --workspace`, wasm, `cargo xtask parity`).
- [ ] **Step 5: Commit** `docs: phase 2C registry, checklist and notices`

Скриншоты оригинального клиента — работа контроллера после задачи 12 (через управление компьютером, с разрешения на каждое приложение): сервер на `127.0.0.1:27015`, клиент `connect`, снимки меню выбора команды, HUD в раунде, табло; снимки не попадают в репозиторий (графика Valve), только к владельцу.

---

## Что вне этого плана

Оружие, гранаты, предсказание оружия, хитбоксы — 2D. Бомба, заложники, VIP, escape, `func_*` логика в правилах, боты — 2E и фазы 7, 12. Голосования, `mp_*` полностью, тимкилл-наказания сверх денег — фаза 7.
