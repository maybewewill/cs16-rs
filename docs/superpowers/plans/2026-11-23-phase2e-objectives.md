# Фаза 2E: цели карт (бомба, заложники, VIP, escape) и боты на движке — план

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Оригинальный клиент CS 1.6 играет полный раунд `de_dust2` (бомба) и `cs_office` (заложники) на нашем сервере против ботов; поведение целей карт совпадает с vanilla ReGameDLL и с эталонным HLDS.

**Architecture:**
- `cs-game` переносит из vanilla ReGameDLL:
  - C4 (`wpn_c4.cpp`) и заложенную бомбу (`ggrenade.cpp`: C4-части);
  - зоны и `info_map_parameters` (`triggers.cpp`, `mapinfo.cpp`);
  - заложников (`hostage/hostage.cpp`, `hostage_localnav.cpp`; классический путь, без CZ-improv);
  - VIP и escape;
  - ветки правил раунда и денег (`multiplay_gamerules.cpp`).
- Движок получает fake-клиентов так, как это сделано в ReHLDS (`SV_CreateFakeClient`, `RunPlayerMove`).
- Боты в `cs-game` — минимальные: команда `bot_add`, вход как у `AddBot`, мозг — `cs_sim::bots::think_bot`. Полный ZBot остаётся в фазе 12.
- Сверка ведётся с оригинальным клиентом и с эталонным HLDS (`cs-probe --diff`).

**Tech Stack:** Rust 2024, `cs-engine`, `cs-game`, `cs-proto48`, `cs-sim` (мозг ботов), `cs-probe`; источники в `scratchpad` (`wpnsrc`, `s1src`, `s2src`, `joinsrc`, `esrc`, `rev_es`); сводки — `survey-source-bomb.md`, `survey-source-hostage-bots.md`, `survey-code.md` в `.superpowers/sdd/2026-11-23-phase2e-objectives/`.

## Global Constraints

- Всё из `constraints.md` (раздел «Phase 2E»), vanilla-цель как в 2D: не `REGAMEDLL_FIXES`, не `REGAMEDLL_ADD`. Ветки `BUILD_LATEST` (иконка дефьюзера в табло, `HealthInfo`/`Account`) не переносим. Это решение плана; оно записано в ledger и проверяется по golden-захвату.
- Порт выполняется оператор за оператором, причуды сохраняются и подписываются в комментарии. Сохраняемые причуды:
  - `m_flBombRadius` передаётся как урон, радиус равен 3,5×;
  - бомба, заложенная в воде, удаляется без конца раунда;
  - `GiveC4` выбирает игрока по вращающемуся индексу;
  - рюкзак с бомбой не исчезает (`BombThink`);
  - сигнал зоны отстаёт на 0,5 с (`HandleSignals`).
- `GameEntity` расширяется только добавлением методов со значениями по умолчанию: старые энтити не меняются.
- Новые классы карт меняют нумерацию edict'ов. Номера `de_dust2` и `cs_office` сверяются с эталоном (baseline-тест `de_dust2_baselines_are_those_of_rehlds` и такой же тест для `cs_office`).
- Тесты, закрепившие поведение «без целей», переписываются по источнику, а не ослабляются:
  - `shield_and_defuser_are_not_for_sale_yet`;
  - `round_time_expiry_as_regamedll` (остаётся верным для карты без целей);
  - `round_rewards_as_regamedll`;
  - `two_teams_play_a_round` (de_dust2 теперь карта с бомбой).
- Боты: никакой ZBot-логики (навмеш, профили, чат). Это фаза 12. Fake-клиенты движка — точный перенос ReHLDS.
- Legacy-код (`cs-sim` `MatchWorld`, `cs-net`, `cs-client`) остаётся зелёным. `cs_sim::bots` переиспользуется как библиотека и не меняет поведения старых тестов.

## Review Focus

1. Порядок и условия по источнику:
   - `CheckWinConditions` и проверки в нём;
   - деньги за каждый исход;
   - сообщения конца раунда;
   - тайминги бомбы (beep-волны, 3 с установки, 5/10 с разминирования, 0,12 с think, `mp_c4timer` 10..90).
2. Целостность энтити: `use` и `damage` не ломают игроков; `UTIL_RestartOther` перезапускает заложников и зоны; сброс состояния на рестарте, смерти, дисконнекте и смене карты.
3. Fake-клиенты:
   - не шлют пакеты и не таймаутятся;
   - не кикаются по backlog'у;
   - unlag, события и movevars пропускают их как в ReHLDS;
   - смена уровня как в `SV_InactivateClients`.
4. Сообщения на проводе: `BombDrop`, `BombPickup`, `BarTime`/`BarTime2`, `Scenario`, `StatusIcon` (`c4`, `defuser`, `rescue`, `escape`, `vipsafety`), биты `ScoreAttrib`, `HostagePos`/`HostageK`, `TextMsg` и `SendAudio`. Порядок сверяется с golden-захватом.

## Порядок и волны

| Волна | Задачи (параллельно) |
|---|---|
| W1 | T1 энтити `use`/`damage` и `PlayerUse`; T2 fake-клиенты движка; T3 энтити зон, `info_map_parameters`, флаги карты, сигналы и иконки |
| W2 | T4 оружие C4 и заложенная бомба; T5 носка, выброс и выдача C4, дефьюзер, табло и радар; T6 заложники; T8 боты на движке |
| W3 | T7 правила раунда и деньги для всех целей; T9 VIP и escape |
| W4 | T10 боты и цели (минимум), полный раунд против эталона; T11 документы, реестр, чек-лист |

Каждая задача — в своём worktree от базы, которую называет диспетчер. Ревью идёт по задаче, слияние делает контроллер.

---

### Task 1: `cs-game`: `use`, урон по энтити, `PlayerUse`, `SUB_UseTargets`

**Files:**
- Modify: `crates/cs-game/src/entity.rs`, `player.rs` (только `PlayerUse`), `bullets.rs` (диспетчер `TraceAttack`), `grenade.rs` (`radius_damage`: по всем энтити с `takedamage`).
- Create: `crates/cs-game/src/targets.rs` (`SUB_UseTargets`, `FireTargets`, `killtarget`, `delay` — в объёме, который нужен целям), `tests/entity_use_damage.rs`.

Перенести:
- **В `GameEntity` по источнику:**
  - `use_(…, activator, caller, use_type, value)`;
  - `trace_attack`, `take_damage`, `killed`;
  - `object_caps()` (`FCAP_IMPULSE_USE`, `FCAP_CONTINUOUS_USE`, `FCAP_ONOFF_USE`);
  - `is_alive`, `body_target`.

  Значения по умолчанию — как в `CBaseEntity`.
- **Бой:**
  - `CBasePlayer::PlayerUse` (`player.cpp`): поиск в сфере 64, точка-в-конус, `m_afButtonPressed` и `IN_USE`, звук `common/wpn_select`/`wpn_denyselect` по источнику;
  - `TraceAttack` пули отдаётся энтити, в которую попали, а не только игроку;
  - `RadiusDamage`/`RadiusDamage2` наносят урон любой энтити с `takedamage` (`combat.cpp`).
- **`SUB_UseTargets`/`FireTargets`** по `target`: энтити `info_target` получает `use`, но ничего не делает.

- [ ] **Step 1: Тесты:**
  - `use_key_uses_the_entity_in_front`;
  - `continuous_use_repeats_while_held`;
  - `a_bullet_damages_a_damageable_entity`;
  - `radius_damage_reaches_entities`;
  - `use_targets_fires_by_targetname`;
  - `players_are_unchanged` (старые тесты урона игрока проходят как есть).
- [ ] **Step 2–5:** реализация, тесты, clippy → Commit `feat(game): entity use and damage, PlayerUse, SUB_UseTargets`

### Task 2: `cs-engine`: fake-клиенты (`SV_CreateFakeClient`, `RunPlayerMove`)

**Files:**
- Modify: `crates/cs-engine/src/{client,lifecycle,server,usercmd,frame,unlag,events,api_*}.rs`.
- Create: `crates/cs-engine/tests/fake_clients.rs`.

Перенести (ReHLDS `pr_cmds.cpp`, `sv_main.cpp`, `sv_user.cpp`, список мест в `survey-source-hostage-bots.md` §2.1):
- `EngineApi::create_fake_client(name) -> Option<u8>`:
  - свободный слот, явный флаг `fake` (без адреса и netchan);
  - `FL_FAKECLIENT | FL_CLIENT` на edict'е;
  - userinfo с `name`, `*bot 1`;
  - `ClientConnect` не зовётся, как в ReHLDS: игра сама вызывает `put_in_server`.
- `EngineApi::run_player_move(slot, viewangles, forward, side, up, buttons, impulse, msec)`: строит `usercmd` и прогоняет `run_player_cmd` без `setup_move`/`restore_move` и без movevars, как `SV_RunCmd` у fake-клиента.
- Пропуски для fake-клиентов:
  - не строить и не слать пакеты, не таймаутить, не кикать по reliable backlog;
  - очередь `MsgDest::One` для fake-клиента отбрасывается;
  - события с `FEV_NOTHOST` и прочие ветки `fakeclient` — по источнику;
  - `SV_InactivateClients` при смене уровня.
- `kick` бота, `status` показывает `BOT`.

- [ ] **Step 1: Тесты:**
  - `a_fake_client_takes_a_slot_without_a_peer`;
  - `run_player_move_moves_the_player`;
  - `fake_clients_are_never_sent_or_timed_out`;
  - `fake_clients_skip_unlag_and_events`;
  - `level_change_drops_fake_clients_as_rehlds`;
  - `real_clients_unchanged`.
- [ ] **Step 2–5:** … → Commit `feat(engine): fake clients and RunPlayerMove as ReHLDS`

### Task 3: `cs-game`: зоны целей, `info_map_parameters`, флаги карты, сигналы и иконки

**Files:**
- Modify: `crates/cs-game/src/{entity,world,buy,rules,player,messages}.rs` (в `rules.rs` только флаги карты и `CheckMapConditions`).
- Create: `crates/cs-game/src/zones.rs`, `tests/zones.rs`.

Перенести:
- **Энтити зон:**
  - `func_bomb_target` (`BombTargetTouch`, `BombTargetUse` + `SUB_UseTargets` из T1) и `info_bomb_target`;
  - `func_hostage_rescue` и `info_hostage_rescue`;
  - `func_escapezone` (`EscapeZoneTouch`, `Disappear`);
  - `func_vip_safetyzone`, `info_vip_start`;
  - `info_map_parameters` (`CMapInfo`: `buying`, `bombradius` ≤ 2048, `CheckMapInfo` на активации и на каждом рестарте).
- **Флаги карты:** `CheckMapConditions`:
  - `m_bMapHasBombTarget`, `m_bMapHasBombZone`;
  - `m_bMapHasRescueZone`, `m_bMapHasEscapeZone`, `m_bMapHasVIPSafetyZone`;
  - `m_bCTCantBuy`/`m_bTCantBuy` и их чтение в `CanPlayerBuy`.
- **Сигналы и иконки:**
  - биты `SIGNAL_BOMB`, `RESCUE`, `ESCAPE`, `VIPSAFETY`;
  - `HandleSignals` целиком, с иконками `c4` (для носителя бомбы), `rescue`, `escape`, `vipsafety` и `Disappear`;
  - запасные `OLD_CheckBombTarget`/`OLD_CheckRescueZone` для карт без зон;
  - бит `iuser3` `IN_BOMB_ZONE`.

- [ ] **Step 1: Тесты:**
  - `zones_raise_their_signals_and_icons`;
  - `map_flags_as_check_map_conditions`;
  - `map_parameters_buying_and_bombradius`;
  - `bomb_target_use_fires_targets`;
  - `old_zone_fallbacks_without_entities`;
  - `cs_office_edict_numbers_as_hlds` (сверка с эталоном; номер карты и эталонные числа контроллер даёт в брифе).
- [ ] **Step 2–5:** … → Commit `feat(game): objective zones, map parameters and zone signals`

### Task 4: `cs-game`: оружие C4 и заложенная бомба

**Files:**
- Create: `crates/cs-game/src/weapons/c4.rs` (вместо заглушки в `grenades.rs`), `tests/bomb.rs`.
- Modify: `grenade.rs` (C4-части `CGrenade`).
- Create: `crates/cs-game/src/bomb.rs`, если удобнее держать заложенную бомбу отдельно от гранат.
- Modify: `cs-proto48/src/usermsg.rs` (`BarTime2`, `Scenario`).

Перенести:
- **`wpn_c4.cpp`:**
  - `Spawn`, `Deploy`, `Holster`;
  - `PrimaryAttack`: условия установки, `m_bStartedArming`, `BarTime`, 3 с, анимации, звуки, отмена;
  - `WeaponIdle`.
- **`ggrenade.cpp`:**
  - `ShootSatchelCharge`;
  - `C4Think`: beep-волны, LED-спрайт и его TE, `Scenario 0`;
  - `C4Touch`;
  - `Use`/`DefuseBombStart`/`DefuseBombEnd`: 5/10 с, `m_fNextDefuse` = now + 0,5, отмена, `BarTime`;
  - `Detonate2`/`Explode2`: `RadiusDamage2` с радиусом 3,5 × `m_flBombRadius`, TE, звуки, `m_bTargetBombed`;
  - удаление бомбы в воде.
- **Вызовы правил:** через крючки `rules::bomb_planted(g, eng, planter)`, `rules::bomb_exploded(g, eng)`, `rules::bomb_defused(g, eng, defuser)`. Их тела делает T7; в T4 они — однострочные функции в `rules.rs` с doc «T7 fills it».

- [ ] **Step 1: Тесты:**
  - `planting_takes_three_seconds_in_a_bomb_zone`;
  - `moving_cancels_the_plant`;
  - `c4_beeps_follow_the_source_schedule`;
  - `defuse_takes_ten_or_five_seconds`;
  - `defuse_is_cancelled_by_leaving`;
  - `bomb_explodes_with_radius_damage2`;
  - `a_bomb_in_water_is_removed_silently`.
- [ ] **Step 2–5:** … → Commit `feat(game): the C4 and the planted bomb as ReGameDLL`

### Task 5: `cs-game`: носка, выброс и выдача C4, дефьюзер, табло и радар

**Files:**
- Modify: `crates/cs-game/src/{inventory,buy,player,messages,score,hud_misc}.rs`.
- Create: `tests/bomb_carrier.rs`.

Перенести:
- **Носка C4:**
  - `m_bHasC4`;
  - `ScoreAttrib`: бит бомбы, `SetScoreAttrib`/`SetScoreboardAttributes`;
  - иконка `c4`, мигающая в зоне (вместе с T3).
- **Выброс и подбор:**
  - выброс C4 (`DropPlayerItem` с C4: `BombDrop`, `#Game_bomb_drop`, флаг сброшенной бомбы в правилах);
  - рюкзак-`weaponbox` с C4 (`BombThink`: `BombDrop` раз в секунду, не исчезает);
  - подбор (`BombPickup`, `#Game_bomb_pickup`, только для T);
  - `PackDeadPlayerItems` с C4;
  - радарная вспышка.
- **Выдача:** функция `give_c4(g, eng, slot)` для `GiveC4` (выбор игрока делает T7).
- **Дефьюзер:**
  - `item_thighpack`;
  - покупка `defuser`: только CT, только на картах с бомбой, 200; заменяет тест-заглушку;
  - `m_bHasDefuser`, `pev->body`, иконка `defuser`, бит `ItemStatus`;
  - выброс при смерти, сброс на спавне.
- **InitHUD:** `BombDrop`/`ShowTimer` для сброшенной или заложенной бомбы по источнику.

- [ ] **Step 1: Тесты:**
  - `the_carrier_has_the_bomb_attrib_and_icon`;
  - `dropping_the_bomb_sends_bomb_drop`;
  - `a_dead_carrier_drops_the_backpack`;
  - `only_terrorists_pick_up_the_bomb`;
  - `defuser_purchase_as_regamedll`;
  - `defuser_drops_at_death`;
  - `init_hud_shows_a_dropped_bomb`.
- [ ] **Step 2–5:** … → Commit `feat(game): carrying, dropping and giving the C4, the defuser`

### Task 6: `cs-game`: заложники (классический `CHostage` + `CLocalNav`)

**Files:**
- Create: `crates/cs-game/src/hostage.rs`, `hostage_localnav.rs`, `tests/hostages.rs`.
- Modify: `entity.rs` (регистрация `hostage_entity`), `world.rs` (прекеш по источнику).

Перенести:
- **`CHostage`:**
  - `Spawn`, `Precache` (модель из ключа или `scientist.mdl`), `MOVETYPE_STEP`;
  - `IdleThink`: 33 мс и полный think 0,1 с;
  - `Use`: следовать или стоять, порог 1 с, сообщения `#Hostage_*`, звуки;
  - `Touch`: толчок;
  - `DoFollow`/`MoveToward`: ступени и прыжки, `walk_move`/`move_to_origin` движка;
  - `TakeDamage`/`Killed`: штраф, `#Killed_Hostage`, `HostageK`, `mp_hostagepenalty`;
  - спасение: `m_bRescueMe`, `GiveCTTouchBonus`;
  - `Restart`/`RePosition`;
  - `HostagePos` и радар.
- **`CLocalNav`:** решётка 26, до 100 узлов, `PathTraversable`, ступень 18, прыжок 40, глобальный бюджет узлов.
- **Хуки правил:** `rules::hostage_rescued`/`hostage_killed` — однострочные функции в `rules.rs`, их тела делает T7.

- [ ] **Step 1: Тесты:**
  - `use_toggles_follow_with_the_one_second_gate`;
  - `a_hostage_follows_its_leader_around_a_corner`;
  - `hostage_touch_pushes`;
  - `killing_a_hostage_costs_money_and_sends_hostage_k`;
  - `rescue_in_the_zone_pays_the_rescuer`;
  - `hostages_restart_to_their_spawn`;
  - `cs_office_hostages_spawn_as_hlds`.
- [ ] **Step 2–5:** … → Commit `feat(game): hostages and local navigation as ReGameDLL`

### Task 8: `cs-game`: боты на движке (минимальные)

**Files:**
- Create: `crates/cs-game/src/bots.rs`, `tests/bots.rs`.
- Modify: `lib.rs` (команды сервера), `teams.rs` (ветки `IsBot`), `cs-server` (`--bots`/`bot_quota` в режиме proto48).

Перенести и сделать:
- `bot_add [t|ct]`, `bot_kick`, `bot_quota` как в `AddBot`: `create_fake_client`, `put_in_server`, `*bot 1`, `HandleMenu_ChooseTeam`/`ChooseAppearance`.
- Ветки `IsBot()` в правилах: без меню и без кика по простою.
- Think 30/10 Гц: наблюдения через трассы движка, решение — `cs_sim::bots::think_bot` и `NavGraph`, ход — через `run_player_move`.
- Закупка фиксированным списком через алиасы покупки в начале раунда.

- [ ] **Step 1: Тесты:**
  - `bot_add_joins_a_team_and_spawns`;
  - `bots_move_and_shoot_an_enemy`;
  - `bots_buy_at_round_start`;
  - `bot_kick_frees_the_slot`;
  - `a_round_with_bots_ends`.
- [ ] **Step 2–5:** … → Commit `feat(game): bots on the engine`

### Task 7: `cs-game`: правила раунда и деньги для всех целей

**Files:**
- Modify: `crates/cs-game/src/{rules,money,messages}.rs`.
- Create: `tests/objective_rounds.rs`.

Перенести:
- **`CheckWinConditions`** в порядке источника, со всеми проверками:
  - бомба;
  - заложники: все мертвы или спасены и спасено не меньше половины;
  - VIP;
  - escape.
- **Конец раунда:** `TerminateRound`/`EndRoundMessage`:
  - `#Target_Bombed`, `#Bomb_Defused`, `#Target_Saved`;
  - `#All_Hostages_Rescued`, `#Hostages_Not_Rescued`;
  - `#VIP_*`, `#Escaped`/`#Not_Escaped`;
  - `SendAudio`.
- **Истечение времени:** `CheckRoundTimeExpired` в порядке бомба → заложники → escape → VIP.
- **Деньги:**
  - таблица 3250/3500 по источнику;
  - бонусы за закладку и разминирование;
  - серия поражений;
  - 150 за живого заложника, не больше 2000;
  - 750 за спасённого.
- **`RestartRound`, части с целями:**
  - `GiveC4` с вращающимся `m_iC4Guy`;
  - `UTIL_RestartOther("hostage_entity")`;
  - выбор следующего VIP (вызов из T9);
  - сброс флагов;
  - `BombPickup`/`ShowTimer`.
- **Таймер:** `mp_c4timer` 10..90; `RoundTime`/`ShowTimer` при заложенной бомбе.
- **Тела крючков** из T4 и T6.

- [ ] **Step 1: Тесты:**
  - `bomb_exploded_wins_for_terrorists`;
  - `bomb_defused_wins_for_cts`;
  - `time_up_on_a_bomb_map_is_target_saved`;
  - `all_hostages_rescued_wins_for_cts`;
  - `time_up_on_a_hostage_map_wins_for_terrorists`;
  - `objective_money_as_regamedll`;
  - `give_c4_rotates`;
  - `round_rewards_on_a_bomb_map` (вместо старого 3000).
- [ ] **Step 2–5:** … → Commit `feat(game): objective round rules and money as ReGameDLL`

### Task 9: `cs-game`: VIP и escape

**Files:**
- Modify: `crates/cs-game/src/{player,teams,buy,inventory}.rs`.
- Create: `crates/cs-game/src/vip.rs`, `tests/vip_escape.rs`.

Перенести:
- **VIP:**
  - выбор VIP: очередь, `m_pVIP`, `m_iConsecutiveVIP` (вызывается из `RestartRound` T7);
  - модель `vip`, броня 200, скорость 227, USP;
  - запрет покупки и ограничения оружия на `as_`;
  - пункт меню команды `VIP`;
  - половинный коэффициент брони;
  - бит `ScoreAttrib` VIP;
  - `#Hint_reward_for_killing_vip` и награда за убийство VIP.
- **Escape:**
  - коэффициент побега;
  - стартовые деньги и радио;
  - `CArmoury::Restart` на escape-картах.
- **Проверка на карте:** `as_oilrig`, если она установлена.

- [ ] **Step 1: Тесты:**
  - `a_vip_is_picked_and_equipped`;
  - `the_vip_cannot_buy`;
  - `vip_escape_ends_the_round`;
  - `vip_killed_rewards_the_killer`;
  - `escape_ratio_ends_the_round`.
- [ ] **Step 2–5:** … → Commit `feat(game): VIP and escape as ReGameDLL`

### Task 10: боты и цели (минимум), полный раунд против эталона

**Files:** `crates/cs-game/src/bots.rs`, `tests/full_round.rs`; сверка — `cs-probe --diff`.

Сделать:
- Простое поведение у целей, чтобы раунд мог закончиться не только убийствами:
  - T-бот с бомбой идёт к зоне и ставит;
  - CT-бот идёт к заложенной бомбе и разминирует;
  - CT-бот ведёт ближайшего заложника к зоне.

  Поведение на `NavGraph`, без ZBot.
- Golden-захват HLDS с теми же действиями: закладка, взрыв, разминирование, если сценарий воспроизводим зондом. Diff потока сообщений целей.

- [ ] **Step 1: Тесты:**
  - `a_bot_plants_and_the_bomb_explodes_on_de_dust2`;
  - `a_bot_defuses_a_planted_bomb`;
  - `a_bot_rescues_a_hostage_on_cs_office`.

  Тесты на реальных картах, если игра установлена.
- [ ] **Step 2–5:** … → Commit `feat(game): bots go for the objectives`

### Task 11: документы, реестр, чек-лист

**Files:**
- Modify: `docs/parity/{entities,weapons,usermsg,tempents,engine,cvars}.toml`.
- Create: `docs/parity/bots.toml` (вес 4 уже есть), `docs/parity/checklists/phase2e-objectives.md`.
- Modify: `THIRD-PARTY-NOTICES.md`, `docs/LICENSING.md`, `README.md`.

Чек-лист владельца — оригинальный клиент:
- `de_dust2`: закладка, beep, взрыв, разминирование с китом и без, сброс и подбор бомбы, радар;
- `cs_office`: заложники идут за CT, спасение, убийство заложника;
- `as_oilrig`: VIP;
- деньги за каждый исход;
- боты.

Пометки «From memory - confirm» ставятся там, где ожидание взято не из источника.
