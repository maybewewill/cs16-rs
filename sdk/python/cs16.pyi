"""PEP 484 type stubs for the `cs16-rs` pure-Rust zero-compile Python 3 plugin API.

Place `sdk/python` on your editor's `python.analysis.extraPaths` (VS Code / Pylance) or
mark it as a Sources Root in PyCharm for full IntelliSense and static type checking.
"""

from typing import Any, Callable, Iterable, Iterator, Optional, Sequence, Tuple, TypeVar, Union

_F = TypeVar("_F", bound=Callable[..., Any])
Vec3 = Tuple[float, float, float]
Rgb = Union[Tuple[int, int, int], int]
Rgba = Union[Tuple[int, int, int, int], Tuple[int, int, int], int]


class Color:
    """Counter-Strike 1.6 chat color bytes and GoldSrc menu color escape sequences."""

    DEFAULT: str = "\x01"
    NORMAL: str = "\x01"
    YELLOW: str = "\x01"
    TEAM: str = "\x03"
    GREEN: str = "\x04"

    MENU_YELLOW: str = "\\y"
    MENU_WHITE: str = "\\w"
    MENU_RED: str = "\\r"
    MENU_GREY: str = "\\d"
    MENU_RIGHT: str = "\\R"


class HookAction:
    """ReAPI-style control-flow directives returned by `@mod.hook` and `@mod.chat_command`."""

    CONTINUE: int = 0
    HANDLED: int = 1
    OVERRIDE: int = 2
    SUPERCEDE: int = 3


class HookPhase:
    """Pre or Post hook execution phase."""

    PRE: int = 0
    POST: int = 1


class HookId:
    """ABI v2 hook identifiers accepted by `@mod.hook(...)` (or use snake_case string names)."""

    SERVER_ACTIVATE: int = 1
    SERVER_DEACTIVATE: int = 2
    SPAWN_ENTITY: int = 3
    CLIENT_CONNECT: int = 4
    CLIENT_PUT_IN_SERVER: int = 5
    CLIENT_DISCONNECT: int = 6
    CLIENT_COMMAND: int = 7
    CLIENT_USERINFO_CHANGED: int = 8
    RUN_USERCMD: int = 9
    POST_USERCMD: int = 10
    START_FRAME: int = 11
    THINK: int = 12
    TOUCH: int = 13
    BLOCKED: int = 14
    SERVER_COMMAND: int = 17
    PLAYER_DEATH: int = 30
    ROUND_START: int = 31
    PLAYER_SPAWN: int = 32
    PLAYER_TAKE_DAMAGE: int = 33
    PLAYER_KILLED: int = 34
    PLAYER_PRE_THINK: int = 35
    PLAYER_POST_THINK: int = 36
    PLAYER_JUMP: int = 37
    PLAYERS_SHOULD_COLLIDE: int = 38
    CAN_BUY_ITEM: int = 39
    CAN_HAVE_PLAYER_ITEM: int = 40
    WEAPON_DEPLOY: int = 41
    WEAPON_PRIMARY_ATTACK: int = 42
    WEAPON_RELOAD: int = 43
    PLAYER_SAY: int = 44
    MENU_SELECT: int = 45
    FREEZE_TIME_END: int = 46
    ROUND_END: int = 47
    BOMB_PLANTING: int = 48
    BOMB_PLANTED: int = 49
    BOMB_DEFUSING: int = 50
    BOMB_DEFUSED: int = 51
    BOMB_EXPLODED: int = 52
    HOSTAGE_TOUCHED: int = 53
    HOSTAGE_RESCUED: int = 54
    GRENADE_THROW: int = 55


class Armor:
    """Player armor type (`CS_ARMOR_*`)."""

    NONE: int = 0
    KEVLAR: int = 1
    VESTHELM: int = 2


class Team:
    """Counter-Strike 1.6 team identifiers (`CS_TEAM_*`)."""

    UNASSIGNED: int = 0
    TERRORIST: int = 1
    T: int = 1
    CT: int = 2
    COUNTER_TERRORIST: int = 2
    SPECTATOR: int = 3
    SPEC: int = 3


class WeaponId:
    """Vanilla CS 1.6 `WeaponIdType` numeric constants."""

    NONE: int = 0
    P228: int = 1
    GLOCK: int = 2
    SCOUT: int = 3
    HEGRENADE: int = 4
    XM1014: int = 5
    C4: int = 6
    MAC10: int = 7
    AUG: int = 8
    SMOKEGRENADE: int = 9
    ELITE: int = 10
    FIVESEVEN: int = 11
    UMP45: int = 12
    SG550: int = 13
    GALIL: int = 14
    FAMAS: int = 15
    USP: int = 16
    GLOCK18: int = 17
    AWP: int = 18
    MP5N: int = 19
    MP5NAVY: int = 19
    M249: int = 20
    M3: int = 21
    M4A1: int = 22
    TMP: int = 23
    G3SG1: int = 24
    FLASHBANG: int = 25
    DEAGLE: int = 26
    SG552: int = 27
    AK47: int = 28
    KNIFE: int = 29
    P90: int = 30
    SHIELD: int = 99
    SHIELDGUN: int = 99


class Button:
    """GoldSrc `IN_*` input button bitmask flags."""

    ATTACK: int = 1 << 0
    JUMP: int = 1 << 1
    DUCK: int = 1 << 2
    FORWARD: int = 1 << 3
    BACK: int = 1 << 4
    USE: int = 1 << 5
    CANCEL: int = 1 << 6
    LEFT: int = 1 << 7
    RIGHT: int = 1 << 8
    MOVELEFT: int = 1 << 9
    MOVERIGHT: int = 1 << 10
    ATTACK2: int = 1 << 11
    RUN: int = 1 << 12
    RELOAD: int = 1 << 13


class RenderFx:
    """GoldSrc entity `renderfx` constants."""

    NONE: int = 0
    PULSE_SLOW: int = 1
    PULSE_FAST: int = 2
    HOLOGRAM: int = 16
    GLOW_SHELL: int = 19


class RenderMode:
    """GoldSrc entity `rendermode` constants."""

    NORMAL: int = 0
    TRANS_COLOR: int = 1
    TRANS_TEXTURE: int = 2
    GLOW: int = 3
    TRANS_ALPHA: int = 4
    TRANS_ADD: int = 5


class CustomWeapon:
    """Declarative custom weapon archetype registered with `mod.register_custom_weapon(...)`."""

    id_name: str
    base_weapon: int
    display_name: str
    damage_mult: float
    cost: int

    def __init__(
        self,
        id_name: str,
        base_weapon: int = WeaponId.AK47,
        display_name: str = "",
        *,
        v_model: Optional[str] = None,
        p_model: Optional[str] = None,
        w_model: Optional[str] = None,
        fire_sound: Optional[str] = None,
        damage_mult: float = 1.0,
        clip_size: Optional[int] = None,
        deploy_speed_mult: float = 1.0,
        recoil_mult: float = 1.0,
        knockback: float = 1.0,
        cost: int = 0,
    ) -> None: ...


class Menu:
    """Host-managed paginated GoldSrc menu (`1..7`, `8. Back`, `9. More`, `0. Exit`)."""

    title: str
    per_page: int
    allow_exit: bool
    id: Optional[int]

    def __init__(
        self,
        title: str,
        callback: Optional[Callable[..., Any]] = None,
        *,
        per_page: int = 7,
        allow_exit: bool = True,
        back_label: Optional[str] = None,
        next_label: Optional[str] = None,
        exit_label: Optional[str] = None,
        number_color: str = "\\r",
        items: Optional[Sequence[Union[str, Tuple[str, str], Tuple[str, str, bool]]]] = None,
    ) -> None: ...

    def add_item(self, label: str, info: str = "", enabled: bool = True) -> "Menu": ...
    def add_blank(self) -> "Menu": ...
    def add_text(self, label: str) -> "Menu": ...
    def set_labels(
        self,
        *,
        back: Optional[str] = None,
        next: Optional[str] = None,
        exit: Optional[str] = None,
    ) -> "Menu": ...
    def show(self, player: Union["Player", int], page: int = 0, time: int = -1) -> int: ...
    def display(self, player: Union["Player", int], page: int = 0, time: int = -1) -> int: ...
    def destroy(self) -> None: ...


class Entity:
    """Handle to a server edict captured in the current engine snapshot."""

    index: int
    classname: str
    health: float
    flags: int
    origin: Vec3
    velocity: Vec3

    def remove(self) -> None: ...


class Player:
    """Handle to a CS 1.6 player slot (`0..31`) exposing state reads, mutations, i18n, and UI."""

    slot: int
    index: int
    ent_index: int
    active: bool
    name: str
    steamid: str
    ip: str
    ping: int
    loss: int
    health: float
    max_health: float
    armor: int
    armor_kind: int
    money: int
    frags: int
    deaths: int
    score: Tuple[int, int]
    team: int
    alive: bool
    is_alive: bool
    active_weapon: int
    clip: int
    bpammo: int
    has_defuser: bool
    has_nvg: bool
    admin_flags: int
    lang: str
    origin: Vec3
    velocity: Vec3
    aim_origin: Vec3
    flags: int
    on_ground: bool
    buttons: int
    gravity: float
    maxspeed: float
    godmode: bool
    noclip: bool
    footsteps: bool
    model: Optional[str]

    def __init__(self, slot: int) -> None: ...

    def tr(self, key: str, *args: Any, **kwargs: Any) -> str:
        """Translates `key` in this player's active language (`LangCatalog`)."""
        ...

    def has_flag(self, flag: str) -> bool:
        """Returns `True` if this player holds all specified AMXX RBAC flags (e.g. `'t'` for VIP)."""
        ...

    def set_lang(self, code: str) -> bool: ...
    def has_weapon(self, weapon_id: int) -> bool: ...
    def weapon_clip(self, weapon_id: int = 0) -> int: ...
    def bpammo_of(self, weapon_id: int = 0) -> int: ...

    def set_health(self, hp: float) -> None: ...
    def set_max_health(self, hp: float) -> None: ...
    def set_armor(self, amount: int, kind: int = Armor.VESTHELM) -> None: ...
    def set_money(self, amount: int, flash: bool = True) -> None: ...
    def add_money(self, delta: int, flash: bool = True) -> None: ...
    def set_score(self, frags: int, deaths: int) -> None: ...
    def set_team(self, team: int, model_class: Optional[int] = None) -> None: ...

    def give_item(self, item_name: str) -> None: ...
    def give_custom_weapon(self, id_name: str) -> None: ...
    def strip_weapons(self, keep_knife: bool = False) -> None: ...
    def set_weapon_clip(self, weapon_id: int, clip: int) -> None: ...
    def set_bpammo(self, weapon_id: int, amount: int) -> None: ...
    def set_defuser(self, has: bool = True) -> None: ...
    def set_nvg(self, has: bool = True) -> None: ...
    def drop_slot(self, inv_slot: int) -> None: ...

    def slap(self, damage: int = 0, random_dir: bool = True) -> None: ...
    def slay(self, preserve_frags: bool = False) -> None: ...
    def respawn(self) -> None: ...
    def damage(
        self, amount: float, attacker: Optional[Union["Player", int]] = None, bits: int = 0
    ) -> None: ...

    def set_godmode(self, enabled: bool = True) -> None: ...
    def set_noclip(self, enabled: bool = True) -> None: ...
    def set_maxspeed(self, speed: float) -> None: ...
    def set_gravity(self, gravity: float) -> None: ...
    def set_footsteps(self, silent: bool = True) -> None: ...
    def set_rendering(
        self,
        fx: int = RenderFx.NONE,
        rgb: Rgb = (255, 255, 255),
        mode: int = RenderMode.NORMAL,
        amount: int = 255,
    ) -> None: ...
    def set_model(self, model: str) -> None: ...
    def reset_model(self) -> None: ...
    def set_origin(self, origin: Vec3) -> None: ...
    def set_velocity(self, velocity: Vec3) -> None: ...

    def chat(self, text: str) -> None: ...
    def print(self, text: str) -> None: ...
    def center_print(self, text: str) -> None: ...
    def hud(
        self,
        text: str,
        x: float = -1.0,
        y: float = 0.35,
        *,
        channel: int = 1,
        rgba: Rgba = (255, 255, 255, 255),
        rgba2: Optional[Rgba] = None,
        effect: int = 0,
        fadein: float = 0.1,
        hold: float = 2.0,
        fadeout: float = 0.2,
        fxtime: float = 0.0,
    ) -> None: ...
    def dhud(
        self,
        text: str,
        x: float = -1.0,
        y: float = 0.25,
        *,
        rgb: Rgb = (255, 255, 255),
        effect: int = 0,
        fadein: float = 0.1,
        hold: float = 2.0,
        fadeout: float = 0.2,
        fxtime: float = 0.0,
    ) -> None: ...
    def show_motd(self, title: str, body: str) -> None: ...
    def screen_fade(
        self,
        duration: float = 0.5,
        hold: float = 0.1,
        flags: int = 0,
        rgba: Rgba = (0, 0, 0, 128),
    ) -> None: ...
    def screen_shake(
        self, amplitude: float = 4.0, duration: float = 1.0, frequency: float = 2.0
    ) -> None: ...
    def bar_time(self, seconds: int) -> None: ...
    def show_menu(self, menu: Menu, page: int = 0, time: int = -1) -> int: ...
    def emit_sound(self, sample: str, channel: int = 0, volume: float = 1.0) -> None: ...
    def kick(self, reason: str = "") -> None: ...


class Event:
    """Mutable hook event context delivered to `@mod.hook(...)` callbacks."""

    hook: int
    phase: int

    # Common fields populated according to hook type
    slot: int
    player: Player
    victim: Player
    attacker: Optional[Player]
    killer: Optional[Player]
    player_a: Player
    player_b: Player
    slot_a: int
    slot_b: int
    damage: float
    hitgroup: int
    weapon_id: int
    weapon_name: str
    damage_bits: int
    team_attack: bool
    headshot: bool
    team_kill: bool
    buttons: int
    oldbuttons: int
    should_collide: bool
    item_id: int
    price: int
    team_only: bool
    message: str
    menu_id: int
    item_index: int
    info: str
    round_num: int
    winner: int
    reason: int
    bonus: int
    origin: Vec3
    has_kit: bool
    ent_index: int
    command: str
    arg1: str
    args: str
    frametime: float
    map_name: str
    max_clients: int
    classname: str
    ent_a: int
    ent_b: int


class Vfx:
    """1-line Protocol 48 `TE_*` VFX builder (accessible as `vfx` or `mod.vfx`)."""

    def shockwave_ring(
        self,
        origin: Vec3,
        radius: float = 200.0,
        rgba: Rgba = (0, 200, 255, 255),
        life: int = 10,
        width: int = 16,
        *,
        slot: Optional[Union[Player, int]] = None,
    ) -> None: ...

    def laser_beam(
        self,
        start: Vec3,
        end: Vec3,
        rgba: Rgba = (255, 0, 0, 255),
        life: int = 10,
        width: int = 10,
        noise: int = 0,
        *,
        slot: Optional[Union[Player, int]] = None,
    ) -> None: ...

    def follow_trail(
        self,
        ent_index: Union[Player, Entity, int],
        rgba: Rgba = (0, 255, 0, 255),
        life: int = 15,
        width: int = 6,
        *,
        slot: Optional[Union[Player, int]] = None,
    ) -> None: ...

    def kill_beam(
        self,
        ent_index: Union[Player, Entity, int],
        *,
        slot: Optional[Union[Player, int]] = None,
    ) -> None: ...

    def aura_light(
        self,
        origin: Vec3,
        radius: int = 20,
        rgb: Rgb = (255, 215, 0),
        life: int = 10,
        decay: int = 15,
        *,
        slot: Optional[Union[Player, int]] = None,
    ) -> None: ...

    def sparks(
        self, origin: Vec3, *, slot: Optional[Union[Player, int]] = None
    ) -> None: ...

    def implosion(
        self,
        origin: Vec3,
        radius: int = 100,
        count: int = 20,
        life: int = 10,
        *,
        slot: Optional[Union[Player, int]] = None,
    ) -> None: ...

    def teleport_splash(
        self, origin: Vec3, *, slot: Optional[Union[Player, int]] = None
    ) -> None: ...

    def lava_splash(
        self, origin: Vec3, *, slot: Optional[Union[Player, int]] = None
    ) -> None: ...

    def explosion(
        self,
        origin: Vec3,
        scale: int = 30,
        flags: int = 0,
        *,
        slot: Optional[Union[Player, int]] = None,
    ) -> None: ...

    def blood_stream(
        self,
        origin: Vec3,
        direction: Vec3,
        color: int = 70,
        speed: int = 120,
        *,
        slot: Optional[Union[Player, int]] = None,
    ) -> None: ...

    def world_decal(
        self,
        origin: Vec3,
        decal_name: str,
        *,
        slot: Optional[Union[Player, int]] = None,
    ) -> None: ...


class Storage:
    """Per-plugin persistent key-value storage preserved across live hot-reloads."""

    def get(self, key: str, default: Any = None) -> Any: ...
    def set(self, key: str, value: Any) -> None: ...
    def delete(self, key: str) -> bool: ...
    def keys(self) -> list[str]: ...
    def items(self) -> list[Tuple[str, str]]: ...
    def __getitem__(self, key: str) -> str: ...
    def __setitem__(self, key: str, value: Any) -> None: ...
    def __delitem__(self, key: str) -> None: ...
    def __contains__(self, key: str) -> bool: ...
    def __len__(self) -> int: ...


class Leaderboard:
    """Shared O(log N) sorted leaderboard store (`/rank`, `/top15`, `csx`)."""

    def upsert(
        self, table: str, key: str, score: float, payload: Any = ""
    ) -> None: ...
    def rank(self, table: str, key: str) -> Optional[dict[str, Any]]: ...
    def top(
        self, table: str, offset: int = 0, limit: int = 15
    ) -> list[dict[str, Any]]: ...
    def count(self, table: str) -> int: ...
    def remove(self, table: str, key: str) -> bool: ...


class Mod:
    """Singleton plugin host interface (`from cs16 import mod`)."""

    vfx: Vfx
    storage: Storage
    leaderboard: Leaderboard
    id: str
    time: float
    frametime: float
    max_clients: int
    map_name: str

    def info(
        self,
        *,
        id: Optional[str] = None,
        name: Optional[str] = None,
        version: Optional[str] = None,
        author: Optional[str] = None,
        description: Optional[str] = None,
        permissions: Optional[Sequence[str]] = None,
        fuel_per_frame: Optional[int] = None,
    ) -> Callable[[_F], _F]: ...

    def hook(
        self,
        hook: Union[str, int],
        phase: Optional[Union[str, int]] = None,
        priority: int = 0,
        *,
        post: Optional[bool] = None,
    ) -> Callable[[_F], _F]: ...

    def chat_command(
        self, *commands: Union[str, Sequence[str]], suppress: bool = True
    ) -> Callable[[_F], _F]: ...

    def console_command(
        self, *commands: Union[str, Sequence[str]]
    ) -> Callable[[_F], _F]: ...

    def timer(
        self, interval: float, repeat: bool = True, arg: int = 0
    ) -> Callable[[_F], _F]: ...

    def export_fn(
        self, name: str, callback: Optional[Callable[..., Any]] = None
    ) -> Callable[[_F], _F]: ...

    def on_event(
        self, event_name: str, callback: Optional[Callable[..., Any]] = None
    ) -> Callable[[_F], _F]: ...

    def players(
        self, *, alive_only: bool = False, team: Optional[int] = None
    ) -> list[Player]: ...
    def all_players(self) -> list[Player]: ...
    def player(self, slot: int) -> Optional[Player]: ...
    def entity(self, index: int) -> Optional[Entity]: ...
    def entities(self, classname: Optional[str] = None) -> list[Entity]: ...
    def create_entity(self, classname: str) -> None: ...
    def remove_entity(self, index: int) -> None: ...

    def menu(
        self,
        title: str,
        callback: Optional[Callable[..., Any]] = None,
        *,
        per_page: int = 7,
        allow_exit: bool = True,
        back_label: Optional[str] = None,
        next_label: Optional[str] = None,
        exit_label: Optional[str] = None,
        number_color: str = "\\r",
    ) -> Menu: ...

    def chat_all(self, text: str) -> None: ...
    def print_all(self, text: str) -> None: ...
    def center_print(
        self, text: str, slot: Optional[Union[Player, int]] = None
    ) -> None: ...
    def hud(
        self,
        text: str,
        x: float = -1.0,
        y: float = 0.35,
        *,
        slot: Optional[Union[Player, int]] = None,
        channel: int = 1,
        rgba: Rgba = (255, 255, 255, 255),
        rgba2: Optional[Rgba] = None,
        effect: int = 0,
        fadein: float = 0.1,
        hold: float = 2.0,
        fadeout: float = 0.2,
        fxtime: float = 0.0,
    ) -> None: ...
    def dhud(
        self,
        text: str,
        x: float = -1.0,
        y: float = 0.25,
        *,
        slot: Optional[Union[Player, int]] = None,
        rgb: Rgb = (255, 255, 255),
        effect: int = 0,
        fadein: float = 0.1,
        hold: float = 2.0,
        fadeout: float = 0.2,
        fxtime: float = 0.0,
    ) -> None: ...
    def screen_fade(
        self,
        duration: float = 0.5,
        hold: float = 0.1,
        flags: int = 0,
        rgba: Rgba = (0, 0, 0, 128),
        *,
        slot: Optional[Union[Player, int]] = None,
    ) -> None: ...
    def screen_shake(
        self,
        amplitude: float = 4.0,
        duration: float = 1.0,
        frequency: float = 2.0,
        *,
        slot: Optional[Union[Player, int]] = None,
    ) -> None: ...

    def tr(
        self,
        key: str,
        *args: Any,
        slot: Optional[Union[Player, int]] = None,
        **kwargs: Any,
    ) -> str: ...
    def load_lang_toml(self, src: str) -> int: ...
    def load_lang_amxx(self, src: str) -> int: ...

    def cvar(self, name: str, default: str = "") -> str: ...
    def cvar_float(self, name: str, default: float = 0.0) -> float: ...
    def cvar_int(self, name: str, default: int = 0) -> int: ...
    def set_cvar(self, name: str, value: Any) -> None: ...

    def set_timer(
        self,
        delay_sec: float,
        callback: Callable[..., Any],
        repeat: bool = False,
        arg: int = 0,
    ) -> int: ...
    def cancel_timer(self, timer_id: int) -> None: ...

    def register_custom_weapon(self, spec: CustomWeapon) -> None: ...
    def call_fn(self, name: str, *args: Any) -> Any: ...
    def emit_event(self, event_name: str, payload: Any = "") -> None: ...
    def http_post_json(
        self,
        url: str,
        body: Union[str, dict[str, Any], list[Any]],
        callback: Optional[Callable[[int, str, Optional[str]], Any]] = None,
    ) -> bool: ...

    def precache_sound(self, sample: str) -> None: ...
    def emit_sound(
        self,
        ent_index: Union[Player, Entity, int],
        sample: str,
        channel: int = 0,
        volume: float = 1.0,
    ) -> None: ...
    def change_level(self, map_name: str) -> None: ...
    def server_exec(self, line: str) -> None: ...
    def trace_fraction(self, start: Vec3, end: Vec3) -> float: ...
    def log(self, *args: Any) -> None: ...


mod: Mod
vfx: Vfx
