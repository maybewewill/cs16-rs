# Фаза 3: наш клиент как клиент Protocol 48 — план

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Новый crate `cs-clientcore` — ядро клиента Protocol 48 без привязки к платформе:
- подключение, signon, ресурсы и таблицы precache;
- хранилище энтити, baseline, delta, интерполяция;
- предсказание движения на общем pm-коде;
- события, temp entities, звук, usermsg → состояние HUD.

`cs-client` становится оболочкой: окно, рендер, ввод, HUD. Локальная игра — `cs-engine` в том же процессе через loopback. `cs-net` удаляется.

**Выход:** наш клиент играет на стоковом HLDS (эталонный стенд, порт 27016) и на нашем `cs-server`. Локальная игра идёт тем же кодом.

**Tech Stack:**
- Rust 2024: `cs-proto48`, `cs-transport`, `cs-assets`, `cs-vfs`, `cs-sim::pm` (выносится в `cs-pm`), wgpu (`cs-client`).
- Сводки: `survey-source.md`, `survey-code.md` в `.superpowers/sdd/2026-11-23-phase3-client/`.
- Источники лежат в `scratchpad\p3src\`.

## Global Constraints

- **Лицензии:**
  - Xash3D FWGS (GPL-3.0-or-later по заголовкам файлов) можно переносить, с указанием файла, функции и даты.
  - cs16-client: переносимы только файлы с заголовком GPL-2-or-later (`events/event_*.cpp`).
  - Файлы с заголовком Valve и Valve HLSDK (`cl_dll`, `pm_shared`, `common`) только читаются ради поведения. Код из них не переносится и не транслитерируется.
- **Эталон поведения клиента** — оригинальный клиент и захваты (`cs-probe`). Xash — это реимплементация: спорные места сверяются с захватами HLDS.
- **wasm:** `cs-clientcore` и всё под ним собираются для `wasm32-unknown-unknown` (добавить в CI-проверку). Время, сеть и файлы — только через трейты `cs-platform`/`cs-transport`/`cs-vfs`.
- **Один код мира** (анти-цель G «два кода мира»). Клиент рендерит любую энтити по `modelindex`, без особых случаев для своего сервера.
- **Legacy-код живёт до задачи T8:** `cs-sim::MatchWorld`, legacy-режим `cs-server`, `cs-client` на `cs-net`. В T8 legacy-путь клиента и `cs-net` удаляются вместе с legacy-режимом сервера. Решение плана: офлайн-игра с ботами переходит на локальный сервер с ботами из 2E. `cs-sim::MatchWorld` остаётся библиотекой, пока на ней есть тесты.
- **Предсказание оружия в этой фазе — учётное:**
  - анимации, `next_attack` и обойма идут из `weapon_data`/`clientdata`;
  - ход времени — по таймерам из `weapon_data`;
  - события выстрела проигрываются по `svc_event` с `skiplocal`.

  Полный клиентский `ItemPostFrame` на общем коде оружия — фаза 6. Это отклонение записывается в реестр.
- Тесты без проверок по реальному времени. Сетевые тесты — через `LoopbackHub`. Golden-реплеи — из `.cs48cap`.
- **Порядок cargo:** throttle, `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=0`. Влитые worktree удаляются сразу.

## Порядок и волны

| Волна | Задачи |
|---|---|
| W1 | T1 crate `cs-pm`; T2 читатели `cs-proto48`; T3 каркас `cs-clientcore`: подключение, ресурсы |
| W2 | T4 кадры, энтити, интерполяция; T6 события, tempents, звук, usermsg → HUD |
| W3 | T5 предсказание; T7 локальная игра |
| W4 | T8 оболочка `cs-client` на clientcore, удаление `cs-net` |
| W5 | T9 проверка на стоковом HLDS, документы |

---

### Task 1: crate `cs-pm` — общий pm-код и трассы без движка

**Files:**
- Create: `crates/cs-pm/` — перенос `cs_sim::pm`, pm-константы, трейт `PmWorld`.
- Create: трассировка по hull'ам BSP и physents (из `cs-engine/src/pm.rs` и коллизии), не зависящая от edict'ов.
- Modify: `cs-engine` (server `PmWorld` поверх `cs-pm`), `cs-sim` (реэкспорт для legacy), `cs-game`.

**Сделать:**
- Разорвать зависимость `cs-sim → cs-engine` для pm-кода. `cs-pm` зависит только от `cs-assets` и `glam` и собирается в wasm.
- Сервер получает physents из edict'ов, клиент — из своего хранилища энтити (T5).

**Поведение не меняется:** все тесты движения (`pm_*`, `walk_game`, `cs-game` pmove) зелёные без правок ожиданий.

- [ ] **Тесты:** весь набор pm-тестов на новом месте; `pm_trace_on_a_bsp_without_the_engine`; wasm-проверка `cs-pm`.
- [ ] Commit `refactor: cs-pm crate, engine-neutral player movement and traces`

### Task 2: `cs-proto48` — читатели, которых не хватает клиенту

**Files:** `crates/cs-proto48/src/{messages,tempent,sound}.rs`.

**Сделать:**
- Типизированный читатель `svc_temp_entity` — все TE, которые шлёт ReGameDLL (как минимум 14, что шлёт наш сервер), остальные пропускаются по длине.
- Типизированные `svc_particle`, `svc_damage`, `svc_spawnstatic`, `svc_spawnstaticsound`, `svc_customization`, `svc_crosshairangle`, `svc_soundfade`, `svc_timescale`.
- Проверить `svc_sound` (9 бит флагов, origin по bit-coord) по Xash `cl_parse.c` и по захвату.

- [ ] **Тесты:**
  - round-trip writer → reader для каждого TE;
  - разбор golden-захватов HLDS (`hlds_join`, `hlds_office_join`) без `Ignored` для перечисленных svc.
- [ ] Commit `feat(proto48): client-side readers for temp entities and remaining svc`

### Task 3: `cs-clientcore` — каркас, подключение, ресурсы

**Files:**
- Create: `crates/cs-clientcore/` (`connection.rs`, `resources.rs`, `precache.rs`, `cvars.rs`, `commands.rs`).
- Modify: `cs-probe` — становится тонкой обёрткой над clientcore или делит с ним код подключения.

**Перенести из Xash** (`cl_main.c`, `cl_parse.c`; поведение сверять с захватами):
- машина состояний: challenge → connect → `new` → `sendres` → `spawn <count> <crc>` → `sendents` → первый полный кадр;
- `SIGNONS = 2`;
- ответы consistency и custom-resource, которые зонд сейчас пропускает;
- таблицы precache моделей, звуков, событий и generic по `svc_resourcelist` и связанным сообщениям;
- скачивание ресурсов: отсутствующие файлы — по `svc_resourcelocation`/fragments, если сервер разрешает; иначе ошибка с понятным текстом;
- userinfo, `setinfo`, `fullserverinfo`;
- `stufftext` → очередь консольных команд клиента (с белым списком опасных команд, как у оригинала, по источнику);
- cvar'ы клиента: `cl_updaterate`, `cl_cmdrate`, `rate`, `ex_interp`, `cl_lw`, `cl_lc`, `name`, `model`;
- отправка `clc_move` с частотой `cl_cmdrate` и `clc_delta`.

Транспорт — `cs-transport`, время — `cs-platform`, всё wasm-совместимо.

- [ ] **Тесты:**
  - `connects_to_our_server_over_loopback`;
  - `fills_the_precache_tables_as_the_server_sent_them`;
  - `answers_consistency_as_the_original`;
  - `stufftext_runs_allowed_commands_only`;
  - `cmd_rate_bounds_clc_move`.
- [ ] Commit `feat(clientcore): connection, signon, resources and precache tables`

### Task 4: кадры, хранилище энтити, интерполяция

**Files:** `crates/cs-clientcore/src/{frames,entities,interp}.rs`.

**Перенести из Xash `cl_frame.c`:**
- кольцо кадров (`CL_UPDATE_BACKUP`), delta от подтверждённого кадра, baseline, instanced baseline;
- история позиций энтити;
- render time = `cl.time - ex_interp`;
- интерполяция позиций, slerp углов;
- экстраполяция игроков до 1,2 интервала;
- `EF_NOINTERP`, `EF_MUZZLEFLASH`;
- локальный игрок не интерполируется (его ведёт T5).

API для рендера: список видимых энтити кадра — `modelindex`, origin, angles, frame, sequence, render-поля.

- [ ] **Тесты:**
  - `replays_a_golden_capture_into_the_entity_store` (`hlds_join`, `hlds_office_join`);
  - `interpolates_between_two_frames`;
  - `nointerp_snaps`;
  - `delta_from_an_unacked_frame_is_refused`.
- [ ] Commit `feat(clientcore): frames, entity store and interpolation`

### Task 5: предсказание

**Files:** `crates/cs-clientcore/src/predict.rs`.

**Перенести рецепт Xash `cl_pmove.c`:**
- повтор всех неподтверждённых команд от `svc_clientdata`;
- команды длиннее 50 мс делятся;
- события только на первом проходе;
- physents из хранилища энтити (T4) и BSP через `cs-pm`;
- телепорт при ошибке больше 64;
- сглаживание ошибки (0,25 за 0,1 с);
- учётное предсказание оружия по `weapon_data`.

- [ ] **Тесты:**
  - `prediction_matches_the_server_on_loopback`: наш сервер и клиент, 500 команд, расхождение 0 вне допуска;
  - `a_teleport_snaps`;
  - `long_commands_are_split`;
  - `prediction_uses_brush_entities`.
- [ ] Commit `feat(clientcore): movement prediction on cs-pm`

### Task 6: события, temp entities, звук, usermsg → состояние HUD

**Files:** `crates/cs-clientcore/src/{events,tempents,sound,hud}.rs`.

**Сделать:**
- `svc_event`/`svc_event_reliable`: очередь событий по индексу `events/*.sc`. Клиентские обработчики CS-событий оружия (`event_*.cpp` из cs16-client, GPL-2+) — анимация вида, гильзы, звук, декали — как данные для оболочки.
- **Tempents:** аллокатор и время жизни по Xash `cl_tent.c`; выход — список tempent'ов для рендера.
- **Звук:** `svc_sound`/`svc_stopsound`/`svc_spawnstaticsound`/`ambient` → команды звуку оболочки (позиция, канал, громкость, затухание, pitch).
- **Usermsg:** регистрация по `svc_newusermsg`; диспетчер в состояние HUD — здоровье, броня, деньги, патроны, `CurWeapon`/`WeaponList`, таймер, `StatusIcon`, `TextMsg`, `SayText`, `ScoreInfo`/`TeamInfo`, `DeathMsg`, радар, `BarTime`, меню (`ShowMenu`/`VGUIMenu`), MOTD. Неизвестные сообщения сохраняются для модов.

- [ ] **Тесты:**
  - `hud_state_after_golden_join` (значения из `hlds_join`);
  - `tempents_expire`;
  - `weapon_fire_event_produces_view_anim_and_shell`;
  - `unknown_usermsg_is_kept`.
- [ ] Commit `feat(clientcore): events, temp entities, sound and HUD state`

### Task 7: локальная игра

**Files:** `crates/cs-clientcore/src/local.rs` (или `cs-client`), `cs-server` (библиотечная точка входа listen-сервера).

**Сделать:**
- `cs-engine` + `cs-game` в том же процессе;
- клиент подключается по `LoopbackHub` настоящим handshake'ом Protocol 48;
- боты из 2E: `bot_quota`.

Один код для локальной и сетевой игры.

- [ ] **Тесты:**
  - `local_game_starts_and_spawns_the_player`;
  - `local_game_with_bots_plays_a_round` (по счётчикам, без реального времени).
- [ ] Commit `feat: local play through the engine over loopback`

### Task 8: оболочка `cs-client` на clientcore, удаление `cs-net`

**Files:** `crates/cs-client/*`; Delete: `crates/cs-net`, legacy-режим `cs-server`; Modify: workspace, README, документы паритета.

**Сделать:**
- Рендер по `modelindex` из clientcore:
  - BSP мира;
  - brush-подмодели с трансформом;
  - студийные модели (sequence, frame, gait);
  - спрайты — новый рендерер;
  - tempents.
- Камера от предсказанного игрока, `viewmodel` с анимацией из `weapon_data`/событий.
- Ввод → usercmd.
- HUD из состояния HUD T6. Меню покупки и команды — через stringcmd и menuselect, а не через `PlayerState`.
- Консоль: `connect <addr>`, `disconnect`, `map <name>` (локальная игра).
- Удалить `cs-net`, legacy-путь `cs-client` и `cs-server`.

- [ ] **Тесты:**
  - сборка;
  - headless-тест кадра: сцена из golden-реплея строится без паники;
  - существующие тесты рендера и интерфейса.
- [ ] Commit `feat(client): the client runs on cs-clientcore; remove cs-net`

### Task 9: проверка на стоковом HLDS, документы

**Сделать:**
- Наш клиент подключается к оригинальному HLDS (порт 27016): `de_dust2` и `cs_office`.
- Проверить ходьбу (без дрожания предсказания), стрельбу, покупку, HUD, табло, смену карты.
- Скриншоты сравниваются с оригинальным клиентом.
- Реестр: категории `render`/`ui`/`sound` — что сделано, `wire` — клиентская сторона.
- Чек-лист `docs/parity/checklists/phase3-clientcore.md`.
- `THIRD-PARTY-NOTICES.md`: Xash3D FWGS, cs16-client (`events`).

- [ ] Commit `docs: phase 3 client checklist and registry`
