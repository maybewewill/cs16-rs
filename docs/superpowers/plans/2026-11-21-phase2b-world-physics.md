# Фаза 2B — мир и физика: план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Движок получает мир энтити как в HLDS: связывание энтити в area-дереве, трассы по энтити, кадр в порядке `SV_Frame`, физика по `movetype` (none, noclip, follow, toss/bounce/fly, step, push), касания и триггеры, толкатели с `blocked`, игроки как энтити мира, компенсация лага, PAS для звука, плюс сетевые хвосты фазы 2A.

**Architecture:** Каждая задача переносит поведение конкретных функций ReHLDS (GPL v2-or-later, порт разрешён, см. `docs/LICENSING.md`) в `crates/cs-engine`; исходник читается в задаче (ссылки ниже), каждая перенесённая функция называет в doc-комментарии файл ReHLDS и дату чтения. Игра по-прежнему общается с движком только через `EngineApi` и `GameDll`; вызовы игры из движка не вкладываются друг в друга — касания, найденные во время вызова игры, ставятся в очередь и раздаются сразу после возврата из него. `WalkGame` переходит на трассы по энтити.

**Tech Stack:** Rust 1.100.0 (edition 2024), `glam`, `cs-proto48`, `cs-engine`, `cs-sim`, `cs-probe`; ReHLDS как эталон поведения.

**Spec:** `docs/superpowers/specs/2026-11-20-cs16-rs-master-plan-design.md` (§4.3 граница движок↔игра, §6 «Фаза 2», п. 5 — компенсация лага); журнал фазы 2A `.superpowers/sdd/2026-11-21-phase2a-engine-services/progress.md` (отложенные пункты).

**Источник ReHLDS** (только чтение; запрос — один URL, без данных пользователя): `https://raw.githubusercontent.com/dreamstalker/rehlds/master/rehlds/engine/<file>` — `world.cpp`, `world.h`, `sv_phys.cpp`, `sv_move.cpp`, `sv_user.cpp`, `sv_main.cpp`, `pr_cmds.cpp`, `host.cpp`, `server.h`; `rehlds/common/qlimits.h`, `rehlds/common/const.h`. Скачанное кладётся в отдельную пустую папку во временном каталоге, в репозиторий не попадает.

## Global Constraints

- `unsafe_code = "forbid"`; `cargo clippy --workspace --all-targets -- -D warnings` и `cargo test --workspace` зелёные после каждой задачи; `cargo check --target wasm32-unknown-unknown` для библиотечных crates (строка из `.github/workflows/ci.yml`).
- Каждая команда cargo — из Git Bash ровно так: `nice -n 19 env CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 cargo <args>`, по одной за раз; никакого `cargo fmt`; ничего в release-профиле.
- Тулчейн Rust 1.100.0 (`rust-toolchain.toml`), `rust-version = "1.100"`.
- Никаких ассетов Valve, захватов и дампов в репозитории; тест, которому нужна установленная игра, без неё возвращается без проверок; найденная игра с нечитаемой картой — провал теста.
- Ни один вызов `EngineApi` не паникует на данных игры (устаревший `EdictId`, слот вне диапазона, нечисловые и огромные числа, длинные строки); в `EntVars` движок не записывает нечисловых значений.
- Утверждения о поведении оригинала: из исходника ReHLDS — с файлом и датой; по памяти — с пометкой «assumption».
- В реестре паритета `verified` только со ссылкой на тест или датированную строку чек-листа.
- Старый путь (`cs-net`, `MatchWorld`, офлайн-игра клиента) работает; его тесты не меняются.
- Лимит протокола: номер энтити — 11 бит, значит не больше 2048 edict'ов.

## Review Focus

1. Враждебная геометрия энтити (вывернутые или огромные `mins`/`maxs`, NaN в `angles`, номер браш-модели вне таблицы) — связывание и трассы не паникуют, вставка в area-дерево ограничена его глубиной. Тест: задача 2 `hostile_bounds_link_safely`, задача 3 `hostile_entities_trace_safely`.
2. Все энтити в одной точке (пул целиком, ~2000 энтити друг в друге) — трасса и раздача касаний ограничены (`MAX_TOUCHES_PER_FRAME`), кадр не зависает. Тест: задача 5 `touch_storm_is_bounded`.
3. Игра удаляет энтити внутри `touch`, `blocked`, `think` — в том числе ту, по которой идёт обход, и ту, что ещё в очереди касаний. Тест: задача 5 `removal_inside_touch_is_safe`, задача 8 `blocked_may_remove_the_pusher`.
4. Толкатель зажимает игрока в стене — вызывается `blocked`, позиции откатываются, нет NaN и бесконечного цикла. Тест: задача 8 `crushed_rider_reverts_the_push`.
5. Компенсация лага с враждебными значениями клиента (огромные `lerp_msec`, нулевой или огромный пинг, телепорт игрока) — откат не глубже `sv_maxunlag` и истории, позиции восстановлены точно после команды. Тест: задача 10 `unlag_bounds_and_restores_exactly`.

---

### Task 1: Сетевые хвосты фазы 2A

**Files:**
- Modify: `crates/cs-engine/src/server.rs` (`ServerConfig::sanitized`, `frame`), `signon.rs`, `api_messages.rs` (init-буфер), `frame.rs` (переполнение блока энтити), `lifecycle.rs`
- Test: `crates/cs-engine/tests/signon.rs`, `tests/frames.rs`, `tests/lifecycle.rs` (или существующие файлы этих тем)

**Interfaces:**
- Produces: `pub const MAX_EDICTS_WIRE: usize = 2048;` `pub const MAX_SIGNON_BUFFER: usize = 65536;` (общий бюджет init-сообщений и baselines). `MAX_INIT_BUFFER` удаляется.

Поведение (ReHLDS `sv_main.cpp`: `MSG_INIT` пишет в `g_psv.signon`, туда же `SV_CreateBaseline` после `pfnServerActivate` дописывает `svc_spawnbaseline`; размер `signonData[NET_MAX_PAYLOAD]` в сборке REHLDS_FIXES, `signon_data[32768]` в стоковой):
- Общий sign-on буфер: init-сообщения и baselines — в одном бюджете `MAX_SIGNON_BUFFER` минус хвост этапа `spawn` (setangle, time, userinfo, light styles, signonnum; посчитать максимум при текущих лимитах). `Init` сверх бюджета ведёт себя как `All` (как сейчас при переполнении). Baselines, не влезающие в остаток, не пишутся: начиная с самого большого индекса, одна строка `tracing::error!` с числом; клиент получает такие энтити полной дельтой от нулевого состояния.
- `ServerConfig::sanitized` ограничивает `max_edicts` сверху `MAX_EDICTS_WIRE`.
- `Server::frame` после `shutdown` ничего не делает, кроме счётчика `frames_skipped`.
- Переполнение блока энтити: прочитать `SV_WriteEntitiesToClient` и `SV_SendClientDatagram` (`sv_main.cpp`), повторить: не больше `MAX_PACKET_ENTITIES` энтити в паке (значение взять из исходника), и поведение при переполнении датаграммы (усечение списка или отказ от датаграммы — как в исходнике). Существующий тест `oversized_frame_drops_entities_but_keeps_clientdata` переписать под проверенное поведение.

- [ ] **Step 1: Падающие тесты:**

```rust
#[test] fn init_and_baselines_share_the_signon_budget()
// карта-заглушка с 1500 энтити с моделью + игра шлёт Init до заполнения: этап spawn ≤ 65536 байт,
// число записанных baselines + init байты ≤ MAX_SIGNON_BUFFER - хвост; probe входит в игру
#[test] fn max_edicts_is_capped_by_the_wire()                  // max_edicts 65535 → пул 2048
#[test] fn frame_after_shutdown_does_nothing()                 // start_frame не вызван, датаграммы не прочитаны
#[test] fn packet_entities_follow_the_original_overflow_rule()  // 600 видимых энтити → ≤ MAX_PACKET_ENTITIES в паке; правило переполнения из исходника
```

- [ ] **Step 2:** Run `cargo test -p cs-engine` → FAIL. **Step 3:** Реализовать. **Step 4:** Run → PASS; workspace, clippy, wasm.
- [ ] **Step 5: Commit** `fix(engine): shared sign-on budget, wire entity limit, overflow as HLDS`

### Task 2: Связывание энтити в мире

**Files:**
- Create: `crates/cs-engine/src/world.rs`, `crates/cs-engine/tests/world_links.rs`
- Modify: `edict.rs` (`EntVars`), `api_entities.rs`, `mapents.rs`, `lifecycle.rs`, `server.rs`

**Interfaces:**
- Produces:

```rust
// EntVars += pub absmin: Vec3, pub absmax: Vec3, pub size: Vec3, pub avelocity: Vec3, pub basevelocity: Vec3,
//            pub groundentity: Option<EdictId>, pub watertype: i32, pub gravity: f32, pub friction: f32, pub ltime: f64
// solid: SOLID_NOT 0, SOLID_TRIGGER 1, SOLID_BBOX 2, SOLID_SLIDEBOX 3, SOLID_BSP 4
// movetype: NONE 0, WALK 3, STEP 4, FLY 5, TOSS 6, PUSH 7, NOCLIP 8, FLYMISSILE 9, BOUNCE 10, BOUNCEMISSILE 11, FOLLOW 12, PUSHSTEP 13
pub mod consts { pub const SOLID_*: u8; pub const MOVETYPE_*: u8; pub const FL_*: u32; } // значения из rehlds/common/const.h
pub const AREA_DEPTH: usize = 4;   // SV_CreateAreaNode
impl EngineApi<'_> {
    pub fn set_origin(&mut self, id: EdictId, origin: Vec3);          // PF_setorigin_I: пишет origin, SV_LinkEdict(e, FALSE)
    pub fn set_size(&mut self, id: EdictId, mins: Vec3, maxs: Vec3);  // PF_setsize_I → SetMinMaxSize: mins, maxs, size, SV_LinkEdict(e, FALSE);
                                                                      // mins > maxs по любой оси — в HLDS Host_Error, здесь тихий отказ
    pub fn link_entity(&mut self, id: EdictId, touch_triggers: bool); // SV_LinkEdict
    pub fn entities_in_box(&self, mins: Vec3, maxs: Vec3) -> Vec<EdictId>; // по area-дереву, по возрастанию индекса
}
```

Перенести из `world.cpp`: `SV_ClearWorld`, `SV_CreateAreaNode`, `SV_UnlinkEdict`, `SV_LinkEdict` (absmin/absmax: BSP с поворотом — описанный куб по `radius`; `FL_ITEM` — расширение на 15/1 как в исходнике; иначе ±1), `SV_FindTouchedLeafs` (номера листьев для PVS — замена приближения «9 точек» в `ServerMap::can_see` на проверку листьев энтити, как HLDS). Связи хранятся в движке (индексы, не указатели): список триггеров и список твёрдых в каждом узле. `change_map` пересоздаёт дерево по границам мира новой карты. Энтити карты связываются после `spawn_entity` (как `ED_LoadFromFile` → `DispatchSpawn` → связывание внутри `SET_ORIGIN`/`SET_MODEL`): `set_model` для `*N` и `set_size` связывают энтити. Касания триггеров (`touch_triggers = true`) в этой задаче только собираются в очередь; раздача — задача 5.

- [ ] **Step 1: Падающие тесты** (`tests/world_links.rs`):

```rust
#[test] fn linked_entity_has_absolute_bounds()           // origin (100,0,0), mins/maxs ±16 → absmin (83,-17,-17), absmax (117,17,17)
#[test] fn rotated_brush_bounds_cover_the_rotation()     // браш *1 с angles (0,45,0) на de_dust2 (пропуск без игры): absmin/absmax как в SV_LinkEdict
#[test] fn entities_in_box_finds_linked_only()            // несвязанная (SOLID_NOT без link) не находится; порядок по индексу
#[test] fn unlink_on_remove_and_on_map_change()
#[test] fn pvs_uses_entity_leaves()                       // энтити вдали за стеной на de_dust2 скрыта; длинная энтити, чей центр не виден, но край виден — видна
#[test] fn hostile_bounds_link_safely()                   // NaN/inf/1e30/вывернутые mins/maxs: set_size отказывает, link не паникует, absmin/absmax конечны
```

- [ ] **Step 2:** FAIL. **Step 3:** Реализовать. **Step 4:** PASS; существующие тесты PVS (`hidden_entity_is_not_sent`, `real_map_pvs_hides_far_spawn`) зелёные.
- [ ] **Step 5: Commit** `feat(engine): area nodes and entity linking as SV_LinkEdict`

### Task 3: Трассы по энтити

**Files:**
- Create: `crates/cs-engine/src/trace_ents.rs`, `crates/cs-engine/tests/entity_traces.rs`
- Modify: `collision.rs` (box hull, смещение/поворот браш-модели), `api_entities.rs`

**Interfaces:**
- Consumes: area-дерево задачи 2.
- Produces:

```rust
#[derive(Debug, Clone, Copy, PartialEq)] pub enum MoveFilter { Normal, NoMonsters, Missile }   // MOVE_NORMAL/NOMONSTERS/MISSILE
pub struct EntTrace { pub trace: TraceResult, pub ent: Option<EdictId> }                         // ent: кого задели (мир — world())
impl EngineApi<'_> {
    pub fn trace_line(&self, start: Vec3, end: Vec3, filter: MoveFilter, pass: Option<EdictId>) -> EntTrace;          // PF_traceline_DLL
    pub fn trace_hull_ents(&self, start: Vec3, end: Vec3, hull: Hull, filter: MoveFilter, pass: Option<EdictId>) -> EntTrace; // PF_TraceHull
    pub fn trace_box(&self, start: Vec3, mins: Vec3, maxs: Vec3, end: Vec3, filter: MoveFilter, pass: Option<EdictId>) -> EntTrace; // SV_Move
    pub fn trace_entity(&self, id: EdictId, start: Vec3, end: Vec3, hull: Hull) -> EntTrace;                         // PF_TraceModel / SV_ClipMoveToEntity
    pub fn point_contents_ents(&self, p: Vec3) -> i32;          // SV_PointContents (с func_water через SV_LinkContents)
    pub fn test_entity_position(&self, id: EdictId) -> Option<EdictId>; // SV_TestEntityPosition
}
```

Перенести: `SV_HullForBox`, `SV_HullForBsp`, `SV_HullForEntity` (BSP-энтити с `origin` и поворотом по `angles` — трасса в локальных координатах, плоскость обратно в мировые), `SV_SingleClipMoveToEntity`, `SV_ClipMoveToEntity`, `SV_ClipToLinks` (правила пропуска: `pass`, владелец `owner` в обе стороны, `SOLID_NOT`/`SOLID_TRIGGER`, `MoveFilter`, `FL_MONSTERCLIP` — как в исходнике), `SV_MoveBounds`, `SV_Move`, `SV_LinkContents`, `SV_PointContents`, `SV_TestEntityPosition`. Нечисловой или вне `MAX_WORLD_COORD` вход → «нет попадания», `end_pos = start` (как в задаче 3 фазы 2A). Существующие `trace_hull`/`trace_model` остаются (только мир / одна модель).

- [ ] **Step 1: Падающие тесты** (`tests/entity_traces.rs`, карта-заглушка + энтити-коробки, плюс de_dust2 с пропуском без игры):

```rust
#[test] fn line_hits_a_bbox_entity_and_reports_it()     // коробка ±16 в (100,0,0): трасса (0,0,0)→(200,0,0) fraction ≈ 84/200, ent = коробка
#[test] fn pass_and_owner_are_skipped()                  // pass = коробка → нет попадания; owner коробки = pass → нет попадания
#[test] fn triggers_and_not_solid_are_ignored()
#[test] fn hull_trace_against_entity_uses_expanded_box() // Standing hull по коробке: остановка на 16+16 раньше
#[test] fn moved_brush_entity_collides_at_its_origin()   // браш *N с origin (0,0,64): трасса останавливается на 64 выше, чем без смещения
#[test] fn rotated_brush_entity_collides_rotated()       // браш с angles (0,90,0): плоскость попадания повернута
#[test] fn world_answers_match_world_only_trace()        // без энтити trace_hull_ents == trace_hull (fraction, end, normal) на 50 трассах de_dust2
#[test] fn hostile_entities_trace_safely()               // энтити с NaN/огромными границами, 2000 энтити в одной точке: без паники, время ограничено (одна трасса < 50 мс в debug)
```

- [ ] **Step 2:** FAIL. **Step 3:** Реализовать. **Step 4:** PASS; характеризующие хэши коллизий фазы 2A не меняются.
- [ ] **Step 5: Commit** `feat(engine): traces against linked entities as SV_Move`

### Task 4: Кадр как `SV_Frame`, физика по `movetype`

**Files:**
- Create: `crates/cs-engine/src/physics.rs`, `crates/cs-engine/tests/physics.rs`
- Modify: `server.rs` (`frame`), `think.rs` (удаляется, логика уходит в `physics.rs`), `edict.rs` (правило переиспользования), `api_entities.rs` (`time`), `game.rs`, `tests/think.rs`

**Interfaces:**
- Produces: `EngineApi::time()` — время симуляции `sv_time` (HLDS `sv.time`); `pub const SV_TIME_AT_MAP_START: f64 = 1.0;` `MAX_THINKS_PER_FRAME` удаляется (обход ограничен размером пула, как в HLDS). Сетевые таймеры (таймауты, rate, пинги) остаются на реальном `now`.

Порядок кадра (ReHLDS `sv_main.cpp` `SV_Frame_Internal`, `sv_phys.cpp` `SV_Physics`): `host_frametime = now - prev` (ограничение — как `Host_FilterTime` в `host.cpp`, прочитать); прочитать датаграммы (команды игроков идут с текущим `sv_time`); `SV_Physics`: `start_frame`, затем каждая живая энтити по индексу (игроки `1..=max_clients` пропускаются; `force_retouch` — по исходнику, касания в очередь задачи 5) — по `movetype`: `NONE` (`SV_Physics_None`: только think), `NOCLIP`, `FOLLOW` в этой задаче; остальные типы — в задачах 6–8 (до них — как `NONE`); `FL_KILLME` → освобождение; затем `sv_time += host_frametime`; затем таймауты, рассылка. `SV_RunThink`: think, если `0 < nextthink <= sv_time + host_frametime`; на время вызова `eng.time()` = `max(nextthink, sv_time)`; `nextthink` обнуляется до вызова. `sv_time` = 1.0 при каждой активации карты; правило переиспользования edict'ов становится буквальным `freetime <= 2.0 || sv_time - freetime >= 0.5` (поле `map_start` пула уходит). Тесты, завязанные на старый порядок (think до датаграмм, `eng.time()` = `now`), переписать с объяснением в отчёте; границы поведения не сдвигать.

- [ ] **Step 1: Падающие тесты** (`tests/physics.rs`):

```rust
#[test] fn frame_order_is_packets_then_physics_then_time()  // usercmd этого кадра виден в think этого кадра; eng.time() в run_usercmd == sv_time до продвижения
#[test] fn think_sees_its_own_time()                          // nextthink = sv_time + 0.004 при кадре 0.01 → вызов в этом кадре, eng.time() == nextthink
#[test] fn sv_time_restarts_at_each_map()                     // после change_map eng.time() == 1.0
#[test] fn killme_frees_after_physics()
#[test] fn follow_tracks_aiment()                             // MOVETYPE_FOLLOW: origin = origin aiment + v_angle (SV_Physics_Follow)
#[test] fn noclip_moves_by_velocity_without_collision()
#[test] fn players_are_skipped_by_the_entity_pass()
```

- [ ] **Step 2:** FAIL. **Step 3:** Реализовать. **Step 4:** PASS; все тесты `think` переписаны под новое правило и зелёные.
- [ ] **Step 5: Commit** `feat(engine): frame order and physics dispatch as SV_Frame / SV_Physics`

### Task 5: Касания и удары

**Files:**
- Create: `crates/cs-engine/src/touch.rs`, `crates/cs-engine/tests/touch.rs`
- Modify: `game.rs`, `world.rs`, `physics.rs`, `server.rs` (раздача после каждого вызова игры)

**Interfaces:**
- Produces:

```rust
pub trait GameDll {
    fn touch(&mut self, _eng: &mut EngineApi<'_>, _toucher: EdictId, _other: EdictId) {}    // pfnTouch
    fn blocked(&mut self, _eng: &mut EngineApi<'_>, _pusher: EdictId, _other: EdictId) {}   // pfnBlocked (вызывает задача 8)
}
pub const MAX_TOUCHES_PER_FRAME: usize = 4096;
```

Перенести `SV_TouchLinks` (триггеры, чей бокс пересекает энтити при `link_entity(_, true)`; для BSP-триггеров — точная проверка через хал, как в исходнике) и `SV_Impact` (обе стороны: `touch(e1, e2)`, затем `touch(e2, e1)`, только если оба не `SOLID_NOT`). Очередь: касания, найденные во время вызова игры или физики, раздаются сразу после него по порядку; новые касания в раздаче дописываются в ту же очередь; всего не больше `MAX_TOUCHES_PER_FRAME` за кадр (остальное отбрасывается, одна строка `tracing::warn!` с числом). Перед раздачей пары проверяется, что обе энтити живы (serial). Doc-комментарий: в HLDS касания вызываются синхронно внутри `SV_LinkEdict`; здесь — после возврата из текущего вызова игры (отличие порядка, assumption о его безвредности).

- [ ] **Step 1: Падающие тесты** (`tests/touch.rs`, `TestGame` пишет журнал касаний):

```rust
#[test] fn linking_into_a_trigger_touches_it_once()       // trigger.touch(trigger, ent) — один раз за link
#[test] fn impact_touches_both_sides_in_order()
#[test] fn removal_inside_touch_is_safe()                 // touch удаляет себя и вторую сторону; следующая пара с удалённой — пропущена
#[test] fn touches_inside_touch_are_dispatched_in_the_same_frame()
#[test] fn touch_storm_is_bounded()                       // 2000 энтити в одном триггере, каждая при касании перелинкуется → ≤ MAX_TOUCHES_PER_FRAME, кадр завершается
```

- [ ] **Step 2:** FAIL. **Step 3:** Реализовать. **Step 4:** PASS.
- [ ] **Step 5: Commit** `feat(engine): touch links and impacts with a bounded dispatch queue`

### Task 6: Toss, bounce, fly

**Files:**
- Modify: `physics.rs`; Create: `crates/cs-engine/tests/physics_toss.rs`

Перенести из `sv_phys.cpp`: `SV_CheckVelocity` (`sv_maxvelocity`, NaN → 0 с одной строкой `warn`), `SV_AddGravity` (`sv_gravity` × `gravity` энтити, 0 значит 1), `SV_ClipVelocity`, `SV_FlyMove`, `SV_PushEntity`, `SV_Physics_Toss` (для `TOSS`, как ветка REHLDS_FIXES), `SV_Physics_Bounce` (`BOUNCE`, `BOUNCEMISSILE`, `FLY`, `FLYMISSILE`), `SV_CheckWater`, `SV_CheckWaterTransition`, `SV_Submerged`/`SV_RecursiveWaterLevel`. Удары — через `SV_Impact` задачи 5; `FL_ONGROUND` и `groundentity` — как в исходнике.

- [ ] **Step 1: Падающие тесты:**

```rust
#[test] fn toss_falls_and_lands()                     // на de_dust2 (пропуск без игры): падение с 200 юнитов, остановка на полу, FL_ONGROUND, groundentity = world
#[test] fn bounce_reflects_with_overbounce()          // BOUNCE: после удара о пол vz меняет знак, |v| уменьшается по overbounce исходника
#[test] fn fly_ignores_gravity()
#[test] fn velocity_is_clamped_and_nan_zeroed()       // sv_maxvelocity 2000: 1e9 → 2000; NaN → 0
#[test] fn water_level_and_transition()               // карта с водой из установленных (назвать в отчёте; пропуск без игры)
#[test] fn impact_on_landing_touches_both()
```

- [ ] **Step 2:** FAIL. **Step 3:** Реализовать. **Step 4:** PASS.
- [ ] **Step 5: Commit** `feat(engine): toss, bounce and fly physics as sv_phys`

### Task 7: Step-физика и шаги энтити

**Files:**
- Create: `crates/cs-engine/src/movestep.rs`, `crates/cs-engine/tests/movestep.rs`
- Modify: `physics.rs`, `api_entities.rs`

**Interfaces:**
- Produces: `EngineApi::walk_move(&mut self, id: EdictId, yaw: f32, dist: f32, mode: WalkMoveMode) -> bool` (`PF_walkmove_I`; `pub enum WalkMoveMode { Normal, WorldOnly, CheckOnly }`), `EngineApi::drop_to_floor(&mut self, id: EdictId) -> i32` (`PF_droptofloor_I`: 1 — встал, 0 — нет пола, -1 — застрял), `EngineApi::move_to_origin(&mut self, id: EdictId, goal: Vec3, dist: f32, move_type: i32)` (`SV_MoveToOrigin_I`).

Перенести из `sv_move.cpp`: `SV_CheckBottom`, `SV_movetest`, `SV_movestep`, `SV_StepDirection`, `SV_FlyDirection`, `SV_FixCheckBottom`, `SV_NewChaseDir2`, `SV_MoveToOrigin_I`; из `sv_phys.cpp`: `SV_Physics_Step` (`STEP`, `PUSHSTEP`: гравитация без `FL_ONGROUND`, трение `sv_friction`/`sv_stopspeed`, `PF_WaterMove`).

- [ ] **Step 1: Падающие тесты** (de_dust2/cs_office с пропуском без игры):

```rust
#[test] fn walk_move_steps_up_a_stair()               // ступенька ≤ sv_stepsize (18) — проходит, выше — нет
#[test] fn walk_move_refuses_to_walk_off_a_ledge()    // без FL_FLY/FL_SWIM край пропасти — false, позиция прежняя
#[test] fn drop_to_floor_results()                     // в воздухе над полом → 1 и на полу; над пустотой → 0; в стене → -1
#[test] fn step_entity_falls_and_gets_friction()
#[test] fn move_to_origin_approaches_the_goal()
```

- [ ] **Step 2:** FAIL. **Step 3:** Реализовать. **Step 4:** PASS.
- [ ] **Step 5: Commit** `feat(engine): step physics and entity walking as sv_move`

### Task 8: Толкатели

**Files:**
- Create: `crates/cs-engine/tests/pushers.rs`
- Modify: `physics.rs`

Перенести из `sv_phys.cpp`: `SV_Physics_Pusher` (`ltime`, think по `ltime`, как в исходнике), `SV_PushMove`, `SV_PushRotate` (перемещение «наездников» и задетых энтити, откат при застревании, вызов `GameDll::blocked(pusher, other)`; игроки и `MOVETYPE_PUSHSTEP` по правилам исходника), `SV_CheckMovingGround`. Энтити, заблокировавшая толкатель, остаётся на месте; толкатель откатывается.

- [ ] **Step 1: Падающие тесты** (карта-заглушка с брашем нет — использовать de_dust2 `*N` с пропуском без игры, плюс BBOX-толкатель на заглушке):

```rust
#[test] fn pusher_moves_by_velocity_on_ltime()
#[test] fn rider_moves_with_the_pusher()              // коробка стоит на толкателе (groundentity) → сдвинулась на тот же вектор
#[test] fn crushed_rider_reverts_the_push()           // зажата между толкателем и стеной → blocked(pusher, rider), позиции до хода, без NaN
#[test] fn rotating_pusher_carries_and_rotates()      // avelocity (0,90,0): наездник повернут вокруг оси толкателя
#[test] fn blocked_may_remove_the_pusher()            // blocked удаляет толкатель — без паники, следующий кадр без него
```

- [ ] **Step 2:** FAIL. **Step 3:** Реализовать. **Step 4:** PASS.
- [ ] **Step 5: Commit** `feat(engine): pushers and blocked as SV_PushMove / SV_PushRotate`

### Task 9: Игроки в мире, `WalkGame` на трассах по энтити

**Files:**
- Modify: `crates/cs-engine/src/usercmd.rs`, `api_entities.rs`, `game.rs`; `crates/cs-sim/src/walk_game.rs`, `crates/cs-sim/tests/walk_game.rs`
- Test: `crates/cs-engine/tests/players_in_world.rs`

**Interfaces:**
- Produces: `EngineApi::trace_player_move(&self, player: EdictId, start: Vec3, end: Vec3, hull: Hull) -> EntTrace` — трасса с правилами `SV_AddLinksToPM` / `SV_IsCollidableInPM` (`sv_user.cpp`: какие энтити попадают в physents — без триггеров, `SOLID_NOT`, самого игрока и его владельца/владеемых, по исходнику); `EngineApi::player_touched(&mut self, player: EdictId, other: EdictId)` — ставит `SV_Impact` в очередь задачи 5.

После каждого `run_usercmd` движок связывает edict игрока с касанием триггеров (`SV_FinishPMove` → `SV_LinkEdict(sv_player, TRUE)`). Edict игрока — `SOLID_SLIDEBOX`, `MOVETYPE_WALK` после `client_put_in_server` (если игра не задала иное). `WalkGame`: твёрдые браш-энтити карты (`func_wall`, `func_door`, `func_door_rotating`, `func_breakable`, `func_button`, `func_plat`, `func_train`, `func_pushable`) теперь остаются энтити движка (`spawn_entity` → `true`, `SOLID_BSP`, `MOVETYPE_PUSH`, скорость 0 — логики дверей ещё нет); список твёрдых брашей в `WalkGame` удаляется; адаптер `MoveWorld` для трассы зовёт `trace_player_move`, для содержимого — `point_contents_ents`; лестницы — прежний список границ. Касания, найденные pmove (`touch`-индексы), передаются через `player_touched`.

- [ ] **Step 1: Падающие тесты:**

```rust
// crates/cs-engine/tests/players_in_world.rs
#[test] fn player_edict_is_linked_after_each_command()
#[test] fn walking_into_a_trigger_touches_it()
#[test] fn player_move_trace_ignores_triggers_and_self()
// crates/cs-sim/tests/walk_game.rs
#[test] fn players_block_each_other()                  // два probe лицом друг к другу на de_dust2: расстояние между центрами ≥ 32
#[test] fn solid_brush_entities_block_movement()       // переписать: тот же de_dust2 func_breakable, теперь через энтити движка
#[test] fn legacy_numbers_unchanged()                  // пять старых тестов walk_game с прежними границами
```

- [ ] **Step 2:** FAIL. **Step 3:** Реализовать. **Step 4:** PASS; `cargo test -p cs-sim`; характеризующие тесты фазы 2A не меняются.
- [ ] **Step 5: Проверка бинаря** (debug, порт 27115): сервер de_dust2 + два probe с `--walk` 20 с — оба в игре, без ошибок разбора; `status` показывает обоих; `quit` — код 0.
- [ ] **Step 6: Commit** `feat: players as world entities; WalkGame on entity traces`

### Task 10: Компенсация лага

**Files:**
- Create: `crates/cs-engine/src/unlag.rs`, `crates/cs-engine/tests/unlag.rs`
- Modify: `usercmd.rs`, `frame.rs` (история позиций игроков по кадрам клиента), `server.rs` (cvars)

**Interfaces:**
- Produces: cvars `sv_unlag` (1), `sv_maxunlag` (0.5), `sv_unlagpush` (0), `sv_unlagsamples` (1) — значения по умолчанию из исходника; во время `run_usercmd` игрока с включённой компенсацией (`cl_lc` в userinfo, как `SV_IsLagCompensationActive`) остальные игроки стоят там, где их видел этот клиент; после команды — восстановлены.

Перенести из `sv_user.cpp`: `SV_IsLagCompensationActive`, `SV_GetUnlagTargetTime` (пинг клиента, `lerp_msec` из usercmd, ограничение `sv_maxunlag`), `SV_FindUnlagFrames`, `SV_CalcUnlagFraction`, `SV_UnlagCheckTeleport`, `SV_FlagNonInterpolatedPlayers`, `SV_SetupMove`, `SV_RestoreMove`. Перемещённые игроки перелинкуются (без касаний), после команды возвращаются точно (побитово те же `origin`, `absmin`, `absmax`).

- [ ] **Step 1: Падающие тесты:**

```rust
#[test] fn shooter_sees_target_where_its_client_saw_it()   // цель бежит; во время команды стрелка с пингом 100 мс trace_line попадает туда, где цель была ~100 мс + lerp назад
#[test] fn unlag_off_traces_current_positions()            // sv_unlag 0 или cl_lc 0
#[test] fn unlag_bounds_and_restores_exactly()             // lerp_msec 65535, пинг 0 и 5000 мс, телепорт цели: откат ≤ sv_maxunlag и не глубже истории; после команды позиции побитово прежние
```

- [ ] **Step 2:** FAIL. **Step 3:** Реализовать. **Step 4:** PASS.
- [ ] **Step 5: Commit** `feat(engine): lag compensation as sv_unlag`

### Task 11: PAS и звук

**Files:**
- Modify: `crates/cs-engine/src/map.rs`, `api_messages.rs`; Test: `crates/cs-engine/tests/messages.rs`

**Interfaces:**
- Produces: `MsgDest::Pas(Vec3)`; `ServerMap::can_hear(&self, ear: Vec3, absmin: Vec3, absmax: Vec3) -> bool`.

PAS — объединение PVS видимых листьев, считается при загрузке карты как `SV_CalcPHS` (`sv_main.cpp`; для карт без vis — «слышно всё»); хранение сжато, память ограничена (на de_dust2 посчитать и записать в отчёт). `emit_sound` шлёт через `Pas` (ReHLDS `SV_StartSound_internal`: `MSG_FL_PAS`); заметка о PVS в doc-комментарии и реестре снимается.

- [ ] **Step 1: Падающие тесты:**

```rust
#[test] fn pas_is_a_superset_of_pvs()           // на de_dust2 (пропуск без игры) для 200 пар листьев: виден ⇒ слышен
#[test] fn sound_reaches_a_listener_behind_a_wall() // точка не в PVS слушателя, но в PAS → svc_sound получен
#[test] fn pas_memory_is_bounded()               // размер таблицы ≤ leafs × ceil(leafs/8) байт
```

- [ ] **Step 2:** FAIL. **Step 3:** Реализовать. **Step 4:** PASS.
- [ ] **Step 5: Commit** `feat(engine): PAS and sounds through it`

### Task 12: Документы и итоговая проверка

**Files:**
- Create: `docs/parity/checklists/phase2b-world.md`
- Modify: `docs/parity/engine.toml`, `README.md`, `THIRD-PARTY-NOTICES.md` (перенесённые функции ReHLDS: файлы и что из них взято)

- [ ] **Step 1: Реестр:** `entity_links`, `entity_traces` (было `missing`), `physics_movetypes`, `touch`, `pushers`, `player_world`, `lag_compensation`, `pas` — `tested` с именами тестов (проверены grep'ом); в доказательствах — принятые отличия (касания после вызова игры, а не внутри `SV_LinkEdict`). Ничего `verified`.
- [ ] **Step 2: Чек-лист владельца** `phase2b-world.md` (пустые «Дата» и «Результат»): оригинальный клиент на нашем сервере — игроки не проходят друг сквозь друга; браш-стена (`func_wall`/`func_breakable` на de_dust2) не проходима; движение возле стен без дрожания; два клиента с пингом 100+ мс (`net_fakelag` если доступен) — нет рывков. Каждая строка — точные команды сервера и клиента.
- [ ] **Step 3: README:** что умеет режим `--proto48` после 2B (игроки сталкиваются, браши твёрдые; двери ещё не открываются).
- [ ] **Step 4: Итоговая проверка** по одной команде: clippy; `cargo test --workspace`; wasm-строка CI; `cargo xtask parity` (цифры до и после).
- [ ] **Step 5: Commit** `docs: phase 2B registry, checklist and notices`

---

## Что вне этого плана

Логика дверей, кнопок, лифтов, поездов и триггеров карт (`func_door` открывается, `trigger_hurt` ранит) — фаза 7 (энтити карт и режимы) и 2E (энтити в правилах CS). Игроки, правила раунда, HUD — 2C. Оружие — 2D. Боты — фаза 12. Хуки модов на каждую запись поля `EntVars` (сейчас хук оборачивает только `ent_mut`) — фаза 5. Клиент с низким `rate` против широковещательного надёжного потока (остаток фазы 1) и хитбоксы по костям модели — фаза 2D вместе с оружием.
