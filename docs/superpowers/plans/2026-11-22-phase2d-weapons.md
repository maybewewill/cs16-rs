# Фаза 2D — оружие: план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Оружие CS 1.6 в `cs-game` как точный перенос ReGameDLL_CS (ванильная ветка): все 25 стволов, нож и три гранаты; покупка, выброс, подбор, `weaponbox`, `armoury_entity`. В том же переносе:
- пули с пробитием по материалам и разбросом по общему seed;
- урон по хитгруппам по настоящим хитбоксам модели;
- броня, замедление при попадании;
- данные для клиентского предсказания: `weapon_data`, `svc_weaponanim`, события `events/*.sc`.

Оригинальный клиент с `cl_lw 1` предсказывает выстрелы, перезарядку и анимации тем же кодом, поэтому любое расхождение видно как дрожание оружия.

**Architecture:**
- **Движок** даёт системы, которых нет:
  - события: `precache_event`, `playback_event`, `svc_event`;
  - `weapon_data_t` в `svc_clientdata` и `svc_weaponanim`;
  - настоящий `random_seed` команды;
  - трассы по хитбоксам студийной модели с `hitgroup`;
  - сборщики temp entity и индексы декалей;
  - недостающие поля `EntVars`.
- **`cs-game`** получает модуль `weapons/`: таблицы, общий генератор случайных чисел, конечный автомат `CBasePlayerWeapon`, отдельные стволы. А также:
  - `bullets.rs` (`FireBullets3`/`FireBullets`, `TraceAttack`, `MultiDamage`);
  - `inventory.rs` (предметы игрока, покупка, выброс, `weaponbox`, `armoury_entity`);
  - `grenades.rs` (`CGrenade`).
- **Старое оружие** (`cs_sim::weapons`, `grenades`, `MatchWorld`, `cs-client`) не трогаем.

**Tech Stack:** Rust 1.100.0, `cs-engine`, `cs-proto48`, `cs-assets` (`mdl.rs`), `cs-game`, `cs-sim::pm` (таблица текстур для пробития), `cs-probe`; ReGameDLL_CS `dlls/weapons.*`, `wpn_shared/*`, `player.cpp`, `client.cpp`, `combat.cpp`, `ggrenade.cpp`, `util.cpp`, `weapontype.*`; ReHLDS `sv_main.cpp`, `pr_cmds.cpp`, `sv_user.cpp`, `r_studio.cpp`, `world.cpp`.

**Spec:** `docs/superpowers/specs/2026-11-20-cs16-rs-master-plan-design.md` §6 «Фаза 2», пп. 1, 2, 6 (оружие), категория реестра «Оружие, предметы, экономика» (вес 10) и «Temp entities» (4). Журналы 2C и pm (`.superpowers/sdd/2026-11-22-phase2c-cs-game/progress.md`, `.superpowers/sdd/2026-11-22-pm-shared/progress.md`) — CARRY для 2D.

**Описи:** `.superpowers/sdd/2026-11-22-phase2d-weapons/survey-source.md` (классы, автомат `ItemPostFrame`, таблица всех стволов, пули и урон, хитбоксы, контракт предсказания, экономика, usermsg, риски) и `survey-code.md` (что есть у нас и чего нет). Исходники скачаны в `scratchpad\wpnsrc` (см. `constraints.md`).

## Global Constraints

- **Общие правила.** Действуют все пункты `constraints.md`:
  - `unsafe_code = "forbid"`;
  - после каждой задачи зелёные clippy `-D warnings`, `cargo test --workspace` и wasm-check;
  - cargo только с throttle;
  - без `cargo fmt` и без release;
  - `EngineApi` не паникует.
- **Ваниль и порядок операций.** Цель — ванильная ветка (конфигурация PLAY_GAMEDLL = `REGAMEDLL_API;CLIENT_WEAPONS`, всегда ветка `#else` у `REGAMEDLL_FIXES`/`ADD`/`BUILD_LATEST`). Переносим оператор за оператором, с причудами. Числа с плавающей точкой — по правилу `constraints.md`.
- **Целочисленное деление.** Делители точности в ванили — `int` (`(shots*shots*shots)/200`, целочисленное деление), у MP5N — `double`.
- **Таймеры оружия.** Это обратные отсчёты от 0 (`UTIL_WeaponTimeBase() == 0`), уменьшаются в PostThink после `ItemPostFrame`. Часть таймеров — абсолютное время.
- **Случайность.** Общий генератор (`UTIL_SharedRandom*`) — точная копия, включая таблицу seed и усечение диапазона. Серверный `RANDOM_LONG`/`RANDOM_FLOAT` — RNG игры.
- **Без ассетов Valve.** Ассетов Valve нет в репозитории. Модели, звуки и `delta.lst` берутся из установленной игры через VFS.
- **Ввод клиента не роняет игру.** Ни один путь не паникует на вводе клиента: `buy` с мусором, `drop` без оружия, `weapon_*` с несуществующим именем, `slot99`, `impulse 255`, команды мёртвого, наблюдателя и до входа.
- **Старый путь.** Старое оружие и его тесты остаются как есть.

## Review Focus

1. **Предсказание:**
   - разброс по seed;
   - тайминги следующего выстрела и перезарядки;
   - поля `weapon_data` (повторно используемые поля);
   - `svc_weaponanim` и события, которые не уходят стрелку при `cl_lw`.

   Тесты: задача 2 `shared_random_matches_util_cpp`; задача 6 `item_post_frame_timeline`; задача 9 `weapon_data_round_trip_per_weapon`.
2. **Таблицы.** Цены, магазины, урон, разброс по состояниям, отдача — построчно с источником. Тесты: задачи 5, 10, 11 — табличные проверки каждого ствола.
3. **Хитбоксы.** Поза из `sequence`/`frame`/`blending`/`controller`/`gaitsequence`, раздувание на 0.5. Тест: задача 3 `bullet_hits_head_hitbox`.
4. **Экономика и инвентарь.**
   - покупка при нехватке денег;
   - покупка за пределами зоны;
   - замена основного;
   - выброс и подбор;
   - смерть с оружием;
   - рестарт раунда.

   Тест: задача 12 `buy_drop_pickup_as_regamedll`.
5. **Враждебные команды.** Тест: задача 12 `hostile_weapon_commands_are_ignored`.

## Порядок и параллельность

- **Волна 1** (5 исполнителей параллельно): задачи 1–4 (движок) в worktree, задача 5 (таблицы и общий RNG) в основной копии.
- **Волна 2:** задачи 6 (автомат оружия) и 7 (пули и урон) — параллельно.
- **Волна 3:** задачи 8 (пистолеты), 10 (автоматы, винтовки, пулемёт), 11 (снайперки, дробовики, нож), 12 (инвентарь и экономика) — параллельно, каждая со своими файлами.
- **Волна 4:** задачи 9 (данные предсказания и анимации игрока) и 13 (гранаты).
- **Волна 5:** задача 14 (документы), финальное ревью.

Задача 6 создаёт заглушки всех стволов (как задача 4 pm); задачи 8, 10, 11 заполняют только свои файлы.

---

### Task 1: Движок — события (`precache_event`, `playback_event`, `svc_event`)

**Files:**
- Create: `crates/cs-engine/src/events.rs`, `crates/cs-engine/tests/events.rs`.
- Modify: `crates/cs-engine/src/precache.rs` (ресурсы `events/*.sc`), `frame.rs` (запись событий в датаграмму кадра), `api_messages.rs` (методы), `lib.rs`, `crates/cs-proto48/src/messages.rs` (писатель `svc_event`/`svc_event_reliable` по схеме `event_t`).

**Interfaces — Produces:**

```rust
pub mod event_flags { pub const FEV_NOTHOST: i32 = 1; pub const FEV_RELIABLE: i32 = 2; pub const FEV_GLOBAL: i32 = 4;
    pub const FEV_UPDATE: i32 = 8; pub const FEV_HOSTONLY: i32 = 16; pub const FEV_SERVER: i32 = 32; pub const FEV_CLIENT: i32 = 64; }
impl EngineApi<'_> {
    /// PF_precache_event: index in the event resource list (1-based), 0 when refused.
    pub fn precache_event(&mut self, name: &str) -> u16;
    /// EV_Playback / PF_playbackevent (ReHLDS pr_cmds.cpp + sv_main.cpp).
    pub fn playback_event(&mut self, flags: i32, invoker: Option<EdictId>, event_index: u16, delay: f32,
        origin: Vec3, angles: Vec3, fparam1: f32, fparam2: f32, iparam1: i32, iparam2: i32, bparam1: bool, bparam2: bool);
}
```

Перенести из ReHLDS:
- `PF_precache_event`, список ресурсов событий в signon (`svc_resourcelist`, тип event);
- `EV_Playback`:
  - origin и angles из invoker, если не заданы;
  - флаг ducking;
  - фильтр PAS/PVS по источнику;
  - `FEV_NOTHOST` не шлёт invoker'у, только если у его клиента `cl_lw` 1 (так в ReHLDS — проверить);
  - `FEV_HOSTONLY`, `FEV_RELIABLE` → `svc_event_reliable`, `FEV_GLOBAL`, `FEV_UPDATE`;
  - очередь событий клиента и `delay`;
- `SV_EmitEvents`: запись в датаграмму кадра, дельта `event_t` по схеме `CS16_DEFAULT_DELTA_LST`, лимит событий на кадр.

Доступ к `cl_lw`/`cl_lc` клиента — метод `EngineApi::client_lw(slot) -> bool` (здесь или в задаче 2; кто первый — второй переиспользует; конфликт решает контроллер при слиянии).

- [ ] **Step 1: Тесты:**
  - `precached_events_are_in_the_resource_list`;
  - `playback_reaches_pas_clients_not_the_shooter_with_lw` — два probe: у стрелка `cl_lw 1`;
  - `reliable_events_use_svc_event_reliable`;
  - `event_args_round_trip_on_the_wire`;
  - `delayed_and_global_events`;
  - `bad_event_index_is_refused_quietly`.
- [ ] **Step 2–4:** FAIL → реализовать → PASS.
- [ ] **Step 5: Commit** `feat(engine): event precache and playback as ReHLDS`

### Task 2: Движок — `weapon_data_t`, `svc_weaponanim`, `random_seed`

**Files:**
- Create: `crates/cs-engine/tests/weapon_data.rs`.
- Modify: `crates/cs-engine/src/game.rs` (`GameDll::get_weapon_data`), `frame.rs` (запись списка оружия в `svc_clientdata`), `client.rs` (последний подтверждённый список на кадр), `usercmd.rs` (`random_seed` как ReHLDS), `api_messages.rs` (`weapon_anim`, `client_lw`), `crates/cs-proto48/src/delta.rs` (`GoldSrcWeaponData`, поля `ammo_*` у `GoldSrcClientData`, если их нет).

**Interfaces — Produces:**

```rust
/// weapon_data_t (weaponinfo.h), field names snake_case of the C names.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GoldSrcWeaponData { pub m_i_id: i32, pub m_i_clip: i32, pub m_fl_next_primary_attack: f32,
    pub m_fl_next_secondary_attack: f32, pub m_fl_time_weapon_idle: f32, pub m_f_in_reload: i32,
    pub m_f_in_special_reload: i32, pub m_fl_next_reload: f32, pub m_fl_pump_time: f32, pub m_f_reload_time: f32,
    pub m_f_aimed_damage: f32, pub m_f_next_aim_bonus: f32, pub m_f_in_zoom: i32, pub m_i_weapon_state: i32,
    pub iuser1: i32, pub iuser2: i32, pub iuser3: i32, pub iuser4: i32,
    pub fuser1: f32, pub fuser2: f32, pub fuser3: f32, pub fuser4: f32 }
// GameDll:
fn get_weapon_data(&mut self, _eng: &mut EngineApi<'_>, _slot: u8, _out: &mut [GoldSrcWeaponData; 64]) -> bool { false }
impl EngineApi<'_> {
    /// svc_weaponanim to the client of `slot` (SendWeaponAnim with skiplocal handled by the game).
    pub fn weapon_anim(&mut self, slot: u8, sequence: i32, body: i32);
    /// The client's `cl_lw` userinfo (local weapons) — ReHLDS host_client->lw.
    pub fn client_lw(&self, slot: u8) -> bool;
}
```

Перенести из ReHLDS:
- `SV_WriteClientdataToMessage`, часть weapon data: какие индексы шлются, дельта против подтверждённого кадра, условие `lw`, 6-битный индекс — проверить по источнику;
- `svc_weaponanim`;
- `random_seed`: откуда ReHLDS берёт seed для `pfnCmdStart` (`SV_ParseMove`/`SV_RunCmd`), один seed на половины команды больше 50 мс. Заменить «рабочее допущение» в `usercmd.rs` на источник.

Схема `weapon_data_t` уже есть в `cs-proto48`. Сверить её поля и разрядность с `delta.lst` установленной игры (`cstrike\delta.lst` или `valve\delta.lst` через VFS), расхождения записать.

- [ ] **Step 1: Тесты:**
  - `weapon_data_is_sent_only_to_lw_clients`;
  - `weapon_data_deltas_against_the_acked_frame`;
  - `weapon_data_round_trip_on_the_wire`;
  - `weapon_anim_reaches_only_its_client`;
  - `random_seed_follows_rehlds_and_halves_share_it`.
- [ ] **Step 2–5:** … → Commit `feat(engine): weapon_data in clientdata, weapon anims, command random seed`

### Task 3: Движок — трассы по хитбоксам студийной модели

**Files:**
- Create: `crates/cs-engine/src/studio.rs`, `crates/cs-engine/tests/studio_hitboxes.rs`.
- Modify: `crates/cs-engine/src/precache.rs` (загрузка `.mdl` через VFS при прекэше модели), `trace_ents.rs` / `collision.rs` (`SV_HullForStudioModel` в клипе против энтити), `edict.rs` (`controller: [u8; 4]`, `blending: [u8; 2]`, если нет), `api_entities.rs` (hitgroup в результате), `lib.rs`, `crates/cs-assets/src/mdl.rs` (только если парсеру не хватает данных).

**Interfaces — Produces:** у результата трассы (`EntTrace` и/или `TraceResult`) поле `pub hitgroup: i32` (0 — generic), заполняемое как `tr.iHitgroup` в ReHLDS. Публичные сигнатуры `trace_line`/`trace_box` не меняются; меняется поведение по источнику: когда ReHLDS трассирует хитбоксы (`SV_HullForStudioModel`: тип модели, `FL_CLIENT`/флаги, точечная трасса, `sv_clienttrace`, щит).

Перенести из ReHLDS `r_studio.cpp`, `world.cpp`:
- `SV_StudioSetupBones`:
  - поза из `sequence`, `frame`, `blending`, `controller`;
  - `gaitsequence` и смешивание ног у игроков, как источник;
  - кэш на кадр, если он есть в источнике;
- `R_StudioHull` / `SV_HullForStudioModel`: хулл на каждый хитбокс по матрице кости, раздувание (0.5 для пуль — проверить);
- `SV_SingleClipMoveToEntity` с несколькими хуллами и `hitgroup`;
- пропуск хитбокса щита (по источнику).

Производительность: тест считает работу счётчиком, не временем.

- [ ] **Step 1: Тесты** (модели игроков из установленной игры, без игры пропуск):
  - `bullet_hits_head_hitbox` — `models/player/gign/gign.mdl` в позе idle: трасса в голову даёт hitgroup HEAD, в грудь CHEST, мимо — промах;
  - `pose_follows_sequence_and_frame`;
  - `hull_traces_ignore_hitboxes` — движение игрока по-прежнему по box hull;
  - `bad_model_data_is_refused_quietly`.
- [ ] **Step 2–5:** … → Commit `feat(engine): studio hitbox traces with hitgroups as ReHLDS`

### Task 4: Движок — temp entity, декали, звук, поля `EntVars`

**Files:**
- Create: `crates/cs-proto48/src/tempent.rs` (типизированные сборщики `TE_*` из списка ниже), `crates/cs-engine/tests/tempents.rs`.
- Modify: `crates/cs-engine/src/api_messages.rs` (`temp_entity` с назначением PVS/PAS/One/All; звук с флагами `SND_*` и звук из точки `EmitAmbientSound`), `edict.rs` (`viewmodel`, `weaponanim`, `impulse`, если нет), `precache.rs` (индексы декалей `decals.wad`, лимиты моделей и звуков на полный набор оружия), `lib.rs`.

**Interfaces — Produces:**

```rust
// cs-proto48::tempent — one builder per TE used by weapons/grenades:
// TE_GUNSHOTDECAL, TE_DECAL, TE_DECALHIGH, TE_GUNSHOT, TE_SPARKS, TE_EXPLOSION, TE_SMOKE, TE_BEAMFOLLOW,
// TE_DLIGHT, TE_STREAK_SPLASH, TE_BLOODSPRITE / TE_BLOODSTREAM (what ReGameDLL blood uses), TE_WORLDDECAL.
impl EngineApi<'_> {
    pub fn decal_index(&self, name: &str) -> i32;          // pfnDecalIndex: index in decals.wad, -1 unknown
    pub fn emit_sound_flags(&mut self, ent: EdictId, channel: i32, sample: &str, volume: f32,
        attenuation: f32, flags: i32, pitch: i32) -> bool;  // SND_STOP/SND_CHANGE_VOL/SND_CHANGE_PITCH
    pub fn emit_ambient_sound(&mut self, ent: EdictId, origin: Vec3, sample: &str, volume: f32,
        attenuation: f32, flags: i32, pitch: i32) -> bool;
}
```

Перенести:
- `pfnDecalIndex` (порядок декалей `decals.wad` как в движке — проверить источник);
- `PF_ambientsound_I`;
- байты `TE_*` по `cs-proto48` и HL SDK `const.h`;
- лимиты `MAX_MODELS`/`MAX_SOUNDS` как HLDS (512) — тест, что полный прекэш оружия CS помещается.

- [ ] **Step 1: Тесты:**
  - `te_builders_match_the_wire_layout` — для каждого `TE_*`;
  - `decal_index_follows_decals_wad`;
  - `ambient_sound_from_a_point`;
  - `full_weapon_precache_fits`.
- [ ] **Step 2–5:** … → Commit `feat(engine): temp entity builders, decal index, sound flags`

### Task 5: `cs-game` — таблицы оружия и общий генератор случайных чисел

**Files:**
- Create: `crates/cs-game/src/weapons/mod.rs` (только `pub mod` таблиц и случайности в этой задаче), `weapons/info.rs`, `weapons/random.rs`, `crates/cs-game/tests/weapon_tables.rs`.
- Modify: `crates/cs-game/src/lib.rs` (`mod weapons;`).

**Interfaces — Produces:**

```rust
#[repr(i32)] pub enum WeaponId { None = 0, P228 = 1, Glock = 2, Scout = 3, HeGrenade = 4, Xm1014 = 5, C4 = 6, Mac10 = 7,
    Aug = 8, SmokeGrenade = 9, Elite = 10, FiveSeven = 11, Ump45 = 12, Sg550 = 13, Galil = 14, Famas = 15, Usp = 16,
    Glock18 = 17, Awp = 18, Mp5n = 19, M249 = 20, M3 = 21, M4a1 = 22, Tmp = 23, G3sg1 = 24, Flashbang = 25,
    Deagle = 26, Sg552 = 27, Ak47 = 28, Knife = 29, P90 = 30, ShieldGun = 99 }
pub struct ItemInfo { pub slot: i32, pub position: i32, pub ammo1: Option<&'static str>, pub max_ammo1: i32,
    pub ammo2: Option<&'static str>, pub max_ammo2: i32, pub name: &'static str, pub max_clip: i32,
    pub id: WeaponId, pub flags: i32, pub weight: i32 }
pub struct WeaponInfo { pub id: WeaponId, pub cost: i32, pub clip_cost: i32, pub buy_clip_size: i32,
    pub gun_clip_size: i32, pub max_rounds: i32, pub ammo_type: AmmoType, pub entity_name: &'static str,
    pub ammo_name1: &'static str, pub ammo_name2: Option<&'static str> }
pub fn item_info(id: WeaponId) -> Option<&'static ItemInfo>;
pub fn weapon_info(id: WeaponId) -> Option<&'static WeaponInfo>;
pub fn alias_to_weapon_id(alias: &str) -> Option<WeaponId>;     // g_weaponAliasInfo
pub fn buy_alias_to_weapon_id(alias: &str) -> Option<WeaponId>; // g_weaponBuyAliasInfo
pub fn max_ammo_carry(ammo: &str) -> i32;
// random.rs: UTIL_SharedRandomLong / UTIL_SharedRandomFloat with seed_table[256] (util.cpp:6-37), U_Random/U_Srand.
pub fn shared_random_long(seed: u32, low: i32, high: i32) -> i32;
pub fn shared_random_float(seed: u32, low: f32, high: f32) -> f32;
```

Плюс:
- `AmmoType`, `BulletType`, `WeaponState`, флаги `ITEM_FLAG_*`, слоты;
- регистрация индексов патронов в порядке `WeaponsPrecache` (`AddAmmoNameToAmmoRegistry`), а не по enum;
- таблицы `g_autoBuyInfo`/`g_weaponSlotInfo` — по описи.

- [ ] **Step 1: Тесты:**
  - `prices_clips_and_ammo_match_weapontype_cpp` — каждая строка;
  - `item_info_matches_get_item_info`;
  - `ammo_indices_follow_precache_order`;
  - `aliases_round_trip`;
  - `shared_random_matches_util_cpp`. Эталонные значения считаются ручным переносом C-кода в тесте с комментарием формулы, включая усечение диапазона до `unsigned` и смешивание битов float в seed.
- [ ] **Step 2–5:** … → Commit `feat(game): weapon tables and the shared random generator`

### Task 6: `cs-game` — автомат оружия (`CBasePlayerWeapon`) и предметы игрока

**Files:**
- Create: `crates/cs-game/src/weapons/weapon.rs` (структура полей `CBasePlayerWeapon`, трейт поведения, реестр стволов), `weapons/core.rs` (`ItemPostFrame`, `DefaultDeploy`, `DefaultReload`, `DefaultShotgunReload`, `PlayEmptySound`/`ResetEmptySound`, `KickBack`, `FireRemaining`, `CanAttack`, `HasSecondaryAttack`, `GetNextAttackDelay`, `SendWeaponAnim`, `UpdateClientData` — `CurWeapon`, `ReloadSound`);
- заглушки стволов: `weapons/{pistols,rifles,smgs,snipers,shotguns,knife,grenades}.rs`;
- `crates/cs-game/tests/weapon_core.rs`.
- Modify: `crates/cs-game/src/player.rs`:
  - поля игрока: `items: [Vec<WeaponSlotItem>; 6]` или эквивалент `m_rgpPlayerItems`, `active_item`, `last_item`, `ammo: [i32; 32]`, `next_attack`, `can_shoot`, `fov`/`client_fov`;
  - `ItemPreFrame`/`ItemPostFrame` в PreThink/PostThink;
  - уменьшение таймеров в конце PostThink (`pt_end`), clamps −1 и −0.001;
  - `SetFOV` при смене FOV.
- Modify: `weapons/mod.rs`.

**Interfaces — Produces:**

```rust
pub struct Weapon { pub id: WeaponId, pub clip: i32, pub next_primary_attack: f32, pub next_secondary_attack: f32,
    pub time_weapon_idle: f32, pub in_reload: bool, pub in_special_reload: i32, pub default_ammo: i32,
    pub fire_on_empty: bool, pub play_empty_sound: bool, pub accuracy: f32, pub last_fire: f32,
    pub shots_fired: i32, pub delay_fire: bool, pub direction: i32, pub decrease_shots_fired: f32,
    pub weapon_state: i32, pub next_reload: f32, pub glock18_shoot: f32, pub glock18_shots_fired: i32,
    pub famas_shoot: f32, pub famas_shots_fired: i32, pub burst_spread: f32, pub pump_time: f32,
    pub start_throw: f32, pub release_throw: f32, pub swing: i32, pub resume_zoom: bool, pub last_zoom: i32,
    pub events: WeaponEvents /* precached ids */, pub ent: Option<EdictId> /* world entity while dropped */ , /* … all fields of survey §2 */ }
pub trait WeaponBehavior: Send + 'static {     // the CBasePlayerItem/Weapon virtuals
    fn id(&self) -> WeaponId;
    fn precache(&mut self, eng: &mut EngineApi<'_>) {}
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool;
    fn primary_attack(&mut self, _w: &mut WeaponCtx<'_, '_>) {}
    fn secondary_attack(&mut self, _w: &mut WeaponCtx<'_, '_>) {}
    fn reload(&mut self, _w: &mut WeaponCtx<'_, '_>) {}
    fn weapon_idle(&mut self, _w: &mut WeaponCtx<'_, '_>) {}
    fn holster(&mut self, _w: &mut WeaponCtx<'_, '_>) {}
    fn max_speed(&self, _w: &Weapon) -> f32 { 250.0 }
    fn use_decrement(&self) -> bool { true }
    fn is_pistol(&self) -> bool { false }
    fn has_secondary_attack(&self, _w: &Weapon) -> bool;
    fn item_post_frame_extra(&mut self, _w: &mut WeaponCtx<'_, '_>) {}   // per-weapon ItemPostFrame overrides (grenades, c4, knife…)
}
pub fn new_weapon(id: WeaponId) -> Option<Box<dyn WeaponBehavior>>;   // registry: every id → its stub/impl
pub struct WeaponCtx<'a, 'e> { /* game state, engine, player slot, the Weapon */ }
```

Точные поля и сигнатуры уточняет исполнитель по `weapons.h` (имена и смысл совпадают с C). Расширять разрешено, менять — нет. Задачи 8, 10, 11 заполняют только свои файлы стволов.

`m_bCanShoot` выставляется в `OnRoundFreezeEnd` (CARRY). Биты `iuser3`:
- `CAN_SHOOT` — `m_bCanShoot && !defusing`;
- `HOLDING_SHIELD` — когда появится щит;

пишутся в `client_data`. Кнопки чистятся во время game over (CARRY).

- [ ] **Step 1: Тесты** (тестовый ствол в тесте, команды по `msec`):
  - `item_post_frame_timeline` — выстрел, удержание, перезарядка, idle с точными таймерами по источнику: строгое `<` у перезарядки, `<=` у атаки, clamps;
  - `deploy_sets_next_attack_0_75`;
  - `default_reload_moves_ammo_into_the_clip`;
  - `shotgun_reload_stages`;
  - `kick_back_as_vanilla`;
  - `empty_sound_once_until_release`;
  - `timers_count_down_in_post_think`;
  - `fov_change_sends_set_fov`;
  - `can_shoot_after_freeze`.
- [ ] **Step 2–5:** … → Commit `feat(game): weapon state machine and player items`

### Task 7: `cs-game` — пули, урон по хитгруппам, броня, `DeathMsg` с оружием

**Files:**
- Create: `crates/cs-game/src/bullets.rs` (`FireBullets3`, `FireBullets` с картечью, `ClearMultiDamage`/`AddMultiDamage`/`ApplyMultiDamage`, `DecalGunshot`, звуки рикошета), `crates/cs-game/tests/bullets.rs`.
- Modify: `crates/cs-game/src/player.rs`:
  - `TraceAttack` с множителями хитгрупп и `m_LastHitGroup`;
  - `TakeDamage`: коэффициент брони по оружию, `DMG_BLAST`, отбрасывание и вздрагивание (CARRY «momentum»), `velocity_modifier` при попадании, звуки боли;
  - `Killed` с именем оружия через `g_pevLastInflictor` (CARRY).
- Modify: `score.rs` (`DeathMsg` с оружием и хедшотом).

Перенести (опись §4):
- пробитие по типу текстуры: таблица `pm` и `eng.trace_texture`;
- модификаторы материалов;
- затухание урона по `range_modifier`;
- разброс по `shared_random_float(seed + n, …)` в порядке источника;
- `TraceAttack` hitgroup → урон;
- кровь и декали по источнику (`TE_*` из задачи 4, `decal_index`).

`hitgroup` — из трассы задачи 3. До её слияния тесты задают hitgroup тестовой трассой.

- [ ] **Step 1: Тесты:**
  - `bullet_damage_falls_off_with_range`;
  - `penetration_by_material`;
  - `headshot_multiplier_and_armor_ratio`;
  - `armor_absorbs_as_regamedll`;
  - `velocity_modifier_on_hit`;
  - `death_msg_names_the_weapon_and_headshot`;
  - `spread_uses_the_shared_seed`;
  - `buckshot_pellets`.
- [ ] **Step 2–5:** … → Commit `feat(game): bullets, hitgroup damage and armor as ReGameDLL`

### Task 8: `cs-game` — пистолеты

**Files:** Modify `crates/cs-game/src/weapons/pistols.rs`; Create `crates/cs-game/tests/weapons_pistols.rs`.

Перенести `wpn_glock18.cpp`, `wpn_usp.cpp`, `wpn_p228.cpp`, `wpn_deagle.cpp`, `wpn_fiveseven.cpp`, `wpn_elite.cpp`:
- Spawn, Precache (модели, звуки, события), Deploy;
- Primary/SecondaryAttack: точность, разброс по состояниям, `cycle`, урон, пробитие;
- аргументы событий с причудами;
- глушитель USP, очередь Glock (`FireRemaining`), левая/правая рука Elite;
- Reload, Idle, `GetMaxSpeed`;
- щит — до задачи 12, только если источник требует.

- [ ] **Step 1: Тесты** (табличные, по каждому стволу):
  - `pistol_rows_match_source` — урон, цикл, магазин, скорость, точность первого и следующих выстрелов;
  - `usp_silencer_toggle_timing`;
  - `glock_burst_three_shots`;
  - `elite_alternates_hands`;
  - `semi_auto_needs_release`;
  - `pistol_events_carry_source_args`.
- [ ] **Step 2–5:** … → Commit `feat(game): pistols as ReGameDLL`

### Task 9: `cs-game` — данные предсказания и анимации игрока

**Files:**
- Create: `crates/cs-game/src/player_anim.rs` (`CBasePlayer::SetAnimation`: последовательности по `m_szAnimExtention`, `gaitsequence`, `frame`/`framerate`, `blending` по питчу — для хитбоксов и вида от третьего лица), `crates/cs-game/tests/weapon_prediction.rs`.
- Modify:
  - `crates/cs-game/src/lib.rs` (`GameDll::get_weapon_data`);
  - `player.rs` (`client_data`: `m_iId`, `m_flNextAttack`, `viewmodel`, `weaponanim`, `ammo_*` по источнику, `fov`);
  - `weapons/core.rs` (`SendWeaponAnim` с `skiplocal` через `eng.weapon_anim`).

Перенести `GetWeaponData` (`client.cpp`):
- поля по описи §5 — с повторным использованием: `m_fInZoom = m_iShotsFired`, `m_fAimedDamage = m_flLastFire`, `fuser2/3` = времена броска, `iuser1 = m_iSwing`;
- полы −0.001;
- условие `sendweapons`.

`UpdateClientData` — поля оружия.

- [ ] **Step 1: Тесты:**
  - `weapon_data_round_trip_per_weapon` — каждое поле каждого ствола после серии команд равно состоянию сервера;
  - `skiplocal_weapon_anims_skip_lw_clients`;
  - `client_data_weapon_fields`;
  - `set_animation_picks_the_source_sequence`;
  - `hitbox_pose_follows_the_player_animation` — вместе с задачей 3.
- [ ] **Step 2–5:** … → Commit `feat(game): weapon prediction data and player animation`

### Task 10: `cs-game` — пистолеты-пулемёты, винтовки, пулемёт

**Files:**
- Modify: `crates/cs-game/src/weapons/smgs.rs` (MAC10, TMP, MP5N, UMP45, P90), `weapons/rifles.rs` (Galil, Famas с очередью, AK47, M4A1 с глушителем, AUG и SG552 с прицелом, M249).
- Create: `crates/cs-game/tests/weapons_auto.rs`.

Перенести соответствующие `wpn_*.cpp` целиком: точность с целочисленным делителем, MP5N — `double`; отдача `KickBack` по состояниям; прицел 55↔90 у AUG и SG552.

- [ ] **Step 1: Тесты:**
  - `auto_rows_match_source`;
  - `accuracy_uses_integer_divisors`;
  - `kickback_by_state`;
  - `famas_burst`;
  - `m4a1_silencer`;
  - `aug_sg552_zoom`;
  - `auto_events_carry_source_args`.
- [ ] **Step 2–5:** … → Commit `feat(game): SMGs, rifles and the machine gun as ReGameDLL`

### Task 11: `cs-game` — снайперские винтовки, дробовики, нож

**Files:**
- Modify: `crates/cs-game/src/weapons/snipers.rs` (Scout, AWP, G3SG1, SG550), `weapons/shotguns.rs` (M3, XM1014), `weapons/knife.rs`.
- Create: `crates/cs-game/tests/weapons_special.rs`.

Перенести:
- прицел 90→40→10/15→90;
- сброс прицела при выстреле и возврат (`m_bResumeZoom`);
- перезарядка в прицеле по источнику;
- спецперезарядка дробовиков;
- картечь;
- константы конуса M3/XM1014 — сверить по источнику, в описи по памяти;
- нож: удар и укол, `FindHullIntersection`, звуки по попаданию.

- [ ] **Step 1: Тесты:**
  - `sniper_rows_match_source`;
  - `zoom_cycle_and_resume`;
  - `shotgun_special_reload_and_pump`;
  - `buckshot_spread`;
  - `knife_slash_and_stab_damage_and_timing`;
  - `knife_hull_intersection`.
- [ ] **Step 2–5:** … → Commit `feat(game): snipers, shotguns and knife as ReGameDLL`

### Task 12: `cs-game` — инвентарь, покупка, выброс, `weaponbox`, `armoury_entity`

**Files:**
- Create: `crates/cs-game/src/inventory.rs`, `crates/cs-game/tests/inventory.rs`.
- Modify:
  - `crates/cs-game/src/buy.rs` (покупка оружия и патронов вместо отказа; тест `weapon_purchase_refused_until_weapons` заменяется);
  - `player.rs` (`GiveDefaultItems`, `RemoveAllItems`, `PackDeadPlayerItems`, `ResetMaxSpeed` с предметом, команды `weapon_*`/`slotN`/`lastinv`/`invnext`/`invprev`/`drop`, `ImpulseCommands`);
  - `rules.rs` (рестарт: очистка `weaponbox`, гранат, предметов);
  - `world.rs`/`entity.rs` (`weaponbox`, `armoury_entity`, `weapon_*` на картах);
  - `messages.rs` (`WeaponList` при входе, `AmmoX`, `WeapPickup`, `AmmoPickup`, `ItemPickup`, `Crosshair`/`HideWeapon` по источнику);
  - `world.rs` precache (`WeaponsPrecache`: модели, звуки, события, спрайты всех стволов);
  - `crates/cs-proto48/src/usermsg.rs` (типизированные кодировщики `WeaponList`, `AmmoPickup`, `WeapPickup`, `ReloadSound`, `Crosshair`, `BarTime2`, `Brass`, если используются).

Перенести:
- `AddPlayerItem`, `RemovePlayerItem`, `SwitchWeapon`, `SelectItem`, `SelectLastItem`, `GiveAmmo`, `TabulateAmmo`;
- `BuyWeaponByWeaponID`, `BuyGunAmmo`, меню покупки по слотам (опись §2 — «authoritative buy mapping»), `HandleBuyAliasCommands`, замена основного и пистолета с выбросом;
- `DropPlayerItem`;
- `CWeaponBox`: время жизни, касание и подбор;
- `CArmoury`;
- `DeadPlayerWeapons`;
- `GetNextBestWeapon`.

- [ ] **Step 1: Тесты:**
  - `default_items_by_team`;
  - `buy_drop_pickup_as_regamedll`;
  - `buying_a_primary_drops_the_old_one`;
  - `ammo_buy_and_caps`;
  - `death_packs_a_weaponbox`;
  - `armoury_gives_items`;
  - `weapon_switch_commands`;
  - `round_restart_clears_weapons_on_the_ground`;
  - `hostile_weapon_commands_are_ignored`;
  - `weapon_list_at_join_as_regamedll`.
- [ ] **Step 2–5:** … → Commit `feat(game): inventory, weapon buying, drop, weaponbox and armoury`

### Task 13: `cs-game` — гранаты

**Files:**
- Modify: `crates/cs-game/src/weapons/grenades.rs` (бросок HE, флешки, дыма: `m_flStartThrow`/`m_flReleaseThrow`, скорость и угол броска).
- Create: `crates/cs-game/src/grenade.rs` (`CGrenade`), `crates/cs-game/tests/grenades.rs`.
- Modify: `entity.rs`, `player.rs` (`m_blindUntilTime` и ослепление).

Перенести `ggrenade.cpp` и `combat.cpp`:
- `ShootTimed`/`ShootSmokeGrenade`;
- `BounceTouch`, `SlideTouch`, `TumbleThink`;
- `Detonate`/`Explode`: HE — `RadiusDamage` с радиусом по источнику, проверить множитель;
- флешка: `RadiusFlash`, `ScreenFade` по углу и дистанции, ослепление;
- дым: события и `TE_SMOKE`;
- звуки, `TE_EXPLOSION`/`TE_BEAMFOLLOW`/декали.

Физика полёта — движок (MOVETYPE_BOUNCE).

- [ ] **Step 1: Тесты:**
  - `he_radius_damage_as_regamedll`;
  - `flash_blinds_by_angle_and_distance`;
  - `smoke_puffs_events`;
  - `grenade_bounces_and_detonates_on_time`;
  - `throw_velocity_from_pitch`.
- [ ] **Step 2–5:** … → Commit `feat(game): HE, flash and smoke grenades as ReGameDLL`

### Task 14: Документы, реестр, чек-лист

**Files:**
- Create:
  - `docs/parity/weapons.toml` — категория «Оружие, предметы, экономика», вес 10: пункты по стволам, автомату, пулям, урону, инвентарю, покупке, гранатам, предсказанию;
  - `docs/parity/tempents.toml` — вес 4: каждый `TE_*`;
  - `docs/parity/checklists/phase2d-weapons.md`.
- Modify: `usermsg.toml`, `delta.toml` (`weapon_data_t.*`, `event_t.*`), `engine.toml` (`studio_hitbox_traces`, события), `entities.toml` (`game_weapons_and_equipment_items` и связанные отличия), `wire.toml` (`svc_event`, `svc_weaponanim`), `THIRD-PARTY-NOTICES.md`, `docs/LICENSING.md`, `README.md`.

Чек-лист владельца — оригинальный клиент с `cl_lw 1` на нашем сервере:
- покупка каждого класса;
- стрельба без дрожания модели оружия;
- разброс и отдача на глаз;
- перезарядка;
- прицел;
- глушитель и очереди;
- нож;
- гранаты, флешка, дым;
- выброс и подбор;
- хедшоты;
- `DeathMsg`;
- деньги за убийство.

Пометки «From memory - confirm» — где ожидание не из источника.

- [ ] **Step 1–3:** Документы → `cargo xtask parity` → Commit `docs: weapons registry, checklist and notices`
