# Фазы 0–1: мера и сервер Protocol 48 — план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> Субагенты запускаются только с `model: "opus"`, `effort: "medium"`. Не `fork`, не модель основной сессии.

**Goal:** оригинальный клиент CS 1.6 командой `connect` заходит на `cs-server --proto48`, видит карту и других игроков и бегает 10 минут без рассинхрона; процент готовности считает `cargo xtask parity`.

**Architecture:** новый crate `cs-engine` (аналог HLDS) держит пул edict, таблицы precache, сессии netchan, signon и построение кадров с delta; игра подключается через трейт `GameDll`. Доставка пакетов — за трейтом `PacketTransport` (`cs-transport`: UDP и loopback). Сверка — headless-клиент `cs-probe`, который работает и против нашего сервера в тестах, и против эталонного HLDS для записи golden-захватов.

**Tech Stack:** Rust 1.95, edition 2024, `cs-proto48` (кодек уже есть), `glam`, `thiserror`, `tracing`, `toml` + `serde` (только `xtask`), pure-Rust декодер BZ2.

**Spec:** `docs/superpowers/specs/2026-11-20-cs16-rs-master-plan-design.md`

## Global Constraints

- `unsafe_code = "forbid"` для всего workspace. Зависимости с `unsafe` внутри допустимы.
- `cargo clippy --workspace --all-targets -- -D warnings` и `cargo test --workspace` зелёные после каждой задачи.
- Библиотечные crates (`cs-proto48`, `cs-cvar`, `cs-assets`, `cs-vfs`, `cs-sim`, `cs-platform`, `cs-transport`, `cs-engine`, `cs-probe` как lib) проходят `cargo check --target wasm32-unknown-unknown`.
- Ассеты Valve, golden-захваты и дампы с оригинала в репозиторий не попадают. Захваты лежат в каталоге из переменной `CS16_GOLDEN_DIR`.
- Тест, которому нужна установленная игра или эталонный стенд, при их отсутствии завершается успешно без проверок. Образец: `let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else { return; };` в `crates/cs-sim/src/lib.rs`.
- В реестре паритета статус `verified` ставится только со ссылкой на тест или строку чек-листа с датой.
- Целевая версия оригинала — текущая Steam-сборка CS 1.6, `PROTOCOL_VERSION = 48`.
- Допуск позиции — 0.03125 юнита.
- Всё, что ниже помечено «рабочее допущение», взято по памяти о поведении HLDS. Арбитр — golden-захват с эталона (задача 8). При расхождении правится код, а не захват.
- Лицензия (задача 1) определяет, можно ли переносить код из ReHLDS/ReGameDLL. До решения — читать как справку о поведении, код не копировать.
- Старый путь (`cs-net`, `MatchWorld`, офлайн-игра клиента) остаётся рабочим. Его тесты не ломаются.

## Review Focus

1. Мусорные, обрезанные и чужие датаграммы с любого адреса — сервер не паникует, не отвечает больше, чем получил, без валидного challenge. Тесты: задачи 12 и 14.
2. Потеря, дублирование и перестановка пакетов во время signon и игры — вход завершается, движение не останавливается. Тесты: задачи 13 и 14.
3. Сервер полон, одинаковые имена, повторный `connect` с того же адреса, поток `connect` — отказ с причиной или переиспользование слота. Тесты: задачи 12 и 13.
4. Клиент молча пропал — слот освобождается по таймауту, остальные видят удаление. Тест: задача 16.
5. Завышенный `msec` и поток usercmd — перемещение ограничено реальным временем. Тест: задача 14.

## Структура файлов

| Файл | Ответственность |
|---|---|
| `.cargo/config.toml` | Алиас `cargo xtask` |
| `xtask/src/main.rs`, `xtask/src/parity.rs` | CLI и подсчёт паритета |
| `xtask/tests/registry_sync.rs` | Реестры совпадают с константами кода |
| `docs/parity/weights.toml`, `docs/parity/*.toml` | Веса и реестры |
| `docs/dev/reference-stand.md` | Как поднять эталонный стенд |
| `crates/cs-platform/src/lib.rs` | `Clock`, `ManualClock`, `NativeClock` |
| `crates/cs-vfs/src/lib.rs` | + `Vfs::from_memory` |
| `.github/workflows/ci.yml` | clippy, тесты, wasm-check, parity |
| `crates/cs-transport/src/lib.rs`, `udp.rs`, `loopback.rs` | Трейт и два транспорта |
| `crates/cs-proto48/src/compress.rs` | Распаковка `BZ2\0` |
| `crates/cs-proto48/src/messages.rs` | + недостающие писатели, `svc::ALL`, `clc::ALL` |
| `crates/cs-probe/src/lib.rs`, `capture.rs`, `main.rs` | Headless-клиент и формат захвата |
| `crates/cs-engine/src/edict.rs` | `EntVars`, `EdictPool` |
| `crates/cs-engine/src/precache.rs`, `map.rs` | Таблицы ресурсов, `ServerMap` и PVS |
| `crates/cs-engine/src/game.rs` | `GameDll`, `EngineApi` |
| `crates/cs-engine/src/oob.rs` | Challenge, connectionless, лимитер |
| `crates/cs-engine/src/server.rs`, `client.rs`, `signon.rs` | Цикл сервера, слоты, этапы входа |
| `crates/cs-engine/src/usercmd.rs` | Отбор команд, бюджет `msec` |
| `crates/cs-engine/src/frame.rs` | Кольцо кадров, delta, частота обновлений |
| `crates/cs-engine/tests/common/mod.rs` | `TestGame`, `Harness` |
| `crates/cs-sim/src/walk_game.rs` | `WalkGame`: ходьба по реальной карте |
| `crates/cs-server/src/main.rs` | Флаг `--proto48` |
| `docs/parity/checklists/phase1-original-client.md` | Ручной чек-лист |

---

## Фаза 0

### Task 1: База и лицензия

**Files:**
- Create: `docs/LICENSING.md`
- Modify: `Cargo.toml` (поле `license`), `README.md` (формулировка «clean-room»)

- [ ] **Step 1: Владелец выбирает лицензию.** Вопрос: `cs-proto48/src/lib.rs` заявляет «ported deterministically from ReHLDS and ReGameDLL_CS». Варианты: (а) проект переходит на `GPL-3.0-or-later`, код эталонов можно читать и переносить; (б) остаётся `MIT`, тогда `cs-proto48` переписывается по собственным захватам и описаниям протокола, а этот план исполняется без чтения исходников эталонов. Сначала проверить лицензии обоих репозиториев по их файлам `LICENSE`.
- [ ] **Step 2: Записать решение** в `docs/LICENSING.md`: выбранная лицензия, дата, что разрешено читать и переносить. Привести `Cargo.toml` и README в соответствие.
- [ ] **Step 3: Проверить базу.** Run: `cargo test --workspace` → 31 тест проходит. Run: `cargo clippy --workspace --all-targets -- -D warnings` → без ошибок.
- [ ] **Step 4: Commit.** `git add -A && git commit -m "chore: commit cs-proto48, record licensing decision"`

### Task 2: `cargo xtask parity`

**Files:**
- Create: `xtask/Cargo.toml`, `xtask/src/main.rs`, `xtask/src/parity.rs`, `.cargo/config.toml`
- Modify: `Cargo.toml` (members += `"xtask"`)

**Interfaces — Produces:**

```rust
pub enum Status { Missing, Implemented, Tested, Verified }
pub struct Item { pub id: String, pub status: Status, pub evidence: Option<String> }
pub struct Category { pub name: String, pub items: Vec<Item> }
pub struct Weights(pub BTreeMap<String, u32>);
pub struct CategoryScore { pub name: String, pub weight: u32, pub total: usize,
                           pub implemented: usize, pub tested: usize, pub verified: usize }
pub struct Report { pub categories: Vec<CategoryScore>, pub verified_pct: f64, pub implemented_pct: f64 }
pub enum ParityError { Toml(String), UnknownStatus { id: String, value: String },
                       DuplicateId(String), MissingEvidence(String), WeightsSum(u32), Io(String) }

pub fn parse_category(name: &str, toml_text: &str) -> Result<Category, ParityError>;
pub fn parse_weights(toml_text: &str) -> Result<Weights, ParityError>;
pub fn score(cats: &[Category], weights: &Weights) -> Report;
pub fn load_dir(dir: &Path) -> Result<(Vec<Category>, Weights), ParityError>;
```

Формат реестра:

```toml
[[item]]
id = "svc_serverinfo"
status = "tested"            # missing | implemented | tested | verified
evidence = "cs-proto48::messages::tests::full_signon_and_gameplay_svc_and_clc_streams_roundtrip"
```

Правила: счётчики накопительные (`verified` входит в `tested`, `tested` — в `implemented`). `verified_pct = Σ вес × verified/total ÷ Σ весов`. Категория из `weights.toml` без файла или с нулём элементов даёт 0 и остаётся в знаменателе.

- [ ] **Step 1: Написать падающие тесты** в `xtask/src/parity.rs`:

```rust
#[test] fn verified_only_counts_in_headline() {
    let w = parse_weights("a = 60\nb = 40").unwrap();
    let a = parse_category("a", r#"
        [[item]] id = "x" status = "verified" evidence = "t::x"
        [[item]] id = "y" status = "tested""#).unwrap();
    let r = score(&[a], &w);
    assert_eq!(r.verified_pct, 30.0);
    assert_eq!(r.implemented_pct, 60.0);
}
#[test] fn verified_without_evidence_is_rejected()   // Err(ParityError::MissingEvidence("x"))
#[test] fn duplicate_id_is_rejected()                // Err(ParityError::DuplicateId("x"))
#[test] fn unknown_status_is_rejected()              // status = "done" → UnknownStatus
#[test] fn weights_must_sum_to_100()                 // "a = 60\nb = 30" → WeightsSum(90)
```

- [ ] **Step 2:** Run: `cargo test -p xtask` → FAIL, функции не определены.
- [ ] **Step 3: Реализовать** функции из блока Interfaces. Зависимости: `cargo add -p xtask toml serde --features serde/derive`.
- [ ] **Step 4: CLI** в `xtask/src/main.rs`: `cargo xtask parity [--dir docs/parity] [--min <pct>]`. Печатает строку на категорию и итог `TOTAL verified: 0.0%  implemented: 0.0%`. Код выхода 1, если `verified_pct < min`. Алиас в `.cargo/config.toml`: `xtask = "run --quiet -p xtask --"`.
- [ ] **Step 5:** Run: `cargo test -p xtask` → PASS.
- [ ] **Step 6: Commit.** `git commit -m "feat(xtask): parity ledger scorer"`

### Task 3: Первые реестры

**Files:**
- Create: `docs/parity/weights.toml`, `wire.toml`, `usermsg.toml`, `delta.toml`, `cvars.toml`, `xtask/tests/registry_sync.rs`
- Modify: `crates/cs-proto48/src/messages.rs` (модули `svc`, `clc`), `xtask/src/parity.rs`, `xtask/Cargo.toml` (dev-deps `cs-proto48`, `cs-cvar`)

**Interfaces — Produces:**

```rust
// cs-proto48/src/messages.rs
pub mod svc { pub const ALL: &[(&str, u8)] = &[("bad", 0), ("nop", 1), /* … все 60 … */ ("exec", 59)]; }
pub mod clc { pub const ALL: &[(&str, u8)] = &[("bad", 0), /* … все 12 … */ ("cvarvalue2", 11)]; }
// xtask/src/parity.rs
pub fn import_names(existing: Category, dump_text: &str) -> Category;
```

`weights.toml` (значения из спеки, сумма 100): `wire = 8`, `usermsg = 6`, `delta = 3`, `tempents = 4`, `entities = 12`, `weapons = 10`, `movement = 8`, `cvars = 6`, `render = 10`, `sound = 4`, `ui = 8`, `engine = 7`, `bots = 4`, `web = 5`, `mods = 5`.

- [ ] **Step 1: Написать падающие тесты** `xtask/tests/registry_sync.rs`:

```rust
#[test] fn wire_registry_matches_opcode_constants()
// множество id в wire.toml == {"svc_<name>" для svc::ALL} ∪ {"clc_<name>" для clc::ALL}; всего 72
#[test] fn usermsg_registry_matches_cs16_default()
// множество id == имена из UserMsgRegistry::cs16_default().iter(); всего 73
#[test] fn delta_registry_matches_builtin_delta_lst()
// множество id == "<schema>.<field>" для каждого поля каждой схемы DeltaRegistry::cs16_default()
#[test] fn every_registered_cvar_is_in_registry()
// каждое имя из CVarRegistry::new_with_defaults() есть в cvars.toml
```

- [ ] **Step 2:** Run: `cargo test -p xtask --test registry_sync` → FAIL.
- [ ] **Step 3: Добавить `svc::ALL` и `clc::ALL`**, имена — константы в нижнем регистре.
- [ ] **Step 4: Заполнить реестры.** Статус элемента: `tested`, если его ветка чтения или функция записи исполняется существующим тестом `cs-proto48` (в `evidence` — путь теста); `implemented`, если код есть, теста нет; иначе `missing`. Для `svc_temp_entity` — `implemented` с пометкой в `evidence`: «payload не разбирается».
- [ ] **Step 5: `import_names`.** Добавляет имена из дампа как `missing`, существующие статусы не трогает. 👤 Владелец в консоли оригинала выполняет `cvarlist log cvarlist.txt` и `cmdlist log cmdlist.txt` (если синтаксис не сработает — `cvarlist`, затем `condump`) и кладёт файлы в `CS16_GOLDEN_DIR`. Правило разбора строк исполнитель выводит из реального файла и закрепляет тестом `import_names_parses_real_dump_lines` на пяти его строках, включая заголовок и итоговую строку. Подкоманда: `cargo xtask parity import-names <category> <file>`.
- [ ] **Step 6:** Run: `cargo test -p xtask` → PASS. Run: `cargo xtask parity` → печатает таблицу, `verified: 0.0%`.
- [ ] **Step 7: Commit.** `git commit -m "feat(parity): seed wire, usermsg, delta and cvar registries"`

### Task 4: Швы платформы

**Files:**
- Create: `crates/cs-platform/Cargo.toml`, `crates/cs-platform/src/lib.rs`
- Modify: `crates/cs-vfs/src/lib.rs`, `Cargo.toml` (members, `cs-platform = { path = … }`)

**Interfaces — Produces:**

```rust
// cs-platform
pub trait Clock { fn now(&self) -> f64; }                 // секунды, монотонно
pub struct ManualClock;  impl ManualClock { pub fn new() -> Self; pub fn advance(&self, dt: f64); }
#[cfg(not(target_arch = "wasm32"))]
pub struct NativeClock;  impl NativeClock { pub fn new() -> Self; }   // отсчёт от создания
// cs-vfs
impl Vfs { pub fn from_memory(files: impl IntoIterator<Item = (String, Vec<u8>)>) -> Self; }
```

`from_memory`: пути нормализуются через `Vfs::normalize`; `read_bytes`, `exists`, `list_dir`, `list_maps`, `file_count` работают; `resolve` для таких записей возвращает `None`.

- [ ] **Step 1: Написать падающие тесты:**

```rust
// cs-platform
#[test] fn manual_clock_advances() { let c = ManualClock::new(); c.advance(0.5); c.advance(0.25); assert_eq!(c.now(), 0.75); }
#[test] fn native_clock_is_monotonic()   // два чтения: второе >= первого
// cs-vfs
#[test] fn memory_vfs_reads_lists_and_is_case_insensitive() {
    let vfs = Vfs::from_memory([("Maps/De_Dust2.bsp".to_string(), b"x".to_vec())]);
    assert!(vfs.exists("maps/de_dust2.bsp"));
    assert_eq!(vfs.read_bytes("MAPS/de_dust2.BSP").unwrap(), b"x");
    assert_eq!(vfs.list_maps(), vec!["de_dust2"]);
    assert!(vfs.resolve("maps/de_dust2.bsp").is_none());
}
```

- [ ] **Step 2:** Run: `cargo test -p cs-platform -p cs-vfs` → FAIL.
- [ ] **Step 3: Реализовать.** В `Vfs` источник записи — перечисление «путь на диске | байты в памяти».
- [ ] **Step 4:** Run: `cargo test -p cs-platform -p cs-vfs` → PASS. Run: `cargo check -p cs-platform -p cs-vfs --target wasm32-unknown-unknown` → OK.
- [ ] **Step 5: Commit.** `git commit -m "feat: cs-platform clock and in-memory VFS"`

Отступление от спеки: прямые `std::fs` и `Instant` в `crates/cs-client/src/app.rs` остаются до фаз 3–4, когда клиент режется на `cs-clientcore`.

### Task 5: CI

**Files:**
- Create: `.github/workflows/ci.yml`

- [ ] **Step 1: Написать workflow** на `windows-latest` (совпадает с машиной разработки, не нужны системные пакеты звука и окна). Шаги: установка toolchain 1.95 с целью `wasm32-unknown-unknown`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace`; `cargo check --target wasm32-unknown-unknown -p cs-proto48 -p cs-cvar -p cs-assets -p cs-vfs -p cs-sim -p cs-platform`; `cargo xtask parity`.
- [ ] **Step 2: Проверить локально.** Run: `rustup target add wasm32-unknown-unknown`, затем четыре команды из шага 1 → все с кодом 0. Если crate не собирается под wasm — исправить в этом же шаге.
- [ ] **Step 3: Commit.** `git commit -m "ci: clippy, tests, wasm check, parity"`

Каждая следующая задача, создающая библиотечный crate, дописывает его в строку wasm-check.

---

## Фаза 1

### Task 6: `cs-transport`

**Files:**
- Create: `crates/cs-transport/Cargo.toml`, `src/lib.rs`, `src/udp.rs`, `src/loopback.rs`

**Interfaces — Produces:**

```rust
pub const MAX_UDP_PACKET: usize = 4010;      // рабочее допущение: значение из net.h эталона

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PeerAddr { Udp(std::net::SocketAddr), Loopback(u32) }   // Display: "ip:port" | "loopback:N"

pub enum TransportError { Io(std::io::Error), TooLarge(usize), Closed }

pub trait PacketTransport {
    fn recv(&mut self) -> Option<(PeerAddr, Vec<u8>)>;                       // не блокирует
    fn send(&mut self, to: PeerAddr, data: &[u8]) -> Result<(), TransportError>;
}

#[cfg(not(target_arch = "wasm32"))]
pub struct UdpTransport;
impl UdpTransport { pub fn bind(addr: &str) -> Result<Self, TransportError>; pub fn local_addr(&self) -> std::net::SocketAddr; }

pub struct LoopbackHub;        // сторона сервера; impl PacketTransport
impl LoopbackHub { pub fn new() -> Self; pub fn connect(&self) -> LoopbackEndpoint; }
pub struct LoopbackEndpoint;   // сторона клиента; impl PacketTransport; сервер = PeerAddr::Loopback(0)
impl LoopbackEndpoint { pub fn id(&self) -> u32; pub fn set_faults(&mut self, f: Faults); }
#[derive(Default, Clone, Copy)]
pub struct Faults { pub drop_every: u32, pub duplicate_every: u32, pub swap_every: u32 }   // 0 = выключено
```

`Faults` действуют на оба направления этого endpoint. Идентификаторы endpoint начинаются с 1.

- [ ] **Step 1: Написать падающие тесты:**

```rust
#[test] fn udp_roundtrip_on_localhost()           // два bind("127.0.0.1:0"); send → recv даёт те же байты и адрес отправителя
#[test] fn loopback_endpoints_get_distinct_ids()  // connect() дважды → id 1 и 2; hub.recv различает отправителей
#[test] fn loopback_drop_every_third()            // drop_every = 3, 9 отправок → hub получает 6
#[test] fn loopback_duplicate_and_swap()          // duplicate_every = 2 → 4 отправки дают 6; swap_every = 2 → порядок [2,1,4,3]
#[test] fn oversize_send_is_rejected()            // 4011 байт → Err(TooLarge(4011))
```

- [ ] **Step 2:** Run: `cargo test -p cs-transport` → FAIL.
- [ ] **Step 3: Реализовать.** UDP-сокет неблокирующий. Loopback — очереди под `Arc<Mutex<…>>`.
- [ ] **Step 4:** Run: `cargo test -p cs-transport` → PASS. wasm-check проходит (модуль `udp` под `cfg`).
- [ ] **Step 5: Commit.** `git commit -m "feat: cs-transport with UDP and fault-injecting loopback"`

### Task 7: Пробелы `cs-proto48`

**Files:**
- Create: `crates/cs-proto48/src/compress.rs`, `crates/cs-proto48/tests/fixtures/text.bz2`, `crates/cs-proto48/tests/fixtures/bomb.bz2`
- Modify: `crates/cs-proto48/src/netchan.rs`, `messages.rs`, `handshake.rs`, `query.rs`, `lib.rs`, `Cargo.toml`

**Interfaces — Produces:**

```rust
// compress.rs
pub const MAX_DECOMPRESSED_STREAM: usize = 4 * 1024 * 1024;
pub fn decompress_stream(payload: &[u8]) -> Result<Vec<u8>, ProtoError>;   // без префикса b"BZ2\0" — возвращает как есть
// messages.rs
pub fn write_svc_packetentities_delta(w: &mut MsgWriter, reg: &DeltaRegistry, max_clients: u8, delta_seq: u8,
    from: &BTreeMap<u16, (u8, DeltaRecord)>, to: &[GoldSrcEntityState], server_time: f64);
pub fn write_svc_sound(w: &mut MsgWriter, s: &SoundMsg);
pub fn write_svc_pings(w: &mut MsgWriter, pings: &[(u8, u16, u8)]);          // (слот, пинг мс, потери %)
pub fn write_clc_resourcelist(w: &mut MsgWriter, resources: &[ResourceEntry]);
pub fn write_clc_fileconsistency(w: &mut MsgWriter, payload: &[u8]);
// handshake.rs — сделать публичной
pub fn tokenize_goldsrc_line(line: &str) -> Vec<String>;
// query.rs
pub fn encode_s2c_query_challenge(challenge: i32) -> Vec<u8>;                // 0xFFFFFFFF 'A' i32, 9 байт
```

`NetChan::process_packet` пропускает собранный normal-stream через `decompress_stream`.

- [ ] **Step 1: Сгенерировать фикстуры.**
  Run: `python -c "import bz2,sys; sys.stdout.buffer.write(bz2.compress(b'cs16-rs netchan bz2 fixture ' * 64))" > crates/cs-proto48/tests/fixtures/text.bz2`
  Run: `python -c "import bz2,sys; sys.stdout.buffer.write(bz2.compress(bytes(8*1024*1024)))" > crates/cs-proto48/tests/fixtures/bomb.bz2`
- [ ] **Step 2: Написать падающие тесты:**

```rust
#[test] fn bz2_prefixed_stream_is_decompressed() {
    let mut p = b"BZ2\0".to_vec(); p.extend_from_slice(include_bytes!("fixtures/text.bz2"));
    assert_eq!(decompress_stream(&p).unwrap(), b"cs16-rs netchan bz2 fixture ".repeat(64));
}
#[test] fn plain_stream_passes_through()           // b"hello" → b"hello"
#[test] fn decompression_bomb_is_rejected()        // "BZ2\0" + bomb.bz2 → Err(ProtoError::InvalidPayload(_))
#[test] fn truncated_bz2_is_an_error_not_a_panic() // "BZ2\0" + первые 10 байт text.bz2 → Err
#[test] fn netchan_delivers_decompressed_fragment_stream()
// отправитель кладёт "BZ2\0"+text.bz2 через create_fragments; у получателя completed_normal_stream == распакованному тексту
#[test] fn delta_packetentities_roundtrip()
// from = {1, 5, 9}; to = {1 (origin сдвинут), 9 (без изменений), 12 (новая)}; после parse_svc_stream:
// is_delta == true, delta_from_seq == Some(seq), entities == to, removed == [5]
#[test] fn sound_pings_and_clc_resourcelist_roundtrip()
#[test] fn unknown_temp_entity_type_is_an_error()  // TE-тип 250 → Err, а не тихий пропуск
```

- [ ] **Step 3:** Run: `cargo test -p cs-proto48` → новые тесты FAIL, старые 12 PASS.
- [ ] **Step 4: Реализовать.** Декодер BZ2 — pure-Rust crate без `unsafe` в нашем коде, собирающийся под wasm32 (кандидат: `bzip2-rs`). Распаковка потоковая, с остановкой при превышении `MAX_DECOMPRESSED_STREAM`.
- [ ] **Step 5:** Run: `cargo test -p cs-proto48` → PASS. wasm-check проходит.
- [ ] **Step 6: Обновить `docs/parity/wire.toml`** для затронутых опкодов. Commit: `git commit -m "feat(proto48): BZ2 streams, delta packetentities writer, missing writers"`

### Task 8: `cs-probe` и golden-захваты

**Files:**
- Create: `crates/cs-probe/Cargo.toml`, `src/lib.rs`, `src/capture.rs`, `src/main.rs`, `tests/golden_replay.rs`, `docs/dev/reference-stand.md`
- Modify: `xtask/src/main.rs` (подкоманда `golden`)

**Interfaces:**
- Consumes: `PacketTransport`, `PeerAddr`, `UdpTransport` (задача 6); кодек `cs-proto48`, писатели из задачи 7.
- Produces:

```rust
pub struct ProbeConfig { pub name: String, pub steam_id: u64, pub cdkey_md5_hex: String }
#[derive(Debug, Clone, PartialEq)]
pub enum ProbeState { Idle, AwaitChallenge, AwaitConnection, Signon, InGame, Rejected(String), Disconnected(String) }
#[derive(Default, Debug, Clone, Copy)]
pub struct ProbeStats { pub datagrams_in: u64, pub bytes_in: u64, pub largest_datagram: usize,
                        pub full_updates: u64, pub delta_updates: u64, pub frames_in_last_second: u32 }

pub struct ProbeClient<T: PacketTransport>;
impl<T: PacketTransport> ProbeClient<T> {
    pub fn new(transport: T, server: PeerAddr, cfg: ProbeConfig) -> Self;
    pub fn connect(&mut self);
    pub fn pump(&mut self, now: f64);                       // принять всё, продвинуть состояние, отправить не чаще 100 пакетов/с
    pub fn queue_cmd(&mut self, cmd: GoldSrcUserCmd);
    pub fn send_stringcmd(&mut self, cmd: &str);
    pub fn force_delta_seq(&mut self, seq: Option<u8>);     // подмена clc_delta для тестов
    pub fn state(&self) -> &ProbeState;
    pub fn server_info(&self) -> Option<&ServerInfoMsg>;
    pub fn player_slot(&self) -> Option<u8>;
    pub fn entities(&self) -> &BTreeMap<u16, GoldSrcEntityState>;
    pub fn client_data(&self) -> Option<&GoldSrcClientData>;
    pub fn userinfos(&self) -> &BTreeMap<u8, String>;
    pub fn opcode_log(&self) -> &[u8];                      // опкоды svc по порядку; usermsg — своим id
    pub fn stats(&self) -> ProbeStats;
    pub fn set_capture(&mut self, w: CaptureWriter);
}
// capture.rs
pub const CAPTURE_MAGIC: &[u8; 8] = b"CS48CAP1";
pub enum Direction { Received = 0, Sent = 1 }
pub struct CaptureRecord { pub dir: Direction, pub time: f64, pub data: Vec<u8> }
pub struct CaptureWriter;  impl CaptureWriter { pub fn create(path: &Path) -> std::io::Result<Self>; pub fn push(&mut self, r: &CaptureRecord) -> std::io::Result<()>; }
pub fn read_capture(bytes: &[u8]) -> Result<Vec<CaptureRecord>, ProtoError>;
```

Формат записи после magic: `u8 dir`, `f64 time` LE, `u32 len` LE, сырые байты датаграммы (до снятия munge).

Ход входа (рабочее допущение): `getchallenge steam` → `connect 48 …` с `lan_steam_auth_ticket` → после `S2cConnection` отправить `new` → после `svc_serverinfo` отправить `sendres` → на `svc_resourcerequest` ответить пустым `clc_resourcelist` → после `svc_resourcelist` отправить `spawn <spawn_count> <crc>`, где crc — `map_crc`, обработанный `com_munge2` с ключом `(-1 - spawn_count) & 0xFF` → после `svc_signonnum 1` отправить `sendents` → первый `svc_packetentities` переводит в `InGame`. `svc_stufftext "reconnect\n"` возвращает к шагу `new`. Если в `svc_resourcelist` есть список consistency — состояние `Disconnected("consistency required: set mp_consistency 0")`.

- [ ] **Step 1: Написать падающие тесты:**

```rust
// src/capture.rs
#[test] fn capture_roundtrips()                 // 3 записи → read_capture даёт их же; неверный magic → Err
#[test] fn truncated_capture_is_an_error()
// tests/golden_replay.rs
#[test] fn golden_captures_parse_without_ignored_or_errors()
// нет CS16_GOLDEN_DIR → return. Для каждого *.cs48cap: датаграммы Received через NetChan::process_packet
// и parse_svc_stream; ни одного Err и ни одного SvcMessage::Ignored(_)
```

- [ ] **Step 2:** Run: `cargo test -p cs-probe` → FAIL.
- [ ] **Step 3: Реализовать** `capture.rs` и `ProbeClient` по описанному ходу входа.
- [ ] **Step 4: Бинарь** `cs-probe --connect <ip:port> [--name N] [--seconds S] [--capture FILE] [--walk]`. `--walk` шлёт команды «вперёд» с `msec = 10`. Подкоманда `cargo xtask golden <ip:port> <имя>` запускает его с `--capture $CS16_GOLDEN_DIR/<имя>.cs48cap --seconds 30 --walk`.
- [ ] **Step 5: Описать стенд** в `docs/dev/reference-stand.md`: установка HLDS через SteamCMD (app 90, мод cstrike), запуск с `+sv_lan 1 +mp_consistency 0 +map de_dust2`, опционально ReHLDS + ReGameDLL.
- [ ] **Step 6: 👤 Сверка с эталоном.** Владелец поднимает стенд. Run: `cargo xtask golden 127.0.0.1:27015 hlds_dust2_walk` → probe доходит до `InGame`, файл создан. Если не доходит — править `cs-probe` и `cs-proto48`, пока не дойдёт. Run: `cargo test -p cs-probe --test golden_replay` → PASS.
- [ ] **Step 7:** Run: `cargo test -p cs-probe` → PASS. Commit: `git commit -m "feat: cs-probe headless client and golden capture replay"`

Если стенда нет — шаг 6 откладывается, задачи 9–18 идут дальше; все «рабочие допущения» остаются непроверенными, и это записывается в реестр как `implemented`, не `verified`.

### Task 9: Пул edict

**Files:**
- Create: `crates/cs-engine/Cargo.toml`, `src/lib.rs`, `src/edict.rs`

**Interfaces — Produces:**

```rust
pub const DEFAULT_MAX_EDICTS: usize = 900;
pub const EDICT_REUSE_DELAY: f64 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EdictId { pub index: u16, pub serial: u32 }

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EntVars {
    pub classname: String, pub model: String,
    pub origin: Vec3, pub angles: Vec3, pub velocity: Vec3, pub v_angle: Vec3, pub punchangle: Vec3,
    pub view_ofs: Vec3, pub mins: Vec3, pub maxs: Vec3,
    pub modelindex: u16, pub weaponmodel: u16, pub sequence: u16, pub gaitsequence: u16,
    pub frame: f32, pub animtime: f32, pub framerate: f32, pub scale: f32,
    pub body: u8, pub skin: i16, pub colormap: u16,
    pub effects: u32, pub rendermode: u8, pub renderamt: u8, pub renderfx: u8,
    pub solid: u8, pub movetype: u8, pub flags: u32,
    pub health: f32, pub max_speed: f32, pub fov: f32, pub waterlevel: i32, pub deadflag: i32,
    pub owner: Option<EdictId>, pub aiment: Option<EdictId>,
    pub iuser1: i32, pub iuser2: i32, pub iuser3: i32, pub iuser4: i32,
}
pub struct Edict { pub vars: EntVars, pub private: Option<Box<dyn std::any::Any>> }

pub struct EdictPool;
impl EdictPool {
    pub fn new(max_clients: u8, max_edicts: usize) -> Self;      // индекс 0 — worldspawn, 1..=max_clients — игроки
    pub fn world(&self) -> EdictId;
    pub fn client(&self, slot: u8) -> EdictId;                   // индекс slot + 1
    pub fn alloc(&mut self, now: f64) -> Option<EdictId>;
    pub fn free(&mut self, id: EdictId, now: f64);               // world и слоты игроков не освобождаются
    pub fn reset_client(&mut self, slot: u8);                    // vars = default, private = None, serial += 1
    pub fn get(&self, id: EdictId) -> Option<&Edict>;
    pub fn get_mut(&mut self, id: EdictId) -> Option<&mut Edict>;
    pub fn by_index(&self, index: u16) -> Option<(EdictId, &Edict)>;
    pub fn iter_live(&self) -> impl Iterator<Item = (EdictId, &Edict)>;
    pub fn max_clients(&self) -> u8;
}
```

- [ ] **Step 1: Написать падающие тесты:**

```rust
#[test] fn first_alloc_is_after_player_slots()   // new(16, 900): alloc(0.0).unwrap().index == 17
#[test] fn stale_id_is_rejected_after_free()     // free → get(старый id) == None; новый alloc на том же индексе имеет другой serial
#[test] fn freed_edict_is_not_reused_within_half_a_second()
// alloc → free(now = 10.0) → alloc(10.4).index != старый; alloc(10.6) может вернуть старый
#[test] fn pool_exhaustion_returns_none()        // new(1, 4): 2 alloc дают Some, третий — None
#[test] fn world_and_clients_cannot_be_freed()   // free(world) и free(client(0)) ничего не делают
#[test] fn reset_client_invalidates_old_id()
```

- [ ] **Step 2:** Run: `cargo test -p cs-engine edict` → FAIL.
- [ ] **Step 3: Реализовать.**
- [ ] **Step 4:** Run: `cargo test -p cs-engine edict` → PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat(engine): edict pool"`

### Task 10: Precache и `ServerMap`

**Files:**
- Create: `crates/cs-engine/src/precache.rs`, `src/map.rs`

**Interfaces — Produces:**

```rust
pub const MAX_MODELS: usize = 512;  pub const MAX_SOUNDS: usize = 512;  pub const MAX_GENERIC: usize = 512;
pub mod res_type { pub const SOUND: u8 = 0; pub const SKIN: u8 = 1; pub const MODEL: u8 = 2; pub const DECAL: u8 = 3;
                   pub const GENERIC: u8 = 4; pub const EVENTSCRIPT: u8 = 5; pub const WORLD: u8 = 6; }
#[derive(Debug, PartialEq)] pub enum PrecacheError { TableFull(&'static str), TooLate }

pub struct PrecacheTables;
impl PrecacheTables {
    pub fn new(map_path: &str, submodel_count: usize) -> Self;   // models: [0]="", [1]=map_path, [2..]="*1".."*{n-1}"; sounds: [0]=""
    pub fn precache_model(&mut self, name: &str) -> Result<u16, PrecacheError>;
    pub fn precache_sound(&mut self, name: &str) -> Result<u16, PrecacheError>;
    pub fn precache_generic(&mut self, name: &str) -> Result<u16, PrecacheError>;
    pub fn model_index(&self, name: &str) -> Option<u16>;
    pub fn sound_index(&self, name: &str) -> Option<u16>;
    pub fn lock(&mut self);                                       // после server_activate; дальше новые имена → TooLate
    pub fn resource_list(&self, size_of: &dyn Fn(&str) -> u32) -> Vec<ResourceEntry>;
}

pub struct ServerMap { pub name: String, pub path: String, pub crc: u32, pub submodel_count: usize,
                       pub entities: Vec<cs_assets::bsp::Entity>, /* vis: Option<Arc<BspMap>> */ }
impl ServerMap {
    pub fn from_bsp(name: &str, raw: &[u8], bsp: Arc<BspMap>) -> Option<Self>;   // crc = crc_map_bsp(raw)
    pub fn stub(name: &str) -> Self;                                             // без vis, crc 0, 1 модель
    pub fn can_see(&self, eye: Vec3, mins: Vec3, maxs: Vec3) -> bool;
}
```

`resource_list` (рабочее допущение о порядке): звуки, затем модели, затем generic; имена `*N` идут с размером 0. `can_see`: у `stub` всегда `true`; иначе лист `eye` и его PVS, цель видима, если лист центра или любого из 8 углов её бокса входит в PVS. Это приближение — в `docs/parity/engine.toml` записать элемент `pvs_leaf_list` со статусом `missing` (точный список листов — фаза 2).

- [ ] **Step 1: Написать падающие тесты:**

```rust
#[test] fn map_and_submodels_occupy_first_model_slots() {
    let t = PrecacheTables::new("maps/de_dust2.bsp", 3);
    assert_eq!(t.model_index("maps/de_dust2.bsp"), Some(1));
    assert_eq!(t.model_index("*1"), Some(2));
    assert_eq!(t.model_index("*2"), Some(3));
}
#[test] fn precache_is_idempotent_and_case_insensitive()
#[test] fn table_full_and_locked_are_errors()      // 513-я модель → TableFull("model"); после lock() новое имя → TooLate, старое → Ok
#[test] fn resource_list_orders_sounds_models_generic_and_zero_sizes_submodels()
#[test] fn stub_map_sees_everything()
#[test] fn real_map_pvs_hides_far_spawn()          // при установленной игре: на de_dust2 из точки CT-спавна бокс игрока на T-спавне не виден
```

- [ ] **Step 2:** Run: `cargo test -p cs-engine precache map` → FAIL.
- [ ] **Step 3: Реализовать.** PVS — через `BspMap::find_leaf` и `BspMap::decompress_pvs`.
- [ ] **Step 4:** Run → PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat(engine): precache tables and server map with PVS"`

### Task 11: `GameDll` и `EngineApi`

**Files:**
- Create: `crates/cs-engine/src/game.rs`, `crates/cs-engine/tests/common/mod.rs`

**Interfaces — Produces:**

```rust
pub struct EngineApi<'a> {
    pub edicts: &'a mut EdictPool, pub precache: &'a mut PrecacheTables,
    pub cvars: &'a mut cs_cvar::CVarRegistry, pub map: &'a ServerMap, pub time: f64,
    /* приватно: исходящие буферы и userinfo клиентов */
}
impl EngineApi<'_> {
    pub fn reliable(&mut self, slot: u8) -> &mut MsgWriter;
    pub fn unreliable(&mut self, slot: u8) -> &mut MsgWriter;
    pub fn reliable_all(&mut self) -> &mut MsgWriter;
    pub fn client_key(&self, slot: u8, key: &str) -> Option<&str>;
    pub fn set_client_key(&mut self, slot: u8, key: &str, value: &str);   // помечает userinfo для рассылки
    pub fn kick(&mut self, slot: u8, reason: &str);
    pub fn is_client_active(&self, slot: u8) -> bool;
}

pub trait GameDll {
    fn usermsgs(&self) -> UserMsgRegistry;
    fn server_activate(&mut self, eng: &mut EngineApi<'_>);
    fn server_deactivate(&mut self, eng: &mut EngineApi<'_>);
    fn client_connect(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> Result<(), String>;
    fn client_put_in_server(&mut self, eng: &mut EngineApi<'_>, slot: u8);
    fn client_disconnect(&mut self, eng: &mut EngineApi<'_>, slot: u8);
    fn client_command(&mut self, eng: &mut EngineApi<'_>, slot: u8, argv: &[String]);
    fn client_userinfo_changed(&mut self, eng: &mut EngineApi<'_>, slot: u8);
    fn run_usercmd(&mut self, eng: &mut EngineApi<'_>, slot: u8, cmd: &GoldSrcUserCmd, random_seed: u32);
    fn start_frame(&mut self, eng: &mut EngineApi<'_>, frametime: f32);
    fn update_client_data(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> GoldSrcClientData;
    fn add_to_full_pack(&mut self, _eng: &mut EngineApi<'_>, _viewer: u8, _ent: u16, _state: &mut GoldSrcEntityState) -> bool { true }
}
pub fn entity_state_from_vars(index: u16, v: &EntVars) -> GoldSrcEntityState;   // entity_type = ENTITY_NORMAL
```

Остальные вызовы из п. 4.3 спеки (spawn/think/touch/use, трассировки, звук, поиск энтити) добавляются в фазе 2.

`tests/common/mod.rs` — `TestGame`: `client_put_in_server` ставит `origin = (64 × slot, 0, 0)`, `modelindex = 2`, `health = 100`; `run_usercmd` сдвигает `origin.x` на `forwardmove × msec / 1000`; `update_client_data` отдаёт `origin`, `health`, `maxspeed = 250`; ведёт счётчики вызовов каждого метода. Публичные поля для тестов: `refuse: Option<String>` (текст отказа в `client_connect`), `extra_entities: usize` (столько энтити с `modelindex = 2` и разными `origin` создаётся в `server_activate`), `late_precache: Option<Result<u16, PrecacheError>>` (результат попытки `precache_model("models/late.mdl")` из `client_put_in_server`), `commands: Vec<Vec<String>>` (всё, что пришло в `client_command`).

- [ ] **Step 1: Написать падающий тест:**

```rust
#[test] fn entity_state_copies_vars() {
    let v = EntVars { origin: Vec3::new(1.0, 2.0, 3.0), modelindex: 7, sequence: 4, effects: 8, ..Default::default() };
    let s = entity_state_from_vars(21, &v);
    assert_eq!((s.number, s.origin, s.modelindex, s.sequence, s.effects), (21, v.origin, 7, 4, 8));
}
```

- [ ] **Step 2:** Run → FAIL. **Step 3:** Реализовать `game.rs` и `TestGame`. **Step 4:** Run → PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat(engine): GameDll and EngineApi boundary"`

### Task 12: Connectionless и защита от отражения

**Files:**
- Create: `crates/cs-engine/src/oob.rs`

**Interfaces:**
- Consumes: `PeerAddr`; `parse_oob_packet`, `OobPacket`, `encode_s2c_challenge`, `encode_s2c_reject`, `encode_a2a_ack`, `encode_s2a_info_src`, `encode_s2a_players`, `encode_s2a_rules`, `encode_s2c_query_challenge`.
- Produces:

```rust
pub const CHALLENGE_BUCKET_SECS: f64 = 30.0;
pub const MAX_INFO_STRING: usize = 256;
pub const MAX_NAME_BYTES: usize = 31;

pub struct ChallengeIssuer;     // без таблицы: keyed-хэш (ключ процесса, адрес, номер 30-секундного окна)
impl ChallengeIssuer { pub fn new() -> Self; pub fn issue(&self, addr: &PeerAddr, now: f64) -> u32;
                       pub fn check(&self, addr: &PeerAddr, challenge: u32, now: f64) -> bool; }   // текущее и предыдущее окно

pub struct OobLimiter;          // token bucket на адрес: 10 ответов/с, запас 20, не больше 4096 адресов
impl OobLimiter { pub fn new() -> Self; pub fn allow(&mut self, addr: &PeerAddr, now: f64) -> bool; }

pub struct ConnectRequest { pub userinfo: InfoString, pub protinfo: InfoString, pub name: String }
pub enum OobOutcome { Reply(Vec<u8>), Connect(ConnectRequest), Ignore }
pub struct OobContext<'a> { pub challenges: &'a ChallengeIssuer, pub query: &'a ServerQueryInfo,
                            pub players: &'a [PlayerQueryEntry], pub rules: &'a [(&'a str, &'a str)], pub is_full: bool }
pub fn handle_oob(raw: &[u8], from: &PeerAddr, now: f64, ctx: &OobContext<'_>) -> OobOutcome;
pub fn sanitize_name(raw: &str) -> String;
```

Тексты отказов: `"Bad challenge.\n"`, `"Server is full.\n"`, `"Invalid userinfo in connect command\n"`, `"This server is using a different protocol ( 48 ) than your client ( {n} ).\n"`. `sanitize_name`: убрать управляющие символы, обрезать пробелы, обрезать до 31 байта по границе символа, пустое → `"unnamed"`. A2S_INFO, A2S_PLAYER и A2S_RULES без валидного challenge получают 9-байтный `encode_s2c_query_challenge`. Ключ хэша — `std::collections::hash_map::RandomState`. Порядок проверок `connect`: challenge → протокол → userinfo → заполненность; так длинные тексты отказа уходят только на подтверждённый адрес.

- [ ] **Step 1: Написать падающие тесты:**

```rust
#[test] fn getchallenge_reply_parses_and_validates()
// Reply разбирается как OobPacket::S2cChallenge { challenge, .. }; check(addr, challenge, now) == true
#[test] fn challenge_is_bound_to_address_and_expires()
// check(другой адрес) == false; check(now + 61.0) == false; check(now + 29.0) == true
#[test] fn connect_with_bad_challenge_is_rejected()       // Reply == encode_s2c_reject("Bad challenge.\n")
#[test] fn connect_with_wrong_protocol_is_rejected()      // protocol 47 → текст содержит "( 47 )"
#[test] fn connect_with_oversized_or_invalid_userinfo_is_rejected()   // 257 байт; строка без ведущего '\'
#[test] fn connect_when_full_is_rejected()
#[test] fn valid_connect_yields_request_with_sanitized_name()   // name "  A\x01B  " → "AB"
#[test] fn a2s_without_challenge_gets_nine_byte_challenge()
#[test] fn sanitize_name_rules()   // "" → "unnamed"; 40 × 'я' → не длиннее 31 байта и валидный UTF-8
#[test] fn limiter_caps_burst()    // 100 allow() в один момент с одного адреса → ровно 20 true; через 1.0 с — ещё 10
#[test] fn random_and_truncated_datagrams_never_panic_or_amplify() {
    // 10_000 датаграмм от LCG с seed 0xC516, длины 0..=64, плюс каждый префикс валидных getchallenge и connect.
    // Для каждой: handle_oob не паникует; если Reply(r) и raw не несёт валидный challenge, то r.len() <= 64.
}
```

- [ ] **Step 2:** Run: `cargo test -p cs-engine oob` → FAIL.
- [ ] **Step 3: Реализовать.**
- [ ] **Step 4:** Run → PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat(engine): connectionless handling with stateless challenges and rate limit"`

### Task 13: Сервер, слоты и signon

**Files:**
- Create: `crates/cs-engine/src/server.rs`, `src/client.rs`, `src/signon.rs`, `crates/cs-engine/tests/signon.rs`
- Modify: `crates/cs-engine/tests/common/mod.rs`, `crates/cs-engine/Cargo.toml` (dev-dep `cs-probe`)

**Interfaces:**
- Consumes: всё из задач 6–12.
- Produces:

```rust
pub struct ServerConfig { pub hostname: String, pub max_clients: u8, pub game_dir: String, pub build: i32,
    pub timeout_secs: f64, pub signon_timeout_secs: f64,
    pub min_update_rate: u32, pub max_update_rate: u32, pub min_rate: u32, pub max_rate: u32 }
// Default: "Counter-Strike 1.6 Rust", 16, "cstrike", 8684, 60.0, 120.0, 10, 102, 1000, 100_000

#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum SignonStage { AwaitNew, AwaitSendRes, AwaitSpawn, AwaitSendEnts }
#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum ClientState { Free, Signon(SignonStage), Active }

pub struct Server<G: GameDll>;
impl<G: GameDll> Server<G> {
    pub fn new(cfg: ServerConfig, map: ServerMap, game: G, deltas: DeltaRegistry, cvars: CVarRegistry) -> Self;
    pub fn frame(&mut self, net: &mut dyn PacketTransport, now: f64);
    pub fn client_state(&self, slot: u8) -> ClientState;
    pub fn client_count(&self) -> usize;
    pub fn spawn_count(&self) -> u32;
    pub fn game(&self) -> &G;
    pub fn game_mut(&mut self) -> &mut G;
    pub fn edicts(&self) -> &EdictPool;
}
// tests/common/mod.rs
pub struct Harness { pub server: Server<TestGame>, pub hub: LoopbackHub, pub now: f64 }
impl Harness {
    pub fn new(max_clients: u8) -> Self;                                           // ServerMap::stub("test")
    pub fn add_probe(&mut self, name: &str) -> ProbeClient<LoopbackEndpoint>;      // уже вызван connect()
    pub fn step(&mut self, probes: &mut [&mut ProbeClient<LoopbackEndpoint>], frames: usize);   // шаг 0.01 с
}
```

Содержимое этапов (рабочее допущение; арбитр — порядок опкодов в golden-захвате):

| Команда клиента | Сервер шлёт надёжным потоком | Следующий этап |
|---|---|---|
| `new` | `svc_print` (баннер), `svc_serverinfo`, `svc_sendextrainfo`, `svc_deltadescription` × все схемы, `svc_newmovevars` (из cvar `sv_*`), `svc_cdtrack`, `svc_setview(slot + 1)`, `svc_newusermsg` × все из `GameDll::usermsgs`, `svc_updateuserinfo` × активные | `AwaitSendRes` |
| `sendres` | `svc_resourcerequest(spawn_count)`, `svc_resourcelist` | `AwaitSpawn` |
| `spawn <n> <crc>` | при `n != spawn_count` — `svc_stufftext "reconnect\n"` и возврат в `AwaitNew`; иначе `svc_spawnbaseline`, вызов `client_put_in_server`, `svc_setangle`, `svc_time`, `svc_updateuserinfo` × все, `svc_lightstyle` × 64 (стиль 0 = `"m"`, остальные пустые), `svc_signonnum(1)` | `AwaitSendEnts` |
| `sendents` | — | `Active` |

Прочее: `GameDll::client_connect` вызывается при приёме `connect`; `Err(reason)` → отказ с этим текстом. Повторный `connect` с адреса занятого слота сбрасывает и переиспользует слот. Совпавшее имя получает префикс `(1)`, `(2)`… Несовпадение crc в `spawn` в этой фазе только логируется предупреждением. `clc_resourcelist` принимается и игнорируется; собранный file-stream отбрасывается. Клиент, не дошедший до `Active` за `signon_timeout_secs`, удаляется.

- [ ] **Step 1: Написать падающие тесты** `tests/signon.rs`:

```rust
#[test] fn probe_reaches_in_game_within_200_frames()
// после step(.., 200): probe.state() == InGame; server.client_state(0) == Active; put_in_server вызван 1 раз
#[test] fn signon_opcode_order_matches_working_assumption()
// opcode_log начинается с PRINT, SERVERINFO, SENDEXTRAINFO, DELTADESCRIPTION…, NEWMOVEVARS, CDTRACK, SETVIEW, NEWUSERMSG…
// и содержит по порядку RESOURCEREQUEST, RESOURCELIST, SPAWNBASELINE, SIGNONNUM
#[test] fn signon_opcode_order_matches_golden_capture()
// нет CS16_GOLDEN_DIR/hlds_dust2_walk.cs48cap → return; последовательность опкодов до первого PACKETENTITIES
// (без usermsg, NOP, TIME, PRINT) совпадает с захватом
#[test] fn signon_survives_every_third_packet_dropped()     // drop_every = 3 → InGame за 3000 кадров
#[test] fn signon_survives_duplicates_and_reordering()      // duplicate_every = 2, swap_every = 3 → InGame за 1000 кадров
#[test] fn second_client_on_full_server_is_rejected()       // Harness::new(1): второй probe → Rejected("Server is full.\n")
#[test] fn reconnect_from_same_address_reuses_slot()        // тот же endpoint заново connect() → client_count() == 1, InGame
#[test] fn duplicate_name_gets_numeric_prefix()             // два "Player" → в userinfos у второго name == "(1)Player"
#[test] fn stale_spawn_count_restarts_signon()              // send_stringcmd("spawn 999 0") в AwaitSpawn → снова SERVERINFO, итог InGame
#[test] fn game_can_refuse_connection()                     // TestGame с отказом → Rejected(текст игры)
#[test] fn stalled_signon_times_out()                       // probe перестаёт вызывать pump после `new`; через 121 с слот Free
```

- [ ] **Step 2:** Run: `cargo test -p cs-engine --test signon` → FAIL.
- [ ] **Step 3: Реализовать** `Server`, слоты и этапы. Каждый этап — отдельная функция в `signon.rs`, пишущая в `MsgWriter`.
- [ ] **Step 4:** Run → PASS (тест с golden пропускается без стенда).
- [ ] **Step 5: Commit.** `git commit -m "feat(engine): client slots, netchan sessions and signon"`

### Task 14: Команды клиента

**Files:**
- Create: `crates/cs-engine/src/usercmd.rs`, `crates/cs-engine/tests/usercmd.rs`
- Modify: `crates/cs-engine/src/server.rs`, `src/client.rs`

**Interfaces — Produces:**

```rust
pub const MAX_BANKED_MS: f64 = 500.0;
pub const MAX_DROPPED_REPLAY: u32 = 20;
pub const MAX_CONSECUTIVE_BAD_MOVES: u32 = 32;
pub const STRINGCMD_RATE_PER_SEC: f64 = 80.0;   pub const STRINGCMD_BURST: f64 = 400.0;

pub struct MsecBudget;
impl MsecBudget { pub fn new() -> Self; pub fn accrue(&mut self, real_dt_secs: f64);   // не выше MAX_BANKED_MS
                  pub fn take(&mut self, msec: u8) -> u8; }                             // min(msec, floor(остатка))

pub fn select_cmds_to_run(cmds_newest_first: &[GoldSrcUserCmd], num_backup: u8, dropped: u32,
                          last: &GoldSrcUserCmd) -> Vec<GoldSrcUserCmd>;                // результат — от старых к новым
```

`select_cmds_to_run`: новые команды — первые `len − num_backup` элементов. При `dropped == 0` — только новые. При `0 < dropped < 20` — сначала `dropped − min(dropped, num_backup)` повторов `last`, затем `min(dropped, num_backup)` самых свежих резервных, затем новые. При `dropped >= 20` — только новые. Команда с выданным `msec == 0` не исполняется. `random_seed` (рабочее допущение) = входящий номер пакета netchan минус индекс команды от новейшей. Строковые команды: `new`, `sendres`, `spawn`, `sendents`, `setinfo`, `disconnect`/`dropclient` обрабатывает движок, остальные уходят в `GameDll::client_command`; разбор — `tokenize_goldsrc_line`. Превышение лимита строковых команд — команда отбрасывается.

- [ ] **Step 1: Написать падающие тесты:**

```rust
// src/usercmd.rs
#[test] fn budget_grants_only_accrued_time() {
    let mut b = MsecBudget::new(); b.accrue(0.010);
    assert_eq!(b.take(10), 10); assert_eq!(b.take(10), 0);
}
#[test] fn budget_bank_is_capped()                 // accrue(5.0) → суммарно выдаётся не больше 500
#[test] fn no_loss_runs_only_new_cmds()
#[test] fn loss_within_backup_replays_backups()    // dropped = 2, backup = 2, 1 новая → 3 команды в порядке [b_старая, b_новая, новая]
#[test] fn loss_beyond_backup_repeats_last()       // dropped = 5, backup = 2 → [last, last, last, b, b, новая]
#[test] fn huge_loss_runs_only_new()               // dropped = 20
// tests/usercmd.rs
#[test] fn forward_cmds_move_the_player()          // 100 команд forwardmove = 250, msec = 10 за 1 с → origin.x ≈ старт + 250 ± 1
#[test] fn speedhack_is_bounded_by_real_time()
// msec = 20 при 100 командах в секунду, 2 с → пройдено не больше 250 × (2.0 + 0.5) юнитов
#[test] fn movement_continues_under_loss()         // drop_every = 3: путь за 2 с отличается от пути без потерь меньше чем на 5%
#[test] fn bad_move_checksum_is_ignored_then_client_dropped()
// 1 испорченный clc_move: клиент Active, позиция не изменилась; 32 подряд → слот Free
#[test] fn garbage_from_connected_address_never_panics()
// 10_000 LCG-датаграмм (seed 0xC516) с адреса активного клиента, в том числе с валидным заголовком netchan
// и случайным телом; сервер не паникует; второй клиент остаётся Active
#[test] fn stringcmd_flood_is_throttled()          // 5000 "say x" в одном кадре → client_command вызван не больше 400 раз
#[test] fn unknown_stringcmd_reaches_game()        // "jointeam 2" → argv ["jointeam", "2"]
```

- [ ] **Step 2:** Run → FAIL. **Step 3:** Реализовать. **Step 4:** Run: `cargo test -p cs-engine` → PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat(engine): usercmd execution with loss replay and msec budget"`

### Task 15: Кадры и delta

**Files:**
- Create: `crates/cs-engine/src/frame.rs`, `crates/cs-engine/tests/frames.rs`
- Modify: `crates/cs-engine/src/server.rs`

**Interfaces — Produces:**

```rust
pub const UPDATE_BACKUP: usize = 64;
pub const MAX_DATAGRAM: usize = 4000;

pub struct ClientFrame { pub seq: u32, pub time: f64, pub entities: BTreeMap<u16, (u8, DeltaRecord)>, pub clientdata: DeltaRecord }
pub struct FrameRing;
impl FrameRing { pub fn new() -> Self; pub fn store(&mut self, f: ClientFrame);
                 pub fn delta_base(&self, delta_seq: u8, current_seq: u32) -> Option<&ClientFrame>; }
pub fn update_interval(cl_updaterate: Option<&str>, cfg: &ServerConfig) -> f64;   // 1 / clamp(значение или 20, min, max)
pub fn clamp_rate(rate: Option<&str>, cfg: &ServerConfig) -> u32;                  // значение или 30000, в пределах min..=max
```

`delta_base` возвращает `None`, если клиент не прислал `clc_delta`, если `frame.seq & 0xFF != delta_seq`, или если `current_seq − frame.seq >= 63`. Кадр клиента: `svc_time`, `svc_clientdata`, затем `svc_packetentities` (полный) или `svc_deltapacketentities`. В кадр идут живые edict с `modelindex != 0` и без `EF_NODRAW` (`effects & 128`), прошедшие `ServerMap::can_see` от глаз зрителя и `GameDll::add_to_full_pack`; сам зритель включается всегда. Если ненадёжная часть больше `MAX_DATAGRAM` — блок энтити отбрасывается, уходят только `svc_time` и `svc_clientdata`. Частота — `update_interval` по ключу `cl_updaterate`; объём — token bucket по `clamp_rate` от ключа `rate`; при исчерпании шлётся `svc_choke`.

- [ ] **Step 1: Написать падающие тесты:**

```rust
// src/frame.rs
#[test] fn delta_base_requires_matching_recent_frame()   // store seq 100: (100 as u8, 101) → Some; (100, 163) → None; (99, 101) → None
#[test] fn update_interval_clamps()                      // "20" → 0.05; "1000" → 1/102; None → 0.05; "abc" → 0.05; "1" → 0.1
// tests/frames.rs
#[test] fn second_probe_sees_first_probe_entity()
// у B в entities() есть индекс A (slot + 1), origin в пределах 0.03125 от server.edicts()
#[test] fn stationary_world_frames_are_small_deltas()
// 100 кадров без движения: stats().delta_updates > 0, датаграмма ненадёжной части меньше 64 байт
#[test] fn stale_delta_request_falls_back_to_full_update()
// force_delta_seq(Some(seq − 70)) → следующий кадр увеличивает stats().full_updates
#[test] fn moved_entity_is_updated_and_removed_entity_disappears()
// A смещается → у B новый origin; A отключается → индекс A пропадает из entities() B
#[test] fn update_rate_follows_userinfo()                // "setinfo cl_updaterate 20" → frames_in_last_second в 19..=21
#[test] fn oversized_frame_drops_entities_but_keeps_clientdata()
// TestGame создаёт 800 энтити с разными полями → largest_datagram <= 4010, client_data() обновляется
#[test] fn hidden_entity_is_not_sent()                   // effects = 128 → отсутствует у зрителя
```

- [ ] **Step 2:** Run → FAIL. **Step 3:** Реализовать. **Step 4:** Run: `cargo test -p cs-engine` → PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat(engine): client frames with delta compression, PVS and rate control"`

### Task 16: Служебные сообщения и уход клиентов

**Files:**
- Modify: `crates/cs-engine/src/server.rs`, `src/client.rs`
- Create: `crates/cs-engine/tests/lifecycle.rs`

**Interfaces — Produces:**

```rust
impl<G: GameDll> Server<G> {
    pub fn kick(&mut self, slot: u8, reason: &str);
    pub fn shutdown(&mut self, net: &mut dyn PacketTransport, reason: &str);
}
pub const PINGS_INTERVAL_SECS: f64 = 2.0;
pub const NAME_CHANGE_MIN_INTERVAL_SECS: f64 = 1.0;
```

Правила: уход клиента по любой причине — `GameDll::client_disconnect`, `EdictPool::reset_client`, рассылка `svc_updateuserinfo` с пустым userinfo. Кик и `shutdown` шлют `svc_disconnect(reason)` тремя пакетами подряд. Молчание дольше `timeout_secs` — уход. `setinfo` обновляет userinfo, вызывает `client_userinfo_changed`, рассылает `svc_updateuserinfo`; смена `name` — не чаще раза в секунду и через `sanitize_name` с правилом дубликатов. `svc_pings` всем активным раз в 2 с; пинг — среднее время подтверждения последних 16 пакетов.

- [ ] **Step 1: Написать падающие тесты:**

```rust
#[test] fn silent_client_times_out_and_others_see_removal()
// A перестаёт вызывать pump; через 61 с: слот A Free, client_disconnect вызван 1 раз,
// у B нет индекса A в entities(), userinfos()[slot_a] == ""
#[test] fn kick_delivers_reason()                    // kick(0, "bye") → Disconnected("bye"), слот Free
#[test] fn client_disconnect_command_frees_slot()    // send_stringcmd("disconnect") → слот Free за 10 кадров
#[test] fn shutdown_notifies_everyone()
#[test] fn name_change_is_broadcast_sanitized_and_rate_limited()
// "setinfo name  X\x01Y " → у другого клиента name == "XY"; вторая смена в ту же секунду игнорируется
#[test] fn pings_are_sent_every_two_seconds()        // за 5 с в opcode_log 2 или 3 опкода PINGS
#[test] fn slot_is_reusable_after_disconnect()       // новый probe занимает освободившийся слот и доходит до InGame
```

- [ ] **Step 2:** Run → FAIL. **Step 3:** Реализовать. **Step 4:** Run: `cargo test -p cs-engine` → PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat(engine): disconnects, timeouts, userinfo updates and pings"`

### Task 17: Смена карты

**Files:**
- Modify: `crates/cs-engine/src/server.rs`, `crates/cs-engine/tests/lifecycle.rs`

**Interfaces — Produces:**

```rust
impl<G: GameDll> Server<G> { pub fn change_map(&mut self, map: ServerMap); }
```

Порядок: `server_deactivate` → `spawn_count += 1` → новые `EdictPool` и `PrecacheTables` → `server_activate` → `PrecacheTables::lock` → каждому клиенту `svc_stufftext "reconnect\n"`, состояние `Signon(AwaitNew)`, кольцо кадров очищается; сессия netchan сохраняется.

- [ ] **Step 1: Написать падающие тесты:**

```rust
#[test] fn probe_survives_changelevel()
// change_map(ServerMap::stub("second")) → InGame, server_info().map_path содержит "second", spawn_count вырос на 1
#[test] fn old_entities_are_gone_after_changelevel()
// extra_entities = 5 до смены; перед change_map выставить game_mut().extra_entities = 0 → у клиента после входа их нет
#[test] fn precache_after_activate_is_too_late()       // game().late_precache == Some(Err(PrecacheError::TooLate))
```

- [ ] **Step 2:** Run → FAIL. **Step 3:** Реализовать. **Step 4:** Run → PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat(engine): map change with client reconnect"`

### Task 18: `WalkGame`

**Files:**
- Create: `crates/cs-sim/src/walk_game.rs`
- Modify: `crates/cs-sim/src/lib.rs`, `crates/cs-sim/Cargo.toml` (deps `cs-engine`, `cs-proto48`; dev-deps `cs-probe`, `cs-transport`)

**Interfaces:**
- Consumes: `GameDll`, `EngineApi` (задача 11); `step_player_move`, `MoveInputCmd`, `MoveParams`, `PlayerMoveState`, `snap_spawn_to_floor`, `BspCollision`, `SpawnPoint` из `cs-sim`.
- Produces:

```rust
pub mod buttons { pub const IN_ATTACK: u16 = 1; pub const IN_JUMP: u16 = 2; pub const IN_DUCK: u16 = 4; pub const IN_USE: u16 = 32;
                  pub const IN_ATTACK2: u16 = 2048; pub const IN_RELOAD: u16 = 8192; pub const IN_SCORE: u16 = 32768; }
pub mod ent_flags { pub const FL_CLIENT: u32 = 1 << 3; pub const FL_ONGROUND: u32 = 1 << 9; pub const FL_DUCKING: u32 = 1 << 14; }
pub const WALK_MAX_SPEED: f32 = 250.0;

pub struct WalkGame;
impl WalkGame { pub fn new(collision: BspCollision, spawns: Vec<SpawnPoint>, params: MoveParams) -> Self; }
impl GameDll for WalkGame { /* … */ }
pub fn usercmd_to_move_input(cmd: &GoldSrcUserCmd, max_speed: f32) -> MoveInputCmd;
```

Поведение: `usermsgs` — `UserMsgRegistry::cs16_default()`. `server_activate` — precache `models/player.mdl` и `models/player/<m>/<m>.mdl` для `urban`, `gsg9`, `sas`, `gign`, `terror`, `leet`, `arctic`, `guerilla`. `client_put_in_server`: чётные слоты — спавны CT и ключ `model = "gign"`, нечётные — спавны T и `"leet"`; позиция через `snap_spawn_to_floor`; `modelindex` от `models/player.mdl`, `solid = 3`, `movetype = 3`, `health = 100`, `flags = FL_CLIENT`, `mins = (-16, -16, -36)`, `maxs = (16, 16, 36)`, `view_ofs.z = 17`, `max_speed = 250`, `fov = 90`. `run_usercmd`: `v_angle = cmd.viewangles`, `angles.y = v_angle.y`, шаг `step_player_move` с `dt = msec / 1000`, затем запись в vars `origin`, `velocity`, `view_ofs.z`, флагов `FL_ONGROUND` и `FL_DUCKING`. `usercmd_to_move_input`: `forward = clamp(forwardmove / max_speed, -1, 1)`, `right = clamp(sidemove / max_speed, -1, 1)`, `yaw_deg = viewangles.y`, `pitch_deg = viewangles.x`, `jump`/`duck` из кнопок, `walk = false`. Анимации игроков нет — фаза 2.

- [ ] **Step 1: Написать падающие тесты:**

```rust
#[test] fn usercmd_maps_to_move_input() {
    let cmd = GoldSrcUserCmd { forwardmove: 130.0, sidemove: -500.0, buttons: buttons::IN_JUMP | buttons::IN_DUCK,
                               viewangles: Vec3::new(10.0, 90.0, 0.0), ..Default::default() };
    let m = usercmd_to_move_input(&cmd, 250.0);
    assert!((m.forward - 0.52).abs() < 1e-6); assert_eq!(m.right, -1.0);
    assert_eq!((m.yaw_deg, m.pitch_deg, m.jump, m.duck), (90.0, 10.0, true, true));
}
#[test] fn player_runs_forward_on_de_dust2()
// при установленной игре: Server<WalkGame> + probe через loopback; 200 команд forwardmove = 250, msec = 10
// → горизонтальный путь в 400..=520 юнитов, |dz| <= 18, у edict выставлен FL_ONGROUND
#[test] fn clientdata_matches_server_origin()
// после тех же команд: client_data().origin в пределах 0.03125 от origin edict
#[test] fn spawns_alternate_teams_and_set_model_key()
// слот 0 → userinfo model == "gign"; слот 1 → "leet"; позиции не совпадают
```

- [ ] **Step 2:** Run: `cargo test -p cs-sim walk_game` → FAIL.
- [ ] **Step 3: Реализовать.**
- [ ] **Step 4:** Run: `cargo test -p cs-sim` → PASS, включая 4 старых теста.
- [ ] **Step 5: Commit.** `git commit -m "feat(sim): WalkGame on the engine boundary"`

### Task 19: `cs-server --proto48`, выносливость, ручная сверка

**Files:**
- Modify: `crates/cs-server/src/main.rs`, `crates/cs-server/Cargo.toml`, `README.md`, `docs/parity/wire.toml`
- Create: `crates/cs-engine/tests/soak.rs`, `docs/parity/engine.toml`, `docs/parity/checklists/phase1-original-client.md`

- [ ] **Step 1: Написать тесты выносливости** `tests/soak.rs`:

```rust
#[test] fn soak_3000_frames_8_clients()             // обычный
#[test] #[ignore] fn soak_60000_frames_8_clients()  // 10 минут модельного времени
// 8 probe, drop_every = 17, команды от LCG с seed 0xC516 (случайные forwardmove, sidemove, углы, msec = 10).
// В конце: все 8 в состоянии Active; у каждого client_data().origin в пределах 0.03125 от origin своего edict;
// largest_datagram <= 4010; в кольце кадров каждого клиента не больше 64 записей.
```

- [ ] **Step 2:** Run: `cargo test -p cs-engine --test soak` → PASS. Run: `cargo test -p cs-engine --release --test soak -- --ignored` → PASS. Провал — исправлять движок, а не тест.
- [ ] **Step 3: Флаг `--proto48`** в `cs-server`: `ServerMap::from_bsp`, `WalkGame`, `UdpTransport`, `NativeClock`, цикл 100 Гц с вызовом `Server::frame`; флаги `--maxplayers <n>` (по умолчанию 16) и `--hostname <s>`. Без `--proto48` поведение прежнее.
- [ ] **Step 4: Проверка probe по UDP.** Run: `cargo run --release -p cs-server -- --proto48 --map de_dust2`, во втором терминале `cargo run --release -p cs-probe -- --connect 127.0.0.1:27015 --seconds 20 --walk` → probe печатает `InGame` и завершается с кодом 0.
- [ ] **Step 5: 👤 Чек-лист с оригинальным клиентом** в `docs/parity/checklists/phase1-original-client.md`; каждая строка получает дату и результат:
  1. `connect 127.0.0.1:27015` в оригинальном клиенте — полоса загрузки доходит до конца.
  2. Карта отрисована, камера в точке спавна.
  3. Ходьба, прыжок, присед, лестница — без рывков и откатов.
  4. Второй клиент или `cs-probe --walk` виден и движется плавно.
  5. 10 минут игры без отключения.
  6. `disconnect` и повторный вход проходят.
  7. Сервер виден в браузере серверов на вкладке LAN.
  При провале пункта — записать, что показал клиент, снять захват через `cs-probe` с эталона на том же сценарии и исправить расхождение.
- [ ] **Step 6: Обновить реестры.** `docs/parity/engine.toml`: элементы `signon`, `delta_frames`, `pvs_points`, `pvs_leaf_list` (`missing`), `usercmd_replay`, `msec_budget`, `timeouts`, `changelevel`, `a2s_queries`, `oob_rate_limit`. В `wire.toml` — статусы опкодов, которые теперь шлёт сервер. `verified` — только пунктам, закрытым golden-тестом или строкой чек-листа. Run: `cargo xtask parity` → итог больше 0.
- [ ] **Step 7: README.** Раздел архитектуры: новые crates. Quick Start: запуск `--proto48` и вход оригинальным клиентом.
- [ ] **Step 8: Финальная проверка.** Run: `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace`; wasm-check со всеми новыми библиотечными crates → всё с кодом 0.
- [ ] **Step 9: Commit.** `git commit -m "feat(server): Protocol 48 server path, soak tests and phase 1 checklist"`

---

## Что вне этого плана

Перенос правил и оружия на edict, usermsg HUD, анимации игроков, точный список листов PVS, трассировки в `EngineApi` — фаза 2. Клиент на Protocol 48 и удаление `cs-net` — фаза 3. Загрузка файлов по каналу и HTTP, спреи — фаза 11.
