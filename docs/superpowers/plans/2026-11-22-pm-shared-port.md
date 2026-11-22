# Перенос pm_shared (движение игрока) — план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Движение игрока в `cs-game` идёт по точному переносу `pm_shared` из ReGameDLL_CS (ванильная ветка) и по контракту движка ReHLDS (`SV_RunCmd`, `SV_SetupPMove`/`SV_FinishPMove`, `pmovetst.cpp`). Оригинальный клиент предсказывает тем же кодом, поэтому без расхождений не будет дрожания. Ходьба, бег, трение, края, ступени, прыжок с «выносливостью» `fuser2`, присед в две фазы, лестницы, вода, звуки шагов по материалам, удар при приземлении, наблюдатель.

**Architecture:**
- Новый модуль `cs_sim::pm`. Он не зависит от движка: мир доступен только через трейт `PmWorld`, чтобы позже клиент предсказывал тем же кодом.
- Движок даёт сервисы `pmove`: `physents`/`moveents` (`SV_AddLinksToPM`), трассу игрока по хуллам (`PM_PlayerTrace`), `PM_TestPlayerPosition`, `PM_PointContents`, лестницы, `PM_TraceModel`, `TraceTexture`, звук всем, кроме игрока (`PM_SV_PlaySound`), `movevars` из cvar с пересылкой при изменении.
- `cs-game` повторяет `SV_RunCmd`: заполняет `PlayerMove` из edict, вызывает `pm_move`, переносит результат обратно.
- Старое движение (`cs_sim::pmove`, `WalkGame`, `MatchWorld`, `cs-net`, `cs-client`) не трогаем.

**Tech Stack:** Rust 1.100.0, `cs-engine`, `cs-sim`, `cs-game`, `cs-proto48`, `cs-cvar`, `cs-probe`; ReGameDLL_CS `pm_shared/*` и ReHLDS `pmove.cpp`, `pmovetst.cpp`, `sv_user.cpp`, `sv_pmove.cpp`, `sv_main.cpp` как эталон.

**Spec:** `docs/superpowers/specs/2026-11-20-cs16-rs-master-plan-design.md`: §6 «Фаза 6» п. 2 — эта работа выносится раньше 2D (решение 2026-11-22); категория реестра «Движение (`pm_shared`)», вес 8. Журнал 2C (`.superpowers/sdd/2026-11-22-phase2c-cs-game/progress.md`), строки CARRY 33, 53, 59 и 80.

**Описи (обязательно прочитать до начала):** `.superpowers/sdd/2026-11-22-pm-shared/survey-source.md` — функции, константы, контракты; `survey-code.md` — что есть в нашем коде. Исходники уже скачаны (см. `constraints.md`).

## Global Constraints

- Действуют все пункты `constraints.md` фаз 2A–2C:
  - `unsafe_code = "forbid"`;
  - после каждой задачи зелёные clippy `-D warnings`, `cargo test --workspace` и wasm-check библиотек;
  - cargo только `nice -n 19 env CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 cargo <args>`;
  - никакого `cargo fmt` и release;
  - ни один вызов `EngineApi` не паникует на данных игры.
- Целевое поведение — ванильная ветка: код вне `REGAMEDLL_FIXES`, `REGAMEDLL_ADD`, `REGAMEDLL_API` и `BUILD_LATEST`.
- Перенос делается оператор за оператором, в порядке источника. Каждая причуда источника (quirk) названа в комментарии.
- Каждая перенесённая функция называет в doc-комментарии файл, функцию и дату чтения.
- Числа с плавающей точкой — по правилу из `constraints.md`: хранение в `f32`, `f64` там, где C смешивает `float` с `double`.
- Ни один путь не паникует на вводе клиента: NaN и бесконечность в usercmd, `msec` 0 и 255, огромные `forwardmove`, углы вне диапазона.
- Ни один путь не паникует на данных карты: пустая модель, плохой индекс хулла, `skin` лестницы без модели.
- Тесты `cs-sim` идут без игры, на `BoxWorld`. Тесты на реальных картах возвращаются без проверок, если игры нет.

## Review Focus

1. Порядок `PM_PlayerMove` и `SV_RunCmd` один в один с источником. Тест: задача 4 `player_move_runs_in_source_order`, задача 8 `run_cmd_follows_sv_runcmd`.
2. Причуды, от которых зависит предсказание:
   - промах трассы оставляет нормаль нулевой, тогда `onground = -1`;
   - `PM_TestPlayerPosition` трассирует от `pmove->origin`;
   - `PM_WalkMove` использует `up` как черновую переменную;
   - `flDuckTime` и `flSwimTime` усекаются до `int` при обратном переносе;
   - `fuser2` 1315.789429.

   Тесты задач 4, 5 и 8.
3. Враждебный usercmd: NaN, `msec` 0 и 255, `forwardmove` 1e30, кнопки 0xFFFF. Тест: задача 4 `hostile_cmds_do_not_break_the_move`, задача 8 `hostile_usercmds_keep_the_player_finite`.
4. Звук шагов: двигающийся игрок свой шаг не получает, остальные получают по PAS. Тест: задача 3 `pm_sound_skips_the_mover`, задача 9 `footsteps_reach_others_not_self`.
5. Старое движение не изменилось. Тест: `walk_game.rs::legacy_numbers_unchanged` и тесты `MatchWorld` зелёные без правок.

## Порядок и параллельность

- Волна 1 (параллельно, 4 исполнителя): задачи 1, 2, 3 в отдельных worktree и задача 4 в основной копии.
- Волна 2 (параллельно, 3 исполнителя): задачи 5, 6 и 7. Каждая владеет своими файлами `cs_sim::pm`, заглушки которых создала задача 4.
- Волна 3: задача 8, затем задача 9.
- Волна 4: задача 10 (документы), затем финальное ревью всей ветки.

---

### Task 1: Движок — `movevars` из cvar и их пересылка

**Files:**
- Modify: `crates/cs-cvar/src/defaults.rs`, `docs/parity/cvars.toml`, `crates/cs-engine/src/signon.rs` (сборщик `move_vars`), `crates/cs-engine/src/frame.rs` (или новый `crates/cs-engine/src/movevars.rs`), `crates/cs-engine/src/lib.rs`, `crates/cs-engine/src/api_entities.rs` или `api_messages.rs` (метод)
- Test: `crates/cs-engine/tests/movevars.rs`

**Interfaces — Produces:**

```rust
impl EngineApi<'_> {
    /// The server's current movevars (ReHLDS `SV_SetMoveVars`, refreshed every frame).
    pub fn move_vars(&self) -> cs_proto48::messages::MoveVarsMsg;
}
```

(Путь к `MoveVarsMsg` — как в `cs-proto48` сейчас.)

Перенести:
1. Регистрация недостающих cvar с умолчаниями и флагами ReHLDS: `sv_spectatormaxspeed`, `sv_wateraccelerate`, `edgefriction`, `sv_waterfriction`, `sv_rollangle`, `sv_rollspeed` и тот cvar, из которого `movevars.footsteps` (`mp_footsteps`). Каждый — с записью в `cvars.toml`. Значения проверить по `rh_sv_main.cpp` и связанным файлам, а не по описи. `sv_stopspeed` остаётся 100: это умолчание движка, 75 ставит игра в задаче 9.
2. `SV_SetMoveVars`: все поля `movevars_t` из своих cvar, `entgravity` 1, `skyName` из `sv_skyname`. `signon.rs` строит `svc_newmovevars` тем же сборщиком.
3. `SV_QueryMovevarsChanged` или его аналог: при изменении любого поля `svc_newmovevars` уходит каждому клиенту, вошедшему в игру, один раз. Сообщение то же, что на входе. Исправляет пробел из `console.rs` и `engine.toml`.

- [ ] **Step 1: Тесты:**
  - `move_vars_follow_every_cvar` — каждое поле `movevars_t` меняется своим cvar;
  - `new_movement_cvars_have_rehlds_defaults`;
  - `changed_movevars_are_resent_once` — probe получает второй `svc_newmovevars` ровно один раз после `sv_gravity 600`;
  - `unchanged_movevars_are_not_resent`.
- [ ] **Step 2–4:** FAIL → реализовать → PASS. Существующий тест `move_vars_follow_cvars_and_sky` остаётся зелёным.
- [ ] **Step 5: Commit** `feat(engine): movevars follow cvars and are resent on change`

### Task 2: Движок — сервисы столкновений `pmove`

**Files:**
- Create: `crates/cs-engine/src/pm.rs`, `crates/cs-engine/tests/pm_services.rs`
- Modify: `crates/cs-engine/src/lib.rs` (модуль, реэкспорт типов), `crates/cs-engine/src/api_entities.rs` (методы `EngineApi`). Хуллы, клипноды и боксы — через существующий код `collision.rs` и `trace_ents.rs`, не копией.

**Interfaces — Produces:**

```rust
/// pmove.cpp player hulls by `usehull` (0 stand, 1 duck, 2 point, 3 large).
pub const PM_PLAYER_MINS: [Vec3; 4];
pub const PM_PLAYER_MAXS: [Vec3; 4];

/// `SV_AddLinksToPM` snapshot for one player's command: physents (world first) and moveents.
#[derive(Clone, Debug, Default)]
pub struct PmPhysEnts { /* private */ }

impl PmPhysEnts {
    pub fn num_physents(&self) -> usize;
    pub fn num_moveents(&self) -> usize;
    /// Physent `index` (`physents[index]`), `None` when out of range.
    pub fn physent(&self, index: i32) -> Option<PhysEntView>;
    pub fn moveent(&self, index: usize) -> Option<PhysEntView>;
    /// The edict behind `physents[index].info` (`EDICT_NUM(info)`), `None` for a bad index.
    pub fn edict_of(&self, index: i32) -> Option<EdictId>;
}

#[derive(Clone, Copy, Debug)]
pub struct PhysEntView {
    pub info: i32, pub player: i32, pub origin: Vec3, pub angles: Vec3,
    pub mins: Vec3, pub maxs: Vec3, pub solid: i32, pub skin: i32, pub rendermode: i32,
    pub movetype: i32, pub is_brush_model: bool, pub model_mins: Vec3, pub model_maxs: Vec3,
}

/// `pmtrace_t` (pmtrace.h) without `deltavelocity` (pm fills it).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PmTraceResult {
    pub allsolid: bool, pub startsolid: bool, pub inopen: bool, pub inwater: bool,
    pub fraction: f32, pub endpos: Vec3, pub plane_normal: Vec3, pub plane_dist: f32,
    pub ent: i32, pub hitgroup: i32,
}

impl EngineApi<'_> {
    /// `SV_AddLinksToPM` for `player` (ReHLDS sv_user.cpp): world first, then the area-tree walk
    /// within ±256 of the player; ladders (SOLID_NOT, skin CONTENTS_LADDER) only into moveents;
    /// other players at their true (not lag-rewound) origin; dead players skipped.
    pub fn pm_physents(&self, player: EdictId) -> PmPhysEnts;
    /// `_PM_PlayerTrace` (pmovetst.cpp): every physent in order, hull map {0→1,1→3,2→0,3→2},
    /// box hulls for non-BSP, rotated BSP, PM_WORLD_ONLY / PM_GLASS_IGNORE / PM_STUDIO_IGNORE.
    pub fn pm_player_trace(&self, pe: &PmPhysEnts, start: Vec3, end: Vec3, usehull: i32,
                           trace_flags: i32, ignore_pe: i32) -> PmTraceResult;
    /// `_PM_TestPlayerPosition`: blocking physent index or -1, and the trace the original
    /// fills (quirk: traced at `trace_origin` = pmove->origin, not at `pos`).
    pub fn pm_test_player_position(&self, pe: &PmPhysEnts, trace_origin: Vec3, pos: Vec3,
                                   usehull: i32) -> (i32, PmTraceResult);
    /// `PM_PointContents`: (contents, truecontents).
    pub fn pm_point_contents(&self, pe: &PmPhysEnts, p: Vec3) -> (i32, i32);
    /// `PM_Ladder`'s test for moveent `index`: brush model, skin CONTENTS_LADDER, and
    /// `PM_HullPointContents(PM_HullForBsp(pe), origin - offset) != CONTENTS_EMPTY`.
    pub fn pm_ladder_contains(&self, pe: &PmPhysEnts, index: usize, origin: Vec3,
                              usehull: i32) -> bool;
    /// `PM_TraceModel` against moveent `index` (point hull): (fraction, plane normal).
    pub fn pm_trace_model(&self, pe: &PmPhysEnts, index: usize, start: Vec3, end: Vec3)
        -> (f32, Vec3);
}
```

Перенести из `rh_pmovetst.cpp`:
- `PM_RecursiveHullCheck`: `DIST_EPSILON`, откат на 0.05, `Trace backed up past 0.0`;
- `PM_HullPointContents`, `PM_HullForBox`, `PM_HullForBsp`;
- `_PM_PlayerTrace`: `allsolid` ⇒ `startsolid`, `startsolid` ⇒ `fraction` 0, пересчёт `endpos`, выигрывает строгое `<`, при промахе нормаль 0;
- `_PM_TestPlayerPosition`, `PM_PointContents`, `PM_TraceModel`.

Из `rh_sv_user.cpp` — `SV_AddLinksToPM`/`SV_AddLinksToPM_`, `SV_IsCollidableInPM`, `SV_CopyEdictToPhysent`, порядок обхода дерева областей.

Старый `trace_player_move` остаётся для `WalkGame`. Если общая часть совпадает, `pm_player_trace` переиспользует её, не дублирует.

- [ ] **Step 1: Тесты:**
  - `physents_start_with_the_world_and_keep_ladders_apart` — `func_ladder` только в moveents;
  - `dead_and_distant_entities_are_not_physents`;
  - `pm_trace_maps_usehull_to_bsp_hulls` — присевший проходит под препятствием 40 ед., стоящий нет;
  - `pm_trace_flags_world_only_and_glass` — стекло с `rendermode` ≠ 0 пропускается при `PM_GLASS_IGNORE`;
  - `pm_trace_quirks` — `startsolid` ⇒ `fraction` 0, промах ⇒ нормаль 0 и `ent` −1, выигрывает первый при равенстве;
  - `test_player_position_traces_at_pm_origin`;
  - `point_contents_sees_water_brushes` — `func_water` даёт свой `skin`, течения сворачиваются в WATER, `truecontents` сохраняется;
  - `ladder_contains_and_trace_model`;
  - `pm_services_refuse_bad_input` — NaN, индекс вне диапазона, устаревший edict; паники нет, результат по умолчанию.

  Стаб-карта из `cs-engine/tests/common`; где нужна реальная карта — de_dust2, без игры пропуск.
- [ ] **Step 2–4:** FAIL → реализовать → PASS.
- [ ] **Step 5: Commit** `feat(engine): pmove physents, player traces and contents as ReHLDS`

### Task 3: Движок — путь команды, звук `pmove`, `TraceTexture`, поля игрока

**Files:**
- Create: `crates/cs-engine/src/texture.rs`, `crates/cs-engine/tests/pm_command_path.rs`
- Modify: `crates/cs-engine/src/usercmd.rs`, `crates/cs-engine/src/physics.rs`, `crates/cs-engine/src/edict.rs`, `crates/cs-engine/src/api_messages.rs`, `crates/cs-engine/src/map.rs` (если для `TraceTexture` нужны узлы и поверхности), `crates/cs-engine/src/lib.rs`, `crates/cs-proto48/src/delta.rs`

**Interfaces — Produces:**

```rust
// EntVars (edict.rs) — new fields (entvars_t names in comments), reset by ED_ClearEdict like the rest:
pub b_in_duck: bool,          // bInDuck
pub fl_duck_time: i32,        // flDuckTime (int in entvars)
pub fl_time_step_sound: i32,  // flTimeStepSound
pub i_step_left: i32,         // iStepLeft
pub fl_fall_velocity: f32,    // flFallVelocity
pub fl_swim_time: i32,        // flSwimTime
pub oldbuttons: i32,          // oldbuttons
pub button: i32,              // button (if not present)
pub teleport_time: f32,       // teleport_time (= waterjumptime for players)
pub fuser1: f32, pub fuser2: f32, pub fuser3: f32, pub fuser4: f32,
pub vuser1: Vec3, pub vuser2: Vec3, pub vuser3: Vec3, pub vuser4: Vec3,
pub pushmsec: i32,

// GoldSrcClientData (cs-proto48 delta.rs) — the fields the clientdata_t schema already carries:
pub fl_swim_time: i32, pub waterjumptime: i32, pub pushmsec: i32,
pub fuser1: f32, pub fuser2: f32, pub fuser3: f32,
pub vuser2: Vec3, pub vuser3: Vec3, pub vuser4: Vec3,   // only the axes the schema has

impl EngineApi<'_> {
    /// `PM_SV_PlaySound` (ReHLDS sv_pmove.cpp): `SV_StartSound(1, player, …)` — PAS multicast
    /// from the player's centre to every client except the player's own (`MSG_FL_ONE` skips host_client);
    /// volume sent as `(int)(volume * 255)`.
    pub fn pm_play_sound(&mut self, player: EdictId, channel: i32, sample: &str, volume: f32,
                         attenuation: f32, flags: i32, pitch: i32) -> bool;
    /// `pfnTraceTexture` / ReHLDS `TraceTexture`: name of the texture of the BSP face of
    /// `ent`'s model hit by the line start→end, `None` when there is none.
    pub fn trace_texture(&self, ent: EdictId, start: Vec3, end: Vec3) -> Option<String>;
}
```

Перенести:
- `SV_RunCmd` (`rh_sv_user.cpp`), часть движка:
  - команда с `msec` 0 доходит до `run_usercmd`, `frametime` 0. Сейчас она пропускается в `handle_move`; при этом сохраняются бюджет защиты от спидхака и деление больше 50 мс пополам;
  - `SV_CheckMovingGround(player, frametime)` для игрока перед вызовом игры: конвейер под ногами, `FL_BASEVELOCITY`, сворачивание `basevelocity` в `velocity`;
  - `pmove->oldangles`, `v.button` — где их ставит движок, а не игра.
- `PM_SV_PlaySound` и `SV_StartSound` с `recipients == 1`.
- `TraceTexture` (ReHLDS `pr_cmds.cpp`/`world.cpp`: `SurfaceAtPoint` по узлам модели). Имя текстуры отдаётся как в BSP, без отрезания префиксов: их режет `pm`.
- Новые поля clientdata в `to_delta`/`from_delta` по схеме `CS16_DEFAULT_DELTA_LST`.

- [ ] **Step 1: Тесты:**
  - `zero_msec_commands_reach_the_game` — счётчик вызовов `run_usercmd` у тестовой игры растёт, `frametime` 0;
  - `a_conveyor_moves_a_standing_player` — `basevelocity` и `FL_BASEVELOCITY`;
  - `pm_sound_skips_the_mover` — два probe, звук получает только второй, громкость `(int)(v*255)`;
  - `trace_texture_names_the_floor` — de_dust2, без игры пропуск; `None` для энтити без брашевой модели и для NaN;
  - `new_clientdata_fields_round_trip`.
- [ ] **Step 2–4:** FAIL → реализовать → PASS. Тесты бюджета и деления в `usercmd.rs` остаются зелёными: поменять только то, что меняет пропуск `msec` 0, и объяснить в отчёте.
- [ ] **Step 5: Commit** `feat(engine): SV_RunCmd zero-msec commands, moving ground, pmove sound and TraceTexture`

### Task 4: `cs_sim::pm` — типы, математика, `PM_Move` и ядро ходьбы

**Files:**
- Create:
  - `crates/cs-sim/src/pm/mod.rs` (`pm_init`, `pm_move`, `PM_PlayerMove`);
  - `types.rs` (`PlayerMove`, `PmCmd`, `PmTrace`, `MoveVars`, `PmWorld`, `PmShared`, `PmCtx`);
  - `consts.rs`, `math.rs` (`pm_math.cpp`);
  - `stuck.rs` (`PM_CreateStuckTable`, `PM_CheckStuck`, смещения);
  - `walk.rs` (`ClipVelocity`, `FlyMove`, `Accelerate`, `AirAccelerate`, `AirMove`, `Friction`, `WalkMove`, `AddToTouched`, `CheckVelocity`, гравитация);
  - `position.rs` (`CheckParameters`, `CalcRoll`, `DropPunchAngle`, `ReduceTimers`, `CategorizePosition`, `CheckWater`, `InWater`, `CheckFalling`);
  - `toss.rs` (`NoClip`, `PushEntity`, `Physics_Toss`, `AddGravity`);
  - заглушки `duck.rs`, `jump.rs`, `ladder.rs`, `water.rs`, `spectator.rs`, `sound.rs`, `materials.rs`;
  - `crates/cs-sim/tests/pm_support/mod.rs` (`BoxWorld`), `crates/cs-sim/tests/pm_walk.rs`.
- Modify: `crates/cs-sim/src/lib.rs` (`pub mod pm;`)

**Interfaces — Produces:**

```rust
pub struct MoveVars { pub gravity: f32, pub stopspeed: f32, pub maxspeed: f32,
    pub spectatormaxspeed: f32, pub accelerate: f32, pub airaccelerate: f32,
    pub wateraccelerate: f32, pub friction: f32, pub edgefriction: f32, pub waterfriction: f32,
    pub entgravity: f32, pub bounce: f32, pub stepsize: f32, pub maxvelocity: f32,
    pub zmax: f32, pub wave_height: f32, pub footsteps: bool, pub rollangle: f32,
    pub rollspeed: f32 }                       // movevars_t without the sky fields; Default = ReHLDS defaults
pub struct PmCmd { pub msec: u8, pub viewangles: Vec3, pub forwardmove: f32,
    pub sidemove: f32, pub upmove: f32, pub buttons: u16, pub impulse: u8 }
pub struct PmTrace { pub allsolid: bool, pub startsolid: bool, pub inopen: bool,
    pub inwater: bool, pub fraction: f32, pub endpos: Vec3, pub plane_normal: Vec3,
    pub plane_dist: f32, pub ent: i32, pub deltavelocity: Vec3, pub hitgroup: i32 }
/// playermove_t (pm_defs.h) minus physents/visents/callbacks; field names snake_case of the C names.
pub struct PlayerMove { pub player_index: i32, pub server: bool, pub multiplayer: bool,
    pub time: f32, pub frametime: f32, pub forward: Vec3, pub right: Vec3, pub up: Vec3,
    pub origin: Vec3, pub angles: Vec3, pub oldangles: Vec3, pub velocity: Vec3,
    pub movedir: Vec3, pub basevelocity: Vec3, pub view_ofs: Vec3, pub fl_duck_time: f32,
    pub b_in_duck: bool, pub fl_time_step_sound: i32, pub i_step_left: i32,
    pub fl_fall_velocity: f32, pub punchangle: Vec3, pub fl_swim_time: f32,
    pub fl_next_primary_attack: f32, pub effects: i32, pub flags: i32, pub usehull: i32,
    pub gravity: f32, pub friction: f32, pub oldbuttons: i32, pub waterjumptime: f32,
    pub dead: bool, pub deadflag: i32, pub spectator: i32, pub movetype: i32,
    pub onground: i32, pub waterlevel: i32, pub watertype: i32, pub oldwaterlevel: i32,
    pub sztexturename: String, pub chtexturetype: u8, pub maxspeed: f32,
    pub clientmaxspeed: f32, pub iuser1: i32, pub iuser2: i32, pub iuser3: i32,
    pub iuser4: i32, pub fuser1: f32, pub fuser2: f32, pub fuser3: f32, pub fuser4: f32,
    pub vuser1: Vec3, pub vuser2: Vec3, pub vuser3: Vec3, pub vuser4: Vec3,
    pub cmd: PmCmd, pub touchindex: Vec<PmTrace>, pub movevars: MoveVars,
    pub player_mins: [Vec3; 4], pub player_maxs: [Vec3; 4], pub runfuncs: bool }

pub trait PmWorld {
    fn player_trace(&mut self, start: Vec3, end: Vec3, usehull: i32, trace_flags: i32, ignore_pe: i32) -> PmTrace;
    fn test_player_position(&mut self, pm_origin: Vec3, pos: Vec3, usehull: i32) -> (i32, PmTrace);
    fn point_contents(&mut self, p: Vec3) -> (i32, i32);
    fn num_moveents(&self) -> usize;
    fn ladder_contains(&mut self, moveent: usize, origin: Vec3, usehull: i32) -> bool;
    fn moveent_bounds(&self, moveent: usize) -> (Vec3, Vec3);
    fn moveent_origin(&self, moveent: usize) -> Vec3;
    fn trace_model(&mut self, moveent: usize, start: Vec3, end: Vec3) -> (f32, Vec3);
    fn trace_texture(&mut self, ground: i32, start: Vec3, end: Vec3) -> Option<String>;
    fn play_sound(&mut self, channel: i32, sample: &str, volume: f32, attenuation: f32, flags: i32, pitch: i32);
    fn random_long(&mut self, low: i32, high: i32) -> i32;
    fn sys_float_time(&mut self) -> f64;
    /// Origin and angles of the first physent whose `info == info` (PM_SpectatorMove tracking).
    fn physent_by_info(&self, info: i32) -> Option<(Vec3, Vec3)>;
}

/// pm_shared.cpp globals and statics: stuck table and per-client stuck state, texture table,
/// iSkipStep and the other function statics. One per game (server) or per client.
pub struct PmShared { /* stuck table, rgStuckLast, rgStuckCheckTime, textures, statics */ }

pub(crate) struct PmCtx<'a> { pub pm: &'a mut PlayerMove, pub sh: &'a mut PmShared, pub w: &'a mut dyn PmWorld }

/// PM_Init: stuck table + PM_InitTextureTypes over the bytes of sound/materials.txt (None = load failed).
pub fn pm_init(materials_txt: Option<&[u8]>) -> PmShared;
/// PM_Move (pm_shared.cpp): PM_PlayerMove(server) and the FL_ONGROUND / single-player friction tail.
pub fn pm_move(pm: &mut PlayerMove, sh: &mut PmShared, world: &mut dyn PmWorld);
/// PM_FindTextureType (exported to the engine for bullet materials).
pub fn pm_find_texture_type(sh: &PmShared, name: &str) -> u8;
```

**Заглушки для волны 2.** Сигнатуры именно такие; тела — `todo`-free: пустые или возвращают значение по умолчанию; doc «stub: Task N». `mod.rs` вызывает их в порядке `PM_PlayerMove`.

| Файл | Функции | Задача |
|---|---|---|
| `duck.rs` | `pm_duck(c)`, `pm_unduck(c)`, `pm_fix_player_crouch_stuck(c, direction: i32)`, `pm_spline_fraction(value: f32, scale: f32) -> f32` | 5 |
| `jump.rs` | `pm_jump(c)`, `pm_prevent_mega_bunny_jumping(c)` | 5 |
| `ladder.rs` | `pm_ladder(c) -> Option<usize>`, `pm_ladder_move(c, ladder: usize)` | 6 |
| `water.rs` | `pm_check_water_jump(c)`, `pm_water_jump(c)`, `pm_water_move(c)`, `pm_play_water_sounds(c)` | 6 |
| `spectator.rs` | `pm_spectator_move(c)` | 6 |
| `sound.rs` | `pm_update_step_sound(c)`, `pm_play_step_sound(c, step: i32, fvol: f32)`, `pm_catagorize_texture_type(c)`, `pm_map_texture_type_step_type(ch: u8) -> i32` | 7 |
| `materials.rs` | `pub struct TextureTable`, `TextureTable::parse(bytes: &[u8]) -> TextureTable`, `TextureTable::find(&self, name: &str) -> u8` — заглушка возвращает `b'C'` | 7 |

Всё, где `c: &mut PmCtx<'_>`, — `pub(crate)`.

Задачи 5–7 меняют только свои файлы и свои тесты. Если источнику нужен ещё метод мира, его добавляют в `PmWorld` с реализацией по умолчанию и пишут об этом в отчёте.

`BoxWorld` (`tests/pm_support/mod.rs`):
- осевые твёрдые боксы с именем текстуры;
- объёмы воды с `contents` и `truecontents`, с течениями;
- лестницы как moveents с нормалью для `trace_model`;
- `physent_by_info`;
- журнал звуков;
- детерминированный `random_long` (очередь значений);
- управляемое `sys_float_time`.

Трасса — разнесённый по хуллу AABB против боксов с отступом `DIST_EPSILON`, как у `PM_RecursiveHullCheck`. Точность до бита не нужна: это испытательный мир для логики `pm`. Промах — нормаль 0, `ent` −1.

- [ ] **Step 1: Тесты** (`pm_walk.rs`; ожидаемые числа — вручную по формулам источника, формула в комментарии к тесту):
  - `player_move_runs_in_source_order` — журнал вызовов мира на одном кадре совпадает с порядком `PM_PlayerMove`;
  - `ground_acceleration_matches_pm_accelerate` — первая команда 10 мс с `forwardmove` 250, `maxspeed` 250, `accelerate` 5 даёт ровно 12.5;
  - `running_converges_to_maxspeed`;
  - `friction_uses_stopspeed_control` — 75, точные первые шаги;
  - `edge_friction_applies_over_a_drop`;
  - `steps_up_to_stepsize_and_not_higher` — 18 да, 19 нет;
  - `wall_slide_clips_velocity`;
  - `falling_uses_half_step_gravity` — скорость после N команд точно;
  - `landing_sets_punch_and_clears_fall_velocity` — `punchangle.z = fall * 0.013`;
  - `no_hit_trace_leaves_player_airborne`;
  - `stuck_player_is_nudged_by_the_table`;
  - `noclip_and_toss_move_as_source`;
  - `check_parameters_scales_and_rolls`;
  - `punch_angle_decays`;
  - `timers_reduce_by_msec`;
  - `msec_zero_changes_no_position`;
  - `nan_velocity_is_reset`;
  - `touches_are_recorded_once_per_entity`;
  - `hostile_cmds_do_not_break_the_move` — NaN, ±1e30, 0xFFFF; конечные `origin` и `velocity`, паники нет.
- [ ] **Step 2–4:** FAIL → реализовать → PASS. `cs-sim` проходит wasm-check.
- [ ] **Step 5: Commit** `feat(sim): pm_shared core — PM_Move, walking, air, fly, stuck, toss`

### Task 5: `cs_sim::pm` — присед и прыжок

**Files:** Modify `crates/cs-sim/src/pm/duck.rs`, `crates/cs-sim/src/pm/jump.rs`; Create `crates/cs-sim/tests/pm_duck_jump.rs`

Перенести:
- `PM_Duck`, `PM_UnDuck`, `PM_FixPlayerCrouchStuck`, `PM_SplineFraction`:
  - присед в две фазы: `flDuckTime` 1000, `TIME_TO_DUCK` 0.4, сплайн `view_ofs` от 17 к 12;
  - на земле `origin.z −= 18`, в воздухе присед сразу;
  - `usehull` 1, ходы × 0.333 при приседе.
- `PM_UnDuck`: проверка трассой, повтор при `startsolid`, `flTimeStepSound −= 100`.
- `PM_Jump` (ванильная ветка) и `PM_PreventMegaBunnyJumping`:
  - нет автобхопа;
  - нет прыжка при `bInDuck && FL_DUCKING`;
  - высота `sqrt(2*800*45)`, константа 800;
  - `fuser2` 1315.789429 и множитель `(100 − fuser2*0.001*19)*0.01`;
  - шаг при прыжке на скорости ≥ 150 через `pm_play_step_sound`;
  - прыжок в воде: 100, 80 или 50, звуки `wade`.

`HalfHumanHeight` и другие значения, которых нет в описи, — проверить по источнику.

- [ ] **Step 1: Тесты:**
  - `duck_takes_four_tenths_and_follows_the_spline` — `view_ofs` на 100, 200 и 300 мс по формуле, на 400 мс `FL_DUCKING`, хулл 1, `origin` −18;
  - `duck_in_the_air_is_immediate`;
  - `unduck_is_blocked_under_a_low_ceiling`;
  - `jump_speed_is_268_and_stamina_reduces_the_next`;
  - `holding_jump_does_not_rejump`;
  - `mega_bunny_jumping_is_capped`;
  - `duck_jump_rules_as_source`;
  - `jumping_in_deep_water_swims_up` — 100/80/50 по типу жидкости, `wade` раз в секунду по `flSwimTime`.
- [ ] **Step 2–5:** FAIL → реализовать → PASS → Commit `feat(sim): pm duck and jump as pm_shared`

### Task 6: `cs_sim::pm` — лестницы, вода, наблюдатель

**Files:** Modify `crates/cs-sim/src/pm/ladder.rs`, `crates/cs-sim/src/pm/water.rs`, `crates/cs-sim/src/pm/spectator.rs`; Create `crates/cs-sim/tests/pm_ladder_water.rs`

Перенести:
- `PM_Ladder`, `PM_LadderMove`: нормаль через `trace_model`, скорость `MAX_CLIMB_SPEED` 200 (× 0.333 при приседе), прыжок с лестницы `normal*270`, `movetype` FLY, гравитация 0.
- `PM_CheckWaterJump`, `PM_WaterJump`, `PM_WaterMove` (опускание −60, × 0.8, трение воды, проба ступени), `PM_PlayWaterSounds`.
- `PM_SpectatorMove`: `OBS_ROAMING` с трением × 1.5 и `spectatormaxspeed`, прочие режимы следуют за physent с `info == iuser2`; клиентская ветка `iJumpSpectator` не переносится.

- [ ] **Step 1: Тесты:**
  - `climbs_a_ladder_at_200`;
  - `jumps_off_a_ladder_along_its_normal`;
  - `water_levels_one_two_three`;
  - `swimming_sinks_and_is_slower`;
  - `water_jump_lifts_out_at_a_ledge`;
  - `currents_push_through_basevelocity`;
  - `entering_and_leaving_water_plays_wade`;
  - `roaming_spectator_flies_at_spectatormaxspeed`;
  - `chasing_spectator_takes_the_target_origin`.
- [ ] **Step 2–5:** FAIL → реализовать → PASS → Commit `feat(sim): pm ladders, water and spectator as pm_shared`

### Task 7: `cs_sim::pm` — звуки шагов и материалы

**Files:** Modify `crates/cs-sim/src/pm/sound.rs`, `crates/cs-sim/src/pm/materials.rs`; Create `crates/cs-sim/tests/pm_sound.rs`

Перенести:
- `PM_InitTextureTypes`, `PM_SortTextures`, `PM_SwapTextures`, `PM_FindTextureType`. Все причуды разбора:
  - комментарии;
  - не-буква в начале строки — строка пропускается;
  - имя без следующего символа — строка пропускается;
  - 16 символов;
  - не больше 1024 строк;
  - сортировка `stricmp`, двоичный поиск;
  - промах даёт `C`;
  - загрузка не удалась — таблица пуста.
- `PM_CatagorizeTextureType`: трасса вниз на 64, отрезание префиксов `-`, `+` и `{!~ `.
- `PM_MapTextureTypeStepType`.
- `PM_PlayStepSound`:
  - `iStepLeft` переключается до проверки `runfuncs`;
  - `mp_footsteps` в мультиплеере;
  - плитка 1 из 5;
  - `wade` пропускает 3 из 4 через статик в `PmShared`;
  - таблица сэмплов и громкостей.
- `PM_UpdateStepSound`: тайминги 300/350/600/400, +100 при приседе или на лестнице, громкость × 0.35, вода по колено и по щиколотку.
- Звуковая часть `PM_CheckFalling` идёт через эти функции; сам `CheckFalling` — в задаче 4.

- [ ] **Step 1: Тесты:**
  - `materials_parse_like_pm_init_texture_types` — каждая причуда отдельным утверждением;
  - `texture_prefixes_are_stripped`;
  - `step_sounds_alternate_feet_by_material`;
  - `step_timing_and_volume_as_source`;
  - `wade_skips_three_of_four`;
  - `footsteps_off_in_multiplayer_is_silent`;
  - `runfuncs_off_still_toggles_the_foot`;
  - `landing_volume_thresholds` — 580, 290, 350 через полный `pm_move` с падением.
- [ ] **Step 2–5:** FAIL → реализовать → PASS → Commit `feat(sim): pm step sounds and texture materials`

### Task 8: `cs-game` — `SV_RunCmd` на `cs_sim::pm`

**Files:**
- Create: `crates/cs-game/src/pmove.rs` — `EnginePmWorld`, `setup_pmove` (`SV_SetupPMove`), `finish_pmove` (`SV_FinishPMove`), касания, CmdStart/CmdEnd; `crates/cs-game/tests/movement.rs`
- Modify: `crates/cs-game/src/player.rs` (`run_usercmd`, `move_player` уходит, `client_data`, `spawn`), `crates/cs-game/src/lib.rs` (`PmShared` в `GameState` и `pm_init` по `sound/materials.txt` через VFS, `move_params` уходит из движения игрока), `crates/cs-sim/src/pm/consts.rs` (хуллы игрока реэкспортируются из `cs_engine::PM_PLAYER_MINS/MAXS`, своей копии нет)

**Interfaces — Consumes:** задачи 1–7. **Produces:** `pub(crate) fn run_pmove(g, eng, slot, cmd)` в `pmove.rs`.

Перенести:
1. Порядок `SV_RunCmd` (ReHLDS `sv_user.cpp`) — в той части, что в нашей архитектуре делает игра:
   - CmdStart (`random_seed`);
   - `oldangles = v_angle`, `v_angle = cmd.viewangles` при `fixangle == 0`;
   - PlayerPreThink;
   - `SV_SetupPMove` — все поля из edict и `movevars` движка (`eng.move_vars()`), `server = true`, `multiplayer = maxclients > 1`, `usehull` по `FL_DUCKING`, `maxspeed = sv_maxspeed`, `clientmaxspeed = pev->maxspeed`, physents от `eng.pm_physents`;
   - `pm_move`;
   - `SV_FinishPMove` — все поля назад, усечение `(int)` для `flDuckTime`/`flSwimTime`, `groundentity` из `physents[onground].info`, `v.angles = (-pitch/3, yaw, roll)`, размеры по `player_mins/maxs[usehull]`;
   - касания из `touchindex` в `eng.player_touched`;
   - PlayerPostThink;
   - CmdEnd: при `FL_DUCKING` размер `VEC_DUCK_HULL`.
2. Команда с `msec` 0 тоже идёт полностью: CARRY 53, со стороны движка — задача 3.
3. `PlayerPreThink`/`PostThink` (`player.cpp`):
   - `m_flFallVelocity = -velocity.z` до движения;
   - урон от падения по значению игры;
   - удар при приземлении приходит из `pm` через `punchangle`;
   - `m_flVelocityModifier`: поле и применение в PreThink; ставит его 2D.
4. `client_data` (`client.cpp UpdateClientData`): `bInDuck`, `flDuckTime`, `flTimeStepSound`, `flSwimTime`, `waterjumptime = (int)teleport_time`, `watertype` (если есть в структуре), `fuser1..3`, `pushmsec`, бит `iuser3` FREEZE_TIME_OVER (CAN_SHOOT, BOMB_ZONE, SHIELD — 2D/2E, записать в ledger), `physinfo`. Для `OBS_IN_EYE` — как в источнике.
5. `EnginePmWorld`:
   - `play_sound` → `eng.pm_play_sound`;
   - `trace_texture` → `eng.trace_texture(physent edict)` с правилами `PM_SV_TraceTexture`;
   - `random_long` → RNG движка;
   - `sys_float_time` → время движка;
   - `physent_by_info` → `PmPhysEnts`.
6. Тесты, привязанные к старой модели, переписываются под источник:
   - `fall_damage_as_regamedll` меняет `sv_gravity`, а не `move_params`;
   - `buy.rs` и `rounds.rs` используют `sv_gravity 0` или стаб с полом.

   Каждая переписанная проверка указана в отчёте с причиной.

- [ ] **Step 1: Тесты** (`movement.rs`, Bench и de_dust2, без игры пропуск):
  - `run_cmd_follows_sv_runcmd` — порядок PreThink → move → PostThink и поля edict после команды;
  - `walking_reaches_250_on_dust2`;
  - `jump_apex_is_45_units`;
  - `duck_view_and_hull_over_four_tenths`;
  - `stairs_are_climbed`;
  - `landing_punch_reaches_client_data`;
  - `client_data_carries_duck_step_swim_fields`;
  - `zero_msec_command_runs_think`;
  - `hostile_usercmds_keep_the_player_finite`;
  - `players_block_each_other_through_physents`.
- [ ] **Step 2–4:** FAIL → реализовать → PASS. `WalkGame` и `legacy_numbers_unchanged` не тронуты.
- [ ] **Step 5: Commit** `feat(game): player movement through pm_shared and SV_RunCmd`

### Task 9: `cs-game` — наблюдатель, жёсткие cvar, вода, звуки, лестницы и вода на картах

**Files:**
- Modify: `crates/cs-game/src/observer.rs` (движение наблюдателя через `pm`: `iuser1 > 0` → `PM_SpectatorMove`; свой `roam` уходит), `crates/cs-game/src/rules.rs`, `crates/cs-game/src/player.rs` (`WaterMove`), `crates/cs-game/src/world.rs` (прекэш звуков `pm`, если чего-то нет)
- Create: `crates/cs-game/tests/movement_world.rs`

Перенести:
- Жёсткие значения `RestartRound` и `Think` раз в секунду (`multiplay_gamerules.cpp`): `sv_accelerate` 5, `sv_friction` 4, `sv_stopspeed` 75. Закрывает CARRY 59.
- `CBasePlayer::WaterMove` (`player.cpp`): `air_finished`, урон утопления, `pain_finished`, звуки, `DMG_DROWN`, восстановление здоровья после утопления, если оно в ванили. Закрывает F15 из 2C.
- Наблюдатель:
  - roaming и следование за целью через `pm`;
  - начало `observer.rs` (выбор режима, цели) остаётся;
  - `UTIL_SetOrigin` наблюдателя за целью в `ClientSetupVisibility` (CARRY 80), если `pm` этого не покрывает.
- Прекэш всех сэмплов из таблиц `pm`: `pl_step*`, `pl_metal*`, `pl_dirt*`, `pl_duct*`, `pl_grate*`, `pl_tile*`, `pl_slosh*`, `pl_wade*`, `pl_ladder*`, `pl_snow*` — по `ClientPrecache`/`W_Precache` источника.

- [ ] **Step 1: Тесты:**
  - `movement_cvars_are_hard_locked` — `sv_accelerate 10` возвращается к 5 за секунду и при рестарте;
  - `drowning_as_regamedll` — стаб с водой: урон после 12 с под водой, интервал по источнику;
  - `footsteps_reach_others_not_self` — два игрока, de_dust2;
  - `a_ladder_is_climbed_on_a_real_map` — найти стоковую карту с `func_ladder`, без неё пропуск;
  - `water_is_swum_on_a_real_map` — карта с водой;
  - `observer_roams_and_chases_through_pm`;
  - тесты `observer.rs` из 2C зелёные; переписанные — с причиной.
- [ ] **Step 2–5:** FAIL → реализовать → PASS → Commit `feat(game): hard-locked movement cvars, drowning, observer and footsteps on pm`

### Task 10: Документы, реестр, чек-лист

**Files:**
- Create: `docs/parity/movement.toml` (категория `movement`, вес 8 из `weights.toml`: пункты по группам функций `pm_shared` и сервисам движка, статусы `tested`/`implemented` с тестами в evidence), `docs/parity/checklists/pm-movement.md` (проверка владельцем с оригинальным клиентом)
- Modify: `docs/parity/entities.toml` (снять «movement is cs_sim::pmove»), `docs/parity/engine.toml` (`movevars`, `player_movement_trace`, `FMOVE_IGNORE_GLASS`/`PM_GLASS_IGNORE`), `docs/parity/delta.toml` (`flSwimTime`, `waterjumptime` и др.), `docs/parity/cvars.toml` (новые cvar, если задача 1 не закрыла), `THIRD-PARTY-NOTICES.md` (`pm_shared.cpp`, `pm_math.cpp`, заголовки `pm_*.h`, ReHLDS `pmove.cpp`, `pmovetst.cpp`, `sv_user.cpp`, `sv_pmove.cpp`, `sv_main.cpp` — с заголовками лицензий из скачанных файлов), `docs/LICENSING.md`, `README.md`, `docs/parity/README.md`

`xtask parity` проходит: каждый `path::fn` из evidence существует.

Чек-лист — оригинальный клиент на нашем сервере:
- бег;
- прыжок, двойной прыжок (второй ниже), бхоп не набирает скорость;
- присед плавный, присед в прыжке;
- ступени и наклоны;
- лестница на карте с лестницей;
- вода;
- шаги других игроков по материалам;
- удар камеры при приземлении;
- нет дрожания при беге вдоль стены и на ступенях.

Невыверенные ожидания помечены «From memory - confirm». Средство показа ошибок предсказания (`cl_showerror` или иное) — тоже с этой пометкой.

- [ ] **Step 1–3:** Документы → `cargo xtask parity` → Commit `docs: pm_shared registry, checklist and notices`
