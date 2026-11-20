# Фаза 2A: лицензия и сервисы движка — план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> Субагенты запускаются только с `model: "opus"`, `effort: "medium"`. Не `fork`, не модель основной сессии.

**Goal:** проект под GPL-3.0-or-later; игра обращается к движку только через вызовы `EngineApi` (геометрия, энтити карты, сообщения, think), а `cs-server --proto48` управляется с консоли и меняет карту — `WalkGame` при этом ходит по новой карте.

**Architecture:** геометрия BSP переезжает из `cs-sim` в `cs-engine` (`WorldCollision`) и раздаётся игре через `EngineApi`. Поля `EngineApi` становятся приватными: всё взаимодействие — методы, которые позже оборачиваются хуками модов. Движок сам создаёт энтити из лампа карты и отдаёт их игре на `spawn_entity`. Движение игрока (`pmove`) работает через трейт `MoveWorld`, поэтому одна и та же физика ходит и по старому `BspCollision`, и по геометрии движка.

**Tech Stack:** Rust (edition 2024), `cs-engine`, `cs-proto48`, `cs-sim`, `cs-probe`, `ctrlc` (только бинарь `cs-server`).

**Spec:** `docs/superpowers/specs/2026-11-20-cs16-rs-master-plan-design.md` — фаза 2 (п. 6) и граница движок↔игра (п. 4.3).

## Как разбита фаза 2

Фаза 2 мастер-плана (~20k строк) — это пять независимых частей. Каждая даёт работающий, проверяемый результат и получает свой план.

| Часть | Что | Выход |
|---|---|---|
| **2A (этот план)** | Лицензия; `EngineApi` как вызовы; геометрия, энтити карты, сообщения, think; консоль сервера | `changelevel` работает с `WalkGame` на настоящих картах |
| 2B | Мир и физика: связывание энтити, трассировки по энтити, типы движения, touch/триггеры, толкатели, компенсация лага | Двери, платформы и триггеры работают на тестовой игре |
| 2C | Игроки и правила на энтити: команды, спавн, раунды, экономика, покупка, HUD-сообщения, меню | Оригинальный клиент выбирает команду, покупает, видит HUD |
| 2D | Оружие: энтити оружия, данные предсказания, события, общий RNG, хитбоксы по костям, гранаты, C4 | Стрельба совпадает с предсказанием клиента |
| 2E | Энтити карт в правилах, заложники, боты на движке, сверка с оригиналом | Полный раунд `de_dust2` и `cs_office` оригинальным клиентом |

## Global Constraints

- Лицензия проекта — `GPL-3.0-or-later` (решение владельца, 2026-11-21). Код ReHLDS и ReGameDLL_CS можно читать и переносить с указанием источника в `THIRD-PARTY-NOTICES.md`.
- `unsafe_code = "forbid"` для всего workspace. Зависимости с `unsafe` внутри допустимы.
- Нагрузка: каждая команда cargo — из Git Bash, строго в виде `nice -n 19 env CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 cargo <args>`, по одной за раз. Ничего не собирать в профиле release.
- `cargo fmt` не запускать ни в каком виде.
- После каждой задачи зелёные: `cargo clippy --workspace --all-targets -- -D warnings` и `cargo test --workspace`.
- Библиотечные crates из строки wasm-check в `.github/workflows/ci.yml` проходят `cargo check --target wasm32-unknown-unknown`.
- Ассеты Valve, захваты и дампы в репозиторий не попадают. Тест, которому нужна установленная игра, без неё завершается успешно без проверок (образец — `crates/cs-sim/tests/walk_game.rs`).
- Реестр паритета: правила статусов — в `docs/parity/README.md`; `verified` только со сравнением с оригиналом.
- Игра не имеет доступа к внутренностям движка: после задачи 3 у `EngineApi` нет публичных полей.
- Ни один вызов `EngineApi` не паникует на данных игры: устаревший `EdictId`, слот вне диапазона, нечисловые координаты, слишком длинная строка — тихий отказ или значение по умолчанию.
- Старый путь (`cs-net`, `MatchWorld`, офлайн-игра клиента) остаётся рабочим; его тесты не ломаются.
- Утверждения о поведении оригинала, взятые по памяти, помечаются в коде и реестре как допущения; проверенные по исходнику ReHLDS — со ссылкой на файл и датой.

## Review Focus

1. Мусор от игры в сервисы движка (устаревшие id, слоты вне диапазона, NaN, гигантские строки и сообщения) — без паники, без роста памяти. Тесты: задачи 3 и 5.
2. Ламп энтити чужой карты — недоверенный ввод: тысячи ключей, неизвестные классы, битые числа, энтити больше, чем слотов — сервер стартует, лишнее отброшено. Тест: задача 4.
3. Консоль: пустые и кривые команды, кик несуществующего, `changelevel` на отсутствующую карту — понятный текст, сервер работает дальше. Тест: задача 8.
4. Think: энтити перепланирует себя каждый вызов, удаляет себя или соседа во время think — ограничено кадром, без доступа к удалённому. Тест: задача 6.
5. Поток сообщений от игры каждый кадр (всем, по PVS, в init-буфер) — лимиты клиента соблюдаются, сервер не падает. Тест: задача 5.

## Структура файлов

| Файл | Ответственность |
|---|---|
| `LICENSE`, `docs/LICENSING.md`, `THIRD-PARTY-NOTICES.md` | GPL-3.0-or-later и происхождение перенесённого кода |
| `crates/cs-engine/src/collision.rs` | `WorldCollision`: трассировка хуллов BSP, содержимое точки |
| `crates/cs-sim/src/trace.rs` | `BspCollision` поверх `WorldCollision`; игровые зоны, материалы, хитбоксы |
| `crates/cs-engine/src/game.rs` | `GameDll`, `EngineApi` — только методы |
| `crates/cs-engine/src/api_entities.rs`, `api_messages.rs` | Реализации методов `EngineApi` по темам |
| `crates/cs-engine/src/mapents.rs` | Создание энтити из лампа карты |
| `crates/cs-engine/src/think.rs` | Планировщик think |
| `crates/cs-engine/src/console.rs` | Команды консоли сервера |
| `crates/cs-server/src/proto48.rs`, `console_io.rs` | Чтение stdin, Ctrl+C, смена карты |
| `crates/cs-sim/src/pmove.rs` | Трейт `MoveWorld` |
| `crates/cs-sim/src/walk_game.rs` | `WalkGame` на геометрии движка |
| `docs/parity/checklists/phase2a-console.md` | Ручной чек-лист |

---

### Task 1: Лицензия GPL-3.0-or-later

**Files:**
- Modify: `Cargo.toml` (`license`), `LICENSE`, `docs/LICENSING.md`, `THIRD-PARTY-NOTICES.md`, `README.md`
- Test: `xtask/tests/licence.rs`

- [ ] **Step 1: Написать падающий тест** `xtask/tests/licence.rs`:

```rust
#[test] fn workspace_is_gpl3_or_later() {
    // корневой Cargo.toml: [workspace.package] license == "GPL-3.0-or-later"
    // каждый crates/*/Cargo.toml и xtask/Cargo.toml: license.workspace = true
    // LICENSE начинается с "GNU GENERAL PUBLIC LICENSE" и содержит "Version 3, 29 June 2007"
}
```

- [ ] **Step 2:** Run: `cargo test -p xtask --test licence` → FAIL.
- [ ] **Step 3: Заменить `LICENSE`** официальным текстом GPL v3. Получить одним запросом: `curl -sL https://www.gnu.org/licenses/gpl-3.0.txt`. Текст не редактировать.
- [ ] **Step 4:** `license = "GPL-3.0-or-later"` в корневом `Cargo.toml`; у `xtask` добавить `license.workspace = true`, если нет.
- [ ] **Step 5: Переписать `docs/LICENSING.md`:** лицензия проекта и дата решения (2026-11-21, решение владельца); раздел «Open question» заменить разделом «Resolved»: что было найдено (корневые `LICENSE` эталонов — MIT, заголовки исходных файлов — GPL v2-or-later с исключением Valve) и почему GPL-3.0-or-later закрывает оба чтения; запрет на перенос снят; оговорка про Half-Life SDK остаётся; ассеты Valve в репозитории не хранятся.
- [ ] **Step 6: `THIRD-PARTY-NOTICES.md`:** для ReHLDS и ReGameDLL_CS — оба факта о лицензии и текст MIT-уведомления, как сейчас; строку о запрете убрать; добавить, что перенесённый код распространяется здесь под GPL-3.0-or-later.
- [ ] **Step 7: README:** строка о лицензии.
- [ ] **Step 8:** Run: `cargo test -p xtask` → PASS. Commit: `git commit -m "chore: relicense to GPL-3.0-or-later"`

### Task 2: Геометрия в движке

**Files:**
- Create: `crates/cs-engine/src/collision.rs`, `crates/cs-sim/tests/collision_characterization.rs`
- Modify: `crates/cs-sim/src/trace.rs`, `crates/cs-engine/src/map.rs`, `crates/cs-engine/src/lib.rs`

**Interfaces — Produces:**

```rust
// cs-engine::collision — типы переезжают из cs-sim::trace без изменений
pub use …::{Hull, TraceResult, DIST_EPSILON, CONTENTS_EMPTY, CONTENTS_SOLID, CONTENTS_WATER,
            CONTENTS_SLIME, CONTENTS_LAVA, CONTENTS_SKY};
pub struct WorldCollision;      // плоскости, узлы, clipnodes, содержимое листьев, модели
impl WorldCollision {
    pub fn from_bsp(map: &BspMap) -> Self;
    pub fn model_count(&self) -> usize;
    pub fn model_bounds(&self, model: usize) -> Option<(Vec3, Vec3)>;
    pub fn trace_world(&self, start: Vec3, end: Vec3, hull: Hull) -> TraceResult;
    pub fn trace_model(&self, model: usize, origin: Vec3, start: Vec3, end: Vec3, hull: Hull) -> TraceResult;
    pub fn point_contents(&self, p: Vec3, hull: Hull) -> i32;
    pub fn point_in_model(&self, model: usize, origin: Vec3, p: Vec3) -> bool;
}
impl ServerMap { pub fn collision(&self) -> Option<&WorldCollision>; }   // None у stub
```

`cs-sim::trace` реэкспортирует перенесённые типы; публичный API `BspCollision` не меняется, внутри он держит `Arc<WorldCollision>` и делегирует трассировку. Модель вне диапазона: `trace_model` возвращает «ничего не задето», `point_in_model` — `false`.

- [ ] **Step 1: Характеризующий тест до переноса** (`collision_characterization.rs`, пропуск без игры): на `de_dust2` 2000 трасс от LCG (seed `0xC516`, state × 6364136223846793005 + 1442695040888963407; точки внутри границ мира, хуллы по кругу) через текущий `BspCollision::trace_hull` и 2000 вызовов `point_contents`; FNV-1a-хэш от битов `fraction`, конечной точки, нормали и флагов. Запустить, вписать полученные хэши константами в тест.
- [ ] **Step 2:** Run → PASS на текущем коде. Commit: `git commit -m "test(sim): pin collision results before moving geometry"`
- [ ] **Step 3: Написать падающие тесты** в `collision.rs`: `out_of_range_model_is_harmless`; `stub_map_has_no_collision`; на `de_dust2` (пропуск без игры) `world_trace_matches_bsp_collision` — те же 2000 трасс через `WorldCollision::trace_world` дают тот же хэш, что тест шага 1 для мира без брашей (если `BspCollision` учитывает браш-энтити, сравнивать с его трассой при отключённых брашах — сказать в отчёте, как именно).
- [ ] **Step 4: Перенести** ядро трассировки в `collision.rs`, перевести `BspCollision` на делегирование, добавить `ServerMap::collision`.
- [ ] **Step 5:** Run: `cargo test -p cs-engine -p cs-sim` → PASS, хэши шага 1 не изменились. wasm-check `cs-engine`, `cs-sim`.
- [ ] **Step 6: Commit.** `git commit -m "refactor: move BSP hull tracing into cs-engine as WorldCollision"`

### Task 3: `EngineApi` — только вызовы

**Files:**
- Create: `crates/cs-engine/src/api_entities.rs`
- Modify: `crates/cs-engine/src/game.rs`, `server.rs`, `lifecycle.rs`, `frame.rs`, `crates/cs-engine/tests/common/mod.rs`, `crates/cs-sim/src/walk_game.rs`

**Interfaces — Produces** (все поля `EngineApi` становятся приватными):

```rust
impl EngineApi<'_> {
    pub fn time(&self) -> f64;                      pub fn frametime(&self) -> f32;
    pub fn max_clients(&self) -> u8;                pub fn map_name(&self) -> &str;
    pub fn world(&self) -> EdictId;                 pub fn client_edict(&self, slot: u8) -> EdictId;
    pub fn create_entity(&mut self, classname: &str) -> Option<EdictId>;
    pub fn remove_entity(&mut self, id: EdictId);                   // мир и слоты игроков игнорируются
    pub fn ent(&self, id: EdictId) -> Option<&EntVars>;
    pub fn ent_mut(&mut self, id: EdictId) -> Option<&mut EntVars>;
    pub fn private<T: Any + Send>(&self, id: EdictId) -> Option<&T>;
    pub fn private_mut<T: Any + Send>(&mut self, id: EdictId) -> Option<&mut T>;
    pub fn set_private<T: Any + Send>(&mut self, id: EdictId, value: T);
    pub fn entity_ids(&self) -> Vec<EdictId>;                       // живые, по возрастанию индекса
    pub fn find_by_classname(&self, after: Option<EdictId>, classname: &str) -> Option<EdictId>;
    pub fn entities_in_sphere(&self, center: Vec3, radius: f32) -> Vec<EdictId>;
    pub fn precache_model(&mut self, name: &str) -> Result<u16, PrecacheError>;
    pub fn precache_sound(&mut self, name: &str) -> Result<u16, PrecacheError>;
    pub fn precache_generic(&mut self, name: &str) -> Result<u16, PrecacheError>;
    pub fn model_index(&self, name: &str) -> Option<u16>;
    pub fn sound_index(&self, name: &str) -> Option<u16>;
    pub fn set_model(&mut self, id: EdictId, name: &str) -> bool;   // "*N": ещё mins/maxs из BSP; false, если модель неизвестна
    pub fn cvar_f32(&self, name: &str) -> f32;                      // 0.0, если нет
    pub fn cvar_str(&self, name: &str) -> Option<String>;
    pub fn set_cvar(&mut self, name: &str, value: &str);
    pub fn trace_hull(&self, start: Vec3, end: Vec3, hull: Hull) -> TraceResult;          // только мир
    pub fn trace_model(&self, model: usize, origin: Vec3, start: Vec3, end: Vec3, hull: Hull) -> TraceResult;
    pub fn point_contents(&self, p: Vec3, hull: Hull) -> i32;
    pub fn point_in_model(&self, model: usize, origin: Vec3, p: Vec3) -> bool;
    pub fn can_see(&self, eye: Vec3, mins: Vec3, maxs: Vec3) -> bool;
    // без изменений: client_key, set_client_key, kick, is_client_active, reliable, unreliable, reliable_all
}
```

На карте без геометрии (`stub`) трассы возвращают «ничего не задето», `point_contents` — `CONTENTS_EMPTY`. Нечисловые аргументы трасс и `entities_in_sphere` — «ничего не задето» / пустой список. Трассировки по энтити — часть 2B.

- [ ] **Step 1: Написать падающие тесты** (юнит, в `api_entities.rs`):

```rust
#[test] fn stale_and_foreign_ids_are_rejected_everywhere()   // ent/ent_mut/private/set_private/remove_entity/set_model → None/false/ничего
#[test] fn create_and_remove_roundtrip()                     // classname записан; после remove id не разрешается; мир и игрок не удаляются
#[test] fn private_data_is_typed()                           // set_private::<u32>; private::<String>() == None
#[test] fn find_by_classname_iterates_in_index_order()       // after = None, затем after = Some(предыдущий) → все три, потом None
#[test] fn entities_in_sphere_uses_origin_and_handles_nan()
#[test] fn set_model_requires_a_precached_name()             // неизвестная → false, vars не тронуты
#[test] fn traces_on_a_stub_map_hit_nothing()
#[test] fn cvar_access_roundtrips_and_missing_is_zero()
```

- [ ] **Step 2:** Run: `cargo test -p cs-engine api_entities` → FAIL.
- [ ] **Step 3: Реализовать**, сделать поля приватными, перевести `TestGame` и `WalkGame` на методы. Поведение не меняется.
- [ ] **Step 4:** Run: `cargo test -p cs-engine -p cs-sim` → PASS. wasm-check.
- [ ] **Step 5: Commit.** `git commit -m "refactor(engine): EngineApi exposes calls only"`

### Task 4: Энтити из лампа карты

**Files:**
- Create: `crates/cs-engine/src/mapents.rs`, `crates/cs-engine/tests/mapents.rs`
- Modify: `crates/cs-engine/src/edict.rs` (`EntVars`), `game.rs`, `server.rs`, `map.rs`, `crates/cs-engine/tests/common/mod.rs`, `crates/cs-sim/src/walk_game.rs` (`spawn_entity` возвращает `false`; настоящая логика — в задаче 9)

**Interfaces — Produces:**

```rust
// EntVars += pub targetname: String, pub target: String, pub spawnflags: u32, pub rendercolor: [u8; 3]
pub const MAX_ENTITY_KEYS: usize = 256;  pub const MAX_KEY_BYTES: usize = 64;  pub const MAX_VALUE_BYTES: usize = 1024;
pub trait GameDll {
    /// false — движок удаляет энтити (мир не удаляется).
    fn spawn_entity(&mut self, eng: &mut EngineApi<'_>, id: EdictId, classname: &str, keys: &[(String, String)]) -> bool;
}
impl ServerMap { pub fn stub_with_entities(name: &str, entities: Vec<Entity>) -> Self; }
```

Порядок активации карты меняется: таблицы → **энтити лампа** → `server_activate` → `lock()` → baselines. Для каждой записи лампа: первая с `classname = worldspawn` — мир (edict 0), остальные через `alloc`. Движок заполняет из ключей: `classname`, `origin`, `angles` (или `angle`: число — рыскание, `-1` — вверх, `-2` — вниз), `model` (через `set_model`), `targetname`, `target`, `spawnflags`, `rendermode`, `renderamt`, `renderfx`, `rendercolor`. Все ключи (в пределах лимитов; лишние и слишком длинные отбрасываются) передаются игре. Число не разобралось — значение по умолчанию. Запись без `classname` пропускается. Кончились слоты — оставшиеся записи пропускаются, одна строка `tracing::error!` с количеством.

- [ ] **Step 1: Написать падающие тесты** (`tests/mapents.rs`, карта через `stub_with_entities`):

```rust
#[test] fn map_entities_reach_the_game_with_parsed_vars()
// worldspawn + "test_keep" {origin "1 2 3", angles "0 90 0", targetname "a", spawnflags "5", renderamt "128"}
// → world получил spawn_entity; у энтити origin, angles, targetname, spawnflags, renderamt как заданы
#[test] fn rejected_entities_are_removed()                 // TestGame возвращает false для "test_drop" → edict свободен
#[test] fn spawn_happens_before_activate_and_precache_still_works()
// порядок вызовов в TestGame: spawn_entity…, server_activate; precache из spawn_entity → Ok
#[test] fn angle_key_shortcuts()                           // "angle" "90" → angles.y = 90; "-1" → angles.x = -90; "-2" → 90
#[test] fn hostile_entity_lump_is_bounded()
// 5000 записей, у одной 2000 ключей, ключ 500 байт, значение 100 КБ, числа "abc"/"1e999"/"NaN", запись без classname
// → Server::new не паникует; живых энтити не больше ёмкости пула; игре пришло не больше 256 ключей на энтити,
//   каждый ключ ≤ 64 и значение ≤ 1024 байт; в EntVars нет нечисловых значений
#[test] fn changelevel_respawns_entities_from_the_new_map()
#[test] fn map_entity_is_visible_to_a_probe()              // энтити с model, прошедшая precache, есть у probe в entities()
```

- [ ] **Step 2:** Run → FAIL. **Step 3:** Реализовать; `TestGame::spawn_entity` принимает классы с префиксом `test_keep`, записывает `(classname, keys)` и порядок вызовов.
- [ ] **Step 4:** Run: `cargo test -p cs-engine` → PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat(engine): spawn entities from the map entity lump"`

### Task 5: Сообщения

**Files:**
- Create: `crates/cs-engine/src/api_messages.rs`, `crates/cs-engine/tests/messages.rs`
- Modify: `crates/cs-engine/src/game.rs`, `server.rs`, `signon.rs`, `frame.rs`, `crates/cs-proto48/src/messages.rs`, `crates/cs-probe/src/lib.rs`, `crates/cs-engine/tests/common/mod.rs`

**Interfaces — Produces:**

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MsgDest { One(u8), OneUnreliable(u8), All, Broadcast, Pvs(Vec3), Init }
pub const MAX_INIT_BUFFER: usize = 64 * 1024;
impl EngineApi<'_> {
    pub fn send(&mut self, dest: MsgDest, bytes: &[u8]);
    pub fn send_usermsg(&mut self, dest: MsgDest, msg: &CsUserMsg) -> bool;       // false — сообщение не зарегистрировано
    pub fn client_print(&mut self, slot: u8, text: &str);
    pub fn print_all(&mut self, text: &str);
    pub fn center_print(&mut self, slot: u8, text: &str);
    pub fn stufftext(&mut self, slot: u8, text: &str);
    pub fn emit_sound(&mut self, ent: EdictId, channel: u8, sample: &str, volume: f32, attenuation: f32, pitch: u8) -> bool;
    pub fn temp_entity(&mut self, dest: MsgDest, te_type: u8, payload: &[u8]);
    pub fn light_style(&mut self, index: u8, pattern: &str);
}
// cs-proto48
pub fn write_svc_centerprint(w: &mut MsgWriter, text: &str);
pub fn write_svc_temp_entity(w: &mut MsgWriter, te_type: u8, payload: &[u8]);
// cs-probe
impl<T: PacketTransport> ProbeClient<T> { pub fn messages(&self) -> &VecDeque<SvcMessage>; }   // последние 1024, без Nop/Time/ClientData/PacketEntities
```

Доставка: `One` — надёжный поток слота; `OneUnreliable` — очередь ненадёжных слота; `All` — надёжно всем клиентам после этапа `new`; `Broadcast` — в очередь ненадёжных каждого `Active`; `Pvs(p)` — в очередь ненадёжных тех `Active`, чей глаз видит точку (`can_see` с боксом ±1 вокруг `p`); `Init` — в init-буфер сервера, который дописывается в этап `spawn` каждому входящему (действует только во время `spawn_entity` и `server_activate`; в остальное время и при переполнении `MAX_INIT_BUFFER` ведёт себя как `All`). Init-буфер очищается при смене карты. `emit_sound`: звук должен быть в precache, иначе `false`; уходит как `svc_sound` по `Pvs(origin энтити)`; громкость и затухание приводятся к допустимым диапазонам. `light_style`: сервер хранит 64 стиля, этап `spawn` шлёт сохранённые, изменение рассылается через `All`. Методы `reliable`, `unreliable`, `reliable_all` удаляются — всё идёт через `send`. Существующие лимиты (очередь ненадёжных, потолок надёжных) действуют как прежде.

- [ ] **Step 1: Написать падающие тесты** (`tests/messages.rs`, через `Harness` и probe):

```rust
#[test] fn each_destination_reaches_exactly_its_audience()
// три probe: A и B Active, C на входе после `new`. One(A) → только A; All → A, B и C; Broadcast → A и B
#[test] fn pvs_message_respects_visibility()               // на stub (всё видно) доходит обоим; пропуск без игры: на de_dust2 с точкой у T-спавна CT-игрок не получает
#[test] fn usermsg_roundtrips_to_the_probe()               // send_usermsg(One, Money{…}) → в messages() UserMsg с теми же полями; незарегистрированное → false
#[test] fn prints_and_stufftext_arrive()
#[test] fn sound_requires_precache_and_reaches_listeners()
#[test] fn init_messages_are_replayed_to_late_joiners()    // send(Init, …) в server_activate → probe, вошедший позже, получает; после change_map — нет
#[test] fn light_styles_are_stored_and_updated()
#[test] fn out_of_range_slots_and_stale_entities_are_ignored()
#[test] fn per_frame_message_flood_respects_client_limits()
// игра шлёт каждый кадр 2 КБ в Broadcast, 2 КБ в Pvs и 1 КБ в Init в течение 3 с
// → без паники; unreliable_dropped вырос; ни одна датаграмма не больше MAX_UDP_PACKET; send_failures == 0;
//   init-буфер не больше MAX_INIT_BUFFER; клиенты получают обновления
```

- [ ] **Step 2:** Run → FAIL. **Step 3:** Реализовать, включая два писателя в `cs-proto48` с roundtrip-тестами и журнал сообщений probe.
- [ ] **Step 4:** Run: `cargo test -p cs-engine -p cs-proto48 -p cs-probe` → PASS. wasm-check.
- [ ] **Step 5:** `docs/parity/engine.toml`: `game_messages` (`tested`). Commit: `git commit -m "feat(engine): message destinations, sounds, prints and light styles"`

### Task 6: Think

**Files:**
- Create: `crates/cs-engine/src/think.rs`, `crates/cs-engine/tests/think.rs`
- Modify: `crates/cs-engine/src/edict.rs` (`EntVars::nextthink`), `game.rs`, `server.rs`

**Interfaces — Produces:**

```rust
// EntVars += pub nextthink: f64       // 0.0 — think не запланирован
pub const MAX_THINKS_PER_FRAME: usize = 4096;
pub trait GameDll { fn think(&mut self, _eng: &mut EngineApi<'_>, _id: EdictId) {} }
```

Каждый кадр после `start_frame`: собрать живые энтити с `0 < nextthink <= time` по возрастанию индекса (не больше `MAX_THINKS_PER_FRAME`; остальные — в следующем кадре); для каждой, если она ещё жива и её `nextthink` всё ещё наступил, обнулить `nextthink` и вызвать `think`. Энтити, запланированная во время прохода, ждёт следующего кадра. Нечисловой `nextthink` считается нулём.

- [ ] **Step 1: Написать падающие тесты** (`tests/think.rs`, с переключателями в `TestGame`):

```rust
#[test] fn think_fires_once_when_due()                      // nextthink = time + 0.5 → вызов на кадре, где time >= значения; ровно один
#[test] fn rescheduling_inside_think_waits_for_the_next_frame()   // think ставит nextthink = time → один вызов за кадр, не цикл
#[test] fn entity_removed_before_its_turn_is_skipped()      // A в think удаляет B, у которой think в этом же кадре → B не вызвана
#[test] fn entity_removing_itself_is_safe()
#[test] fn thinks_are_capped_per_frame()                    // 5000 энтити с наступившим think (пул увеличен) → 4096 в первом кадре, остальные во втором
#[test] fn nan_nextthink_never_fires()
```

- [ ] **Step 2:** Run → FAIL. **Step 3:** Реализовать. **Step 4:** Run: `cargo test -p cs-engine` → PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat(engine): entity think scheduling"`

### Task 7: Укрепление по итогам финального ревью

**Files:**
- Modify: `crates/cs-engine/src/connect.rs`, `server.rs`, `client.rs`, `crates/cs-engine/tests/connectionless.rs`, `tests/usercmd.rs`, `Cargo.toml`, `.github/workflows/ci.yml`

- [ ] **Step 1: Написать падающие тесты:**

```rust
#[test] fn garbage_does_not_spend_the_global_reply_budget()
// 5000 мусорных датаграмм, на которые нет ответа, затем getchallenge с нового адреса → ответ получен
#[test] fn challenged_connect_passes_while_the_global_budget_is_empty()
// бюджет исчерпан потоком getchallenge; connect с валидным challenge принят
#[test] fn repeated_refused_streams_drop_the_sender()
// 8 отвергнутых сжатых потоков подряд от одного клиента → слот свободен; второй клиент в игре
```

- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3: Реализовать:** общий бюджет списывается только когда ответ действительно будет отправлен; `pub const MAX_REFUSED_STREAMS: u32 = 8;` — счётчик подряд отвергнутых netchan-ом потоков на клиента, сброс при успешно принятом потоке, превышение — уход по общему пути.
- [ ] **Step 4: Минимальная версия Rust.** `rustc --version` на этой машине; вписать её `major.minor` в `rust-version` корневого `Cargo.toml` (сейчас заявлено `1.95`, никем не проверено); в CI убрать комментарий о непроверенной версии и добавить шаг `cargo check --workspace` на тулчейне этой версии.
- [ ] **Step 5:** Run: `cargo test -p cs-engine` → PASS. Commit: `git commit -m "fix(engine): charge reply budget only for replies, drop repeat stream offenders; pin verified rust-version"`

### Task 8: Консоль сервера

**Files:**
- Create: `crates/cs-engine/src/console.rs`, `crates/cs-engine/tests/console.rs`, `crates/cs-server/src/console_io.rs`
- Modify: `crates/cs-engine/src/game.rs`, `crates/cs-server/src/proto48.rs`, `crates/cs-server/Cargo.toml`

**Interfaces — Produces:**

```rust
#[derive(Debug, PartialEq)] pub enum ConsoleOutcome { Output(String), ChangeLevel(String), Quit }
impl<G: GameDll> Server<G> { pub fn console_command(&mut self, line: &str) -> ConsoleOutcome; }
pub trait GameDll { fn server_command(&mut self, _eng: &mut EngineApi<'_>, _argv: &[String]) -> Option<String> { None } }
pub const MAX_CONSOLE_LINE: usize = 1024;
```

Команды движка (разбор — `tokenize_goldsrc_line`): `status` — имя сервера, карта, по строке на клиента (слот, userid, имя, состояние, пинг, адрес); `kick <#userid | слот | имя> [причина]` — причина по умолчанию `Kicked by server`; `say <текст>` — `print_all("Console: <текст>\n")`; `changelevel <карта>` и `map <карта>` → `ChangeLevel(имя)`, имя только из `[A-Za-z0-9_-]`, до 64 символов, иначе `Output` с ошибкой; `quit`, `exit` → `Quit`; `<cvar>` — показать значение; `<cvar> <значение>` — установить, только для существующих cvar. Иначе — `GameDll::server_command`; если и она вернула `None` — `Output("Unknown command \"<имя>\"\n")`. Пустая строка — `Output("")`. Строка длиннее `MAX_CONSOLE_LINE` обрезается.

Бинарь: поток читает stdin построчно и шлёт строки в канал; главный цикл между кадрами забирает их и печатает `Output`; `ChangeLevel` — загрузить карту через VFS, при ошибке напечатать её и продолжить, иначе `Server::change_map`; `Quit` и Ctrl+C (crate `ctrlc`, флаг `AtomicBool`) — `Server::shutdown(net, "Server shutting down")`, ещё один кадр для отправки прощаний, выход с кодом 0.

- [ ] **Step 1: Написать падающие тесты** (`tests/console.rs`):

```rust
#[test] fn status_lists_connected_clients()
#[test] fn kick_by_userid_slot_and_name()                 // probe получает Disconnected(причина); "kick 99" и "kick nobody" → Output с текстом ошибки
#[test] fn say_reaches_everyone()
#[test] fn changelevel_validates_the_name()               // "changelevel de_dust2" → ChangeLevel("de_dust2"); "../x", "", 65 символов → Output с ошибкой
#[test] fn cvars_can_be_read_and_set()                    // "sv_gravity" → значение; "sv_gravity 600" → установлено; "nosuch 1" → Unknown command
#[test] fn game_commands_are_dispatched()
#[test] fn hostile_console_input_is_harmless()
// пустая строка, пробелы, 100 КБ текста, незакрытая кавычка, NUL и управляющие символы, "kick" без аргументов
// → без паники; каждый вызов возвращает Output; сервер продолжает кадры, клиент остаётся в игре
```

- [ ] **Step 2:** Run → FAIL. **Step 3:** Реализовать `console.rs`. **Step 4:** Run: `cargo test -p cs-engine --test console` → PASS.
- [ ] **Step 5: Бинарь:** `console_io.rs` и цикл в `proto48.rs`; `cargo add -p cs-server ctrlc`.
- [ ] **Step 6: Commit.** `git commit -m "feat(server): console commands, kick, changelevel and graceful shutdown"`

Проверка бинаря на настоящих картах — в задаче 9, когда `WalkGame` сможет сменить карту.

### Task 9: `pmove` через `MoveWorld`, `WalkGame` на геометрии движка

**Files:**
- Modify: `crates/cs-sim/src/pmove.rs`, `walk_game.rs`, `crates/cs-sim/tests/walk_game.rs`, `crates/cs-server/src/proto48.rs`

**Interfaces — Produces:**

```rust
// pmove.rs — в трейт входят ровно те методы, которые pmove сейчас вызывает у BspCollision
pub trait MoveWorld { /* trace_hull, is_on_ladder, point_contents — сигнатуры как у BspCollision */ }
impl MoveWorld for BspCollision
pub fn step_player_move<W: MoveWorld + ?Sized>(state: &mut PlayerMoveState, cmd: &MoveInputCmd, weapon_max_speed: f32,
                                              params: &MoveParams, world: &W, dt: f32) -> MoveStepEvents;
pub fn snap_spawn_to_floor<W: MoveWorld + ?Sized>(raw_spawn: Vec3, world: &W) -> Vec3;
// walk_game.rs
impl WalkGame { pub fn new(params: MoveParams) -> Self; }     // без геометрии и спавнов
```

`WalkGame::spawn_entity` (всегда возвращает `false` — своих энтити не держит): `info_player_start` → спавн CT, `info_player_deathmatch` → спавн T (позиция и рыскание из vars); `func_ladder` → индекс браш-модели в список лестниц; `func_wall`, `func_door`, `func_door_rotating`, `func_breakable`, `func_button`, `func_plat`, `func_train`, `func_pushable` → индекс в список твёрдых брашей. `server_deactivate` очищает спавны и оба списка. Движение идёт через адаптер над `EngineApi`: трасса — ближайшее попадание среди мира и твёрдых брашей (`trace_hull`, `trace_model`); лестница — `point_in_model` по списку лестниц; содержимое — `point_contents`. `cs-server` создаёт `WalkGame::new(params)`.

- [ ] **Step 1: Написать падающие тесты** (`tests/walk_game.rs`, пропуск без игры):

```rust
#[test] fn walk_game_follows_changelevel()
// de_dust2 → change_map(cs_office): после входа игрок стоит у спавна cs_office (в пределах 1 юнита от
// прижатой к полу позиции спавна новой карты), 100 команд вперёд — на земле, пол не дальше 1 юнита под ногами
#[test] fn spawns_come_from_the_map_entity_lump()          // число спавнов CT и T совпадает с BspMap::spawns
#[test] fn solid_brush_entities_block_movement()           // на карте с func_wall/func_breakable на пути трасса игрока упирается;
                                                           // если на de_dust2 подходящего места нет — выбрать карту из установленных и назвать её в отчёте
```

- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3: Реализовать.** Пять существующих тестов `walk_game` должны пройти с прежними границами (дистанция 440..=500, скорость в пределах 5 от 250, пол не дальше 1 юнита). Если числа изменились — показать старые и новые в отчёте; границы не двигать.
- [ ] **Step 4:** Run: `cargo test -p cs-sim` → PASS, включая 4 старых теста `cs-sim` и характеризующий тест задачи 2.
- [ ] **Step 5: Проверка бинаря** (debug, порт 27115): запустить сервер с командами в stdin — `(sleep 10; echo status; sleep 2; echo "changelevel cs_office"; sleep 10; echo status; echo quit) | cargo run -p cs-server -- --proto48 --map de_dust2 --port 27115` — и одновременно `cargo run -p cs-probe -- --connect 127.0.0.1:27115 --seconds 20 --walk`. Ожидается: probe в игре до и после смены карты; второй `status` показывает `cs_office`; сервер завершился с кодом 0; probe получил причину отключения. Убедиться, что процессов не осталось.
- [ ] **Step 6: Commit.** `git commit -m "feat(sim): pmove over MoveWorld, WalkGame on engine geometry, changelevel on real maps"`

### Task 10: Документы и итоговая проверка

**Files:**
- Create: `docs/parity/checklists/phase2a-console.md`
- Modify: `docs/parity/engine.toml`, `docs/parity/checklists/phase1-original-client.md`, `README.md`

- [ ] **Step 1: Чек-лист** `phase2a-console.md` для владельца, таблица с пустыми «Дата» и «Результат»: оригинальный клиент в игре → `kick` с причиной (текст виден клиенту); `say` (текст в чате/консоли клиента); `changelevel cs_office` с клиентом в игре (клиент загружает новую карту сам); `changelevel` дважды подряд; Ctrl+C (клиент видит отключение сразу, а не через таймаут). В `phase1-original-client.md` у строки про кик указать, что теперь она выполнима командой `kick`.
- [ ] **Step 2: Реестр.** `docs/parity/engine.toml`: `map_entity_spawn`, `think`, `console`, `game_messages` — `tested` с именами тестов; `engine_traces` — `tested` для мира и `missing`-элемент `entity_traces` (часть 2B). Ничего не помечается `verified`.
- [ ] **Step 3: README.** Лицензия; консольные команды режима `--proto48`; что режим по-прежнему «только ходьба».
- [ ] **Step 4: Итоговая проверка,** по одной команде: `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace`; строка wasm-check из CI; `cargo xtask parity`.
- [ ] **Step 5: Commit.** `git commit -m "docs: phase 2A checklist, registry and README"`

---

## Что вне этого плана

Связывание энтити в мире, трассировки по энтити, типы движения, touch и триггеры, толкатели, компенсация лага — часть 2B. Игроки, правила, HUD — 2C. Оружие — 2D. Энтити карт в правилах, заложники, боты, сверка с оригиналом — 2E. Усечение блока энтити вместо отбрасывания: поведение оригинала при переполнении датаграммы надо сначала прочитать в исходнике ReHLDS — отдельным пунктом в 2B.
