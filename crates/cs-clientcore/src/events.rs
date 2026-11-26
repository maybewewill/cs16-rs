//! `svc_event` and `svc_event_reliable`: the client's event queue, and what the Counter-Strike
//! weapon events (`events/*.sc`) do when they fire, as data for the shell: the view-model
//! animation, the ejected shell, the sound, and the bullets whose impacts the shell traces.
//!
//! Engine side, ported from Xash3D FWGS `engine/client/cl_events.c` (GPL-3.0-or-later), read
//! 2026-11-23: `CL_ParseEvent` (the invoker is the entity at `packet_index` of the update just
//! received: it supplies `entindex`, the origin and angles when the event has none, a player's
//! pitch is multiplied by 3 and negated to undo the server's `-1/3` packing, `ducking` is
//! `usehull == 1`), `CL_ParseReliableEvent`, `CL_QueueEvent` (64 slots, a reliable event may take
//! the slot of an unreliable one), `CL_FireEvents` (slots in order, a delayed one waits for
//! `cl.time`), `CL_FireEvent`, `CL_PlaybackEvent` (flags of a client event), and
//! `CL_FindEmptyEvent`. A deviation: Xash3D reuses one `args` across the events of a message,
//! so an event with a packet index but no argument delta inherits the previous event's
//! arguments; the stock client zeroes them (`nullargs`), and so does this port.
//!
//! Handlers: the 30 weapon events of cs16-client `cl_dll/events/event_*.cpp` (GPL-2.0-or-later,
//! read 2026-11-23), whose per-weapon constants - the sounds, the animation numbers, the shell
//! offsets, the bullet type and the penetration - are the table [`WEAPONS`]. The helpers those
//! files call live in Valve-licensed files that may not be ported, so the shell's placement and
//! ejection and the gun position are our own clean-room design (`gun_position`, `eject_shell`). The impacts of the bullets (decals, smoke,
//! penetration) need a trace against the map and the players, which is the shell's: [`BulletFire`] carries the call's arguments.

use std::collections::BTreeMap;

use cs_proto48::delta::GoldSrcEntityState;
use cs_proto48::event::EventArgs;
use cs_proto48::messages::SvcMessage;
use glam::Vec3;

use crate::precache::PrecacheTables;
use crate::sound::{ATTN_NORM, PITCH_NORM, SoundCommand, SoundStart, chan};
use crate::tempents::{FxRandom, FxWorld, te_bounce};

/// Slots of the queue (`MAX_EVENT_QUEUE`).
pub const MAX_EVENT_QUEUE: usize = 64;

/// `FEV_*` (`common/event_flags.h`).
pub mod fev {
    pub const NOTHOST: u32 = 1 << 0;
    pub const RELIABLE: u32 = 1 << 1;
    pub const GLOBAL: u32 = 1 << 2;
    pub const UPDATE: u32 = 1 << 3;
    pub const HOSTONLY: u32 = 1 << 4;
    pub const SERVER: u32 = 1 << 5;
    pub const CLIENT: u32 = 1 << 6;
}

/// The view height of a standing player (Xash3D FWGS `cl_tent.c` `CL_UpdateFlashlight`: 28;
/// 12 when ducking).
pub const DEFAULT_VIEWHEIGHT: f32 = 28.0;

/// What the effect modules need to know about the connection.
#[derive(Clone, Copy)]
pub struct FxEnv<'a> {
    /// Client time now (`cl.time`).
    pub time: f64,
    pub precache: &'a PrecacheTables,
    /// The entities of the latest `svc_packetentities`, by number: the update the events of a
    /// message refer to by position.
    pub entities: &'a BTreeMap<u16, GoldSrcEntityState>,
    pub max_clients: u16,
    /// The entity number of the local player (slot + 1); 0 when not known.
    pub local_player: u16,
    /// `clientdata.origin` of the local player (used when `Player_Encode` omits local origin).
    pub local_origin: Vec3,
    /// `clientdata.view_ofs` of the local player (the predicted view height).
    pub local_view_ofs: Vec3,
    /// `clientdata.velocity` of the local player.
    pub local_velocity: Vec3,
    /// The weapon state bits of the local player's current weapon (`g_iWeaponFlags`, from
    /// `weapon_data_t::m_iWeaponState`): a Glock in burst mode.
    pub weapon_state: i32,
    /// Whether the local player holds a shield (`g_bHoldingShield`).
    pub holding_shield: bool,
    /// The `cl_righthand` cvar.
    pub right_hand: bool,
    /// Whether client-side movement prediction (`!cl_nopred`) is active for local footsteps.
    pub prediction_active: bool,
    pub world: &'a dyn FxWorld,
}

impl<'a> FxEnv<'a> {
    /// An environment with no local player, no shield and nothing in the world.
    pub fn new(
        time: f64,
        precache: &'a PrecacheTables,
        entities: &'a BTreeMap<u16, GoldSrcEntityState>,
        world: &'a dyn FxWorld,
    ) -> Self {
        Self {
            time,
            precache,
            entities,
            max_clients: 32,
            local_player: 0,
            local_origin: Vec3::ZERO,
            local_view_ofs: Vec3::new(0.0, 0.0, DEFAULT_VIEWHEIGHT),
            local_velocity: Vec3::ZERO,
            weapon_state: 0,
            holding_shield: false,
            right_hand: false,
            prediction_active: true,
            world,
        }
    }

    fn is_player(&self, entity: i32) -> bool {
        entity > 0 && entity <= i32::from(self.max_clients)
    }

    fn is_local(&self, entity: i32) -> bool {
        self.local_player != 0 && entity == i32::from(self.local_player)
    }
}

/// An event waiting for its time (`event_info_t`).
#[derive(Debug, Clone, PartialEq)]
pub struct QueuedEvent {
    /// The index in the event precache table; never 0.
    pub index: u16,
    /// `FEV_*`.
    pub flags: u32,
    /// Client time to fire at; 0 fires at once.
    pub fire_time: f64,
    pub args: EventArgs,
    /// `event_args_t::velocity`, which is not on the wire.
    pub velocity: Vec3,
}

/// The queue of events (`cl.events`).
#[derive(Debug, Clone)]
pub struct EventQueue {
    slots: Vec<Option<QueuedEvent>>,
    /// Events dropped because the queue was full of reliable ones.
    pub dropped: u32,
}

impl Default for EventQueue {
    fn default() -> Self {
        Self {
            slots: vec![None; MAX_EVENT_QUEUE],
            dropped: 0,
        }
    }
}

impl EventQueue {
    /// How many events wait.
    pub fn len(&self) -> usize {
        self.slots.iter().flatten().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&mut self) {
        self.slots.iter_mut().for_each(|s| *s = None);
    }

    /// `CL_QueueEvent`: the first empty slot; a reliable event takes the first slot that does
    /// not hold a reliable one when there is none empty.
    pub fn queue(
        &mut self,
        flags: u32,
        index: u16,
        delay: f32,
        time: f64,
        args: EventArgs,
        velocity: Vec3,
    ) {
        // A slot with index 0 is an empty one in the original, so such an event cannot exist.
        if index == 0 {
            return;
        }
        let mut slot = self.slots.iter().position(Option::is_none);
        if slot.is_none() && flags & fev::RELIABLE != 0 {
            slot = self
                .slots
                .iter()
                .position(|s| s.as_ref().is_some_and(|e| e.flags & fev::RELIABLE == 0));
        }
        let Some(slot) = slot else {
            self.dropped = self.dropped.saturating_add(1);
            return;
        };
        self.slots[slot] = Some(QueuedEvent {
            index,
            flags,
            fire_time: if delay != 0.0 {
                time + f64::from(delay)
            } else {
                0.0
            },
            args,
            velocity,
        });
    }

    /// Queues a locally predicted weapon event to fire immediately on the next `update`.
    pub fn push_local(&mut self, index: u16, args: EventArgs) {
        self.queue(fev::CLIENT, index, 0.0, 0.0, args, Vec3::ZERO);
    }

    /// `CL_PlaybackEvent`: an event the client itself plays (a predicted weapon). Events the
    /// server owns (`FEV_SERVER`) are ignored, as are indices the server never precached.
    #[allow(clippy::too_many_arguments)]
    pub fn playback(
        &mut self,
        flags: u32,
        index: u16,
        delay: f32,
        mut args: EventArgs,
        env: &FxEnv,
    ) {
        if flags & fev::SERVER != 0 || env.precache.event(index).is_none() {
            return;
        }
        let flags = (flags | fev::CLIENT) & !(fev::NOTHOST | fev::HOSTONLY | fev::GLOBAL);
        args.entindex = i32::from(env.local_player);
        args.ducking = i32::from(env.local_view_ofs.z < DEFAULT_VIEWHEIGHT - 1.0);
        self.queue(
            flags,
            index,
            delay.max(0.0),
            env.time,
            args,
            env.local_velocity,
        );
    }

    /// Reacts to a parsed `svc_event` or `svc_event_reliable`; other messages are ignored.
    pub fn handle(&mut self, msg: &SvcMessage, env: &FxEnv) {
        match msg {
            SvcMessage::Event(events) => {
                for ev in events {
                    // Without a position in the update there is no invoker, and the original
                    // does not queue the event at all.
                    let Some(position) = ev.packet_index else {
                        continue;
                    };
                    let delay = ev.fire_time.map_or(0.0, |t| f32::from(t) / 100.0);
                    let mut args = ev.args.unwrap_or_default();
                    let mut velocity = Vec3::ZERO;
                    match env.entities.values().nth(usize::from(position)) {
                        Some(state) => {
                            let number = i32::from(state.number);
                            args.entindex = number;
                            if args.origin == Vec3::ZERO {
                                args.origin = if state.origin == Vec3::ZERO && env.is_local(number)
                                {
                                    env.local_origin
                                } else {
                                    state.origin
                                };
                            }
                            if args.angles == Vec3::ZERO {
                                args.angles = state.angles;
                            }
                            args.angles = normalize_angles(args.angles);
                            if env.is_player(number) {
                                args.angles.x *= -3.0;
                                // The remote players' velocity is not in the states this client
                                // keeps (assumption: zero); the local one is `clientdata`'s.
                                if env.is_local(number) {
                                    velocity = env.local_velocity;
                                }
                                args.ducking = i32::from(state.usehull == 1);
                            }
                        }
                        None => {
                            // The invoker is not in the update: the angles keep the packing,
                            // undone the other way.
                            if env.is_player(args.entindex) {
                                args.angles.x /= -3.0;
                            }
                        }
                    }
                    self.queue(fev::SERVER, ev.index, delay, env.time, args, velocity);
                }
            }
            SvcMessage::EventReliable {
                index,
                args,
                fire_time,
            } => {
                let mut args = *args;
                if env.is_player(args.entindex) {
                    args.angles.x *= -3.0;
                }
                let delay = fire_time.map_or(0.0, |t| f32::from(t) / 100.0);
                self.queue(
                    fev::RELIABLE | fev::SERVER,
                    *index,
                    delay,
                    env.time,
                    args,
                    Vec3::ZERO,
                );
            }
            _ => {}
        }
    }

    /// `CL_FireEvents`: fires the events whose time has come, in slot order, and empties their
    /// slots.
    pub fn fire_due(&mut self, env: &FxEnv, rng: &mut FxRandom) -> Vec<FiredEvent> {
        let mut fired = Vec::new();
        for slot in &mut self.slots {
            let Some(ev) = slot else {
                continue;
            };
            if ev.fire_time != 0.0 && ev.fire_time > env.time {
                continue;
            }
            let ev = slot.take().expect("the slot was just matched");
            let name = env.precache.event(ev.index).map(str::to_string);
            let fx = match name.as_deref() {
                Some(name) => resolve(name, &ev, env, rng),
                None => EventFx::Unhandled,
            };
            fired.push(FiredEvent {
                index: ev.index,
                name,
                flags: ev.flags,
                args: ev.args,
                velocity: ev.velocity,
                fx,
            });
        }
        fired
    }
}

/// `COM_NormalizeAngles`: each angle into -180 to 180.
fn normalize_angles(angles: Vec3) -> Vec3 {
    let wrap = |a: f32| {
        if (-180.0..=180.0).contains(&a) {
            a
        } else {
            a - 360.0 * ((a + 180.0) / 360.0).floor()
        }
    };
    Vec3::new(wrap(angles.x), wrap(angles.y), wrap(angles.z))
}

/// An event that fired.
#[derive(Debug, Clone, PartialEq)]
pub struct FiredEvent {
    pub index: u16,
    /// The script name from the event precache table (`events/ak47.sc`).
    pub name: Option<String>,
    pub flags: u32,
    pub args: EventArgs,
    pub velocity: Vec3,
    pub fx: EventFx,
}

/// What a handler decided.
#[derive(Debug, Clone, PartialEq)]
pub enum EventFx {
    /// A weapon fired.
    Weapon(WeaponFireFx),
    /// `events/knife.sc`: a swing.
    Knife {
        local: bool,
        view_anim: Option<ViewAnim>,
        sound: SoundStart,
    },
    /// `events/vehicle.sc`: the sound of a moving vehicle starts or stops.
    Vehicle(SoundCommand),
    /// `events/decal_reset.sc`: a new round; decals and dead bodies go.
    DecalReset,
    /// `events/createsmoke.sc`: a smoke grenade bursts (`first`) or puffs.
    CreateSmoke { first: bool, args: EventArgs },
    /// `events/createexplo.sc`: Condition Zero's explosion; the original logs a stub.
    CreateExplo,
    /// A script this client has no handler for, or one the server never precached.
    Unhandled,
}

/// A view-model animation (`EV_WeaponAnimation( sequence, body )`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewAnim {
    pub sequence: i32,
    pub body: i32,
}

/// The bullet types of `BULLET_PLAYER_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BulletType {
    Nine,
    Acp45,
    Ae50,
    Mm762,
    Mm556,
    Mag338,
    Mm57,
    Sig357,
    Buckshot,
}

impl BulletType {
    /// The penetration power of the ammunition: the ammunition table of `ReGameDLL_CS`
    /// (`CBaseEntity::FireBullets3`), as `cs-game` `bullets.rs` has it. Buckshot has none.
    pub fn penetration_power(self) -> i32 {
        match self {
            BulletType::Nine => 21,
            BulletType::Acp45 => 15,
            BulletType::Ae50 => 30,
            BulletType::Mm762 => 39,
            BulletType::Mm556 => 35,
            BulletType::Mag338 => 45,
            BulletType::Mm57 => 30,
            BulletType::Sig357 => 25,
            BulletType::Buckshot => 0,
        }
    }

    /// How far the ammunition penetrates.
    pub fn penetration_distance(self) -> f32 {
        match self {
            BulletType::Nine => 800.0,
            BulletType::Acp45 => 500.0,
            BulletType::Ae50 => 1000.0,
            BulletType::Mm762 => 5000.0,
            BulletType::Mm556 => 4000.0,
            BulletType::Mag338 => 8000.0,
            BulletType::Mm57 => 2000.0,
            BulletType::Sig357 => 800.0,
            BulletType::Buckshot => 0.0,
        }
    }
}

/// The call the weapon scripts make to `EV_HLDM_FireBullets`: the shell traces the bullets and
/// draws their impacts.
#[derive(Debug, Clone, PartialEq)]
pub struct BulletFire {
    pub entity: i32,
    /// The gun position: the origin plus the view height.
    pub origin: Vec3,
    pub forward: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    pub shots: u32,
    /// The spread on the right and up axes.
    pub spread: [f32; 2],
    pub distance: f32,
    pub bullet: BulletType,
    /// The last argument of the call: how many walls the bullet may go through.
    pub penetration: i32,
}

/// A shell leaving the weapon (`EV_EjectBrass`).
#[derive(Debug, Clone, PartialEq)]
pub struct ShellEject {
    pub origin: Vec3,
    pub velocity: Vec3,
    /// The yaw the shell model starts with.
    pub yaw: f32,
    pub model: &'static str,
    /// A `te_bounce::*` value.
    pub kind: u8,
}

/// What a weapon event does.
#[derive(Debug, Clone, PartialEq)]
pub struct WeaponFireFx {
    /// The weapon's script name without `events/` and `.sc`.
    pub weapon: &'static str,
    pub entity: i32,
    /// The invoker is the local player.
    pub local: bool,
    /// The view-model animation; only for the local player.
    pub view_anim: Option<ViewAnim>,
    /// The muzzle flash of the view model; only for the local player.
    pub muzzle_flash: bool,
    pub shell: Option<ShellEject>,
    pub sound: Option<SoundStart>,
    pub bullets: Option<BulletFire>,
}

/// How a script picks the animation of a shot.
#[derive(Debug, Clone, Copy)]
enum Anim {
    /// A random sequence of `first..=last`.
    Random(i32, i32),
    /// `bparam1` (the magazine is empty) picks the empty sequence.
    EmptyAware {
        shoot: (i32, i32),
        empty: i32,
    },
    /// M4A1: `bparam1` is "silenced".
    M4a1,
    Usp,
    Glock,
    P228,
    Elite,
    /// The knife: the sequence is `iparam1`.
    Knife,
}

/// How a script picks its sound.
#[derive(Debug, Clone, Copy)]
enum Pick {
    Random,
    M4a1,
    Usp,
    Glock,
}

/// How a script derives the spread of its bullets.
#[derive(Debug, Clone, Copy)]
enum Spread {
    /// `fparam1`, `fparam2`.
    Args,
    /// `fparam1 / 1000`, `fparam2 / 1000` (the scout).
    ArgsMilli,
    /// A fixed cone (the shotguns).
    Fixed(f32),
    /// The elites: `fparam2` and `iparam1 / 100`.
    Elite,
}

#[derive(Debug, Clone, Copy)]
struct Shell {
    model: &'static str,
    /// (forward, up, right) offsets for the local player, left- and right-handed.
    left: [f32; 3],
    right: [f32; 3],
}

const RSHELL: &str = "models/rshell.mdl";
const PSHELL: &str = "models/pshell.mdl";
const SHOTGUN_SHELL: &str = "models/shotgunshell.mdl";

struct Weapon {
    /// The name of the script, `events/<name>.sc`.
    name: &'static str,
    sounds: &'static [&'static str],
    pick: Pick,
    anim: Anim,
    shell: Option<Shell>,
    bullet: BulletType,
    shots: u32,
    distance: f32,
    penetration: i32,
    spread: Spread,
    /// The divisor that turns `iparam1/2` (the punch the server sends) into degrees added to
    /// the aim: 100 for most guns, 1e7 for the Famas and the Galil (whose events the server
    /// fills with `punchangle * 10000000`, `cs-game` `weapons/rifles.rs`); 0 for none.
    aim_div: f32,
}

const fn shell(model: &'static str, left: [f32; 3], right: [f32; 3]) -> Option<Shell> {
    Some(Shell { model, left, right })
}

const fn std_shell(model: &'static str, forward: f32, up: f32, right: f32) -> Option<Shell> {
    shell(model, [forward, up, -right], [forward, up, right])
}

macro_rules! weapon {
    ($name:expr, $sounds:expr, $anim:expr, $shell:expr, $bullet:expr, $pen:expr) => {
        Weapon {
            name: $name,
            sounds: $sounds,
            pick: Pick::Random,
            anim: $anim,
            shell: $shell,
            bullet: $bullet,
            shots: 1,
            distance: 8192.0,
            penetration: $pen,
            spread: Spread::Args,
            aim_div: 100.0,
        }
    };
}

/// The weapon events of the CS 1.6 client (`events/event_*.cpp`, read 2026-11-23). The
/// animation numbers are the `*_SHOOT*` of the weapons' view models (`dlls/wpn_shared.h`).
static WEAPONS: &[Weapon] = &[
    weapon!(
        "ak47",
        &["weapons/ak47-1.wav", "weapons/ak47-2.wav"],
        Anim::Random(3, 5),
        std_shell(RSHELL, 20.0, -8.0, 10.0),
        BulletType::Mm762,
        2
    ),
    weapon!(
        "aug",
        &["weapons/aug-1.wav"],
        Anim::Random(3, 5),
        std_shell(RSHELL, 17.0, -8.0, 14.0),
        BulletType::Mm556,
        2
    ),
    weapon!(
        "awp",
        &["weapons/awp1.wav"],
        Anim::Random(1, 3),
        None,
        BulletType::Mag338,
        3
    ),
    weapon!(
        "deagle",
        &["weapons/deagle-1.wav", "weapons/deagle-2.wav"],
        Anim::EmptyAware {
            shoot: (1, 2),
            empty: 3
        },
        std_shell(PSHELL, 35.0, -11.0, 16.0),
        BulletType::Ae50,
        2
    ),
    Weapon {
        name: "elite_left",
        sounds: &["weapons/elite_fire.wav"],
        anim: Anim::Elite,
        shell: shell(PSHELL, [35.0, -11.0, 16.0], [35.0, -11.0, 16.0]),
        spread: Spread::Elite,
        aim_div: 0.0,
        ..weapon!("elite_left", &[], Anim::Elite, None, BulletType::Nine, 2)
    },
    Weapon {
        name: "elite_right",
        sounds: &["weapons/elite_fire.wav"],
        anim: Anim::Elite,
        shell: shell(PSHELL, [35.0, -11.0, -16.0], [35.0, -11.0, -16.0]),
        spread: Spread::Elite,
        aim_div: 0.0,
        ..weapon!("elite_right", &[], Anim::Elite, None, BulletType::Nine, 2)
    },
    Weapon {
        aim_div: 1.0e7,
        ..weapon!(
            "famas",
            &["weapons/famas-1.wav", "weapons/famas-2.wav"],
            Anim::Random(3, 5),
            std_shell(RSHELL, 17.0, -8.0, 14.0),
            BulletType::Mm556,
            2
        )
    },
    weapon!(
        "fiveseven",
        &["weapons/fiveseven-1.wav"],
        Anim::EmptyAware {
            shoot: (1, 2),
            empty: 3
        },
        std_shell(PSHELL, 36.0, -14.0, 14.0),
        BulletType::Mm57,
        2
    ),
    weapon!(
        "g3sg1",
        &["weapons/g3sg1-1.wav"],
        Anim::Random(1, 2),
        shell(RSHELL, [20.0, -8.0, -10.0], [17.0, -8.0, 10.0]),
        BulletType::Mm762,
        2
    ),
    Weapon {
        aim_div: 1.0e7,
        ..weapon!(
            "galil",
            &["weapons/galil-1.wav", "weapons/galil-2.wav"],
            Anim::Random(3, 5),
            std_shell(RSHELL, 20.0, -8.0, 10.0),
            BulletType::Mm556,
            2
        )
    },
    Weapon {
        pick: Pick::Glock,
        distance: 4096.0,
        ..weapon!(
            "glock18",
            &["weapons/glock18-1.wav", "weapons/glock18-2.wav"],
            Anim::Glock,
            std_shell(PSHELL, 36.0, -14.0, 14.0),
            BulletType::Nine,
            2
        )
    },
    Weapon {
        aim_div: 0.0,
        ..weapon!(
            "knife",
            &["weapons/knife_miss1.wav"],
            Anim::Knife,
            None,
            BulletType::Nine,
            0
        )
    },
    weapon!(
        "m249",
        &["weapons/m249-1.wav", "weapons/m249-2.wav"],
        Anim::Random(1, 2),
        std_shell(RSHELL, 20.0, -10.0, 13.0),
        BulletType::Mm556,
        2
    ),
    Weapon {
        shots: 9,
        spread: Spread::Fixed(0.0675),
        ..weapon!(
            "m3",
            &["weapons/m3-1.wav"],
            Anim::Random(1, 2),
            None,
            BulletType::Buckshot,
            1
        )
    },
    Weapon {
        pick: Pick::M4a1,
        ..weapon!(
            "m4a1",
            &[
                "weapons/m4a1-1.wav",
                "weapons/m4a1_unsil-1.wav",
                "weapons/m4a1_unsil-2.wav"
            ],
            Anim::M4a1,
            std_shell(RSHELL, 20.0, -8.0, 10.0),
            BulletType::Mm556,
            2
        )
    },
    weapon!(
        "mac10",
        &["weapons/mac10-1.wav"],
        Anim::Random(3, 5),
        std_shell(RSHELL, 32.0, -9.0, 11.0),
        BulletType::Acp45,
        2
    ),
    weapon!(
        "mp5n",
        &["weapons/mp5-1.wav", "weapons/mp5-2.wav"],
        Anim::Random(3, 5),
        std_shell(RSHELL, 36.0, -10.0, 11.0),
        BulletType::Nine,
        2
    ),
    weapon!(
        "p228",
        &["weapons/p228-1.wav"],
        Anim::P228,
        std_shell(PSHELL, 36.0, -14.0, 14.0),
        BulletType::Sig357,
        2
    ),
    weapon!(
        "p90",
        &["weapons/p90-1.wav"],
        Anim::Random(3, 5),
        std_shell(RSHELL, 35.0, -16.0, 22.0),
        BulletType::Mm57,
        2
    ),
    Weapon {
        spread: Spread::ArgsMilli,
        ..weapon!(
            "scout",
            &["weapons/scout_fire-1.wav"],
            Anim::Random(1, 2),
            None,
            BulletType::Mm762,
            2
        )
    },
    weapon!(
        "sg550",
        &["weapons/sg550-1.wav"],
        Anim::Random(1, 2),
        shell(RSHELL, [20.0, -8.0, -10.0], [17.0, -8.0, 10.0]),
        BulletType::Mm556,
        2
    ),
    weapon!(
        "sg552",
        &["weapons/sg552-1.wav", "weapons/sg552-2.wav"],
        Anim::Random(3, 5),
        std_shell(RSHELL, 20.0, -8.0, 10.0),
        BulletType::Mm556,
        2
    ),
    weapon!(
        "tmp",
        &["weapons/tmp-1.wav", "weapons/tmp-2.wav"],
        Anim::Random(3, 5),
        std_shell(PSHELL, 32.0, -6.0, 11.0),
        BulletType::Nine,
        2
    ),
    weapon!(
        "ump45",
        &["weapons/ump45-1.wav"],
        Anim::Random(3, 5),
        std_shell(PSHELL, 34.0, -10.0, 11.0),
        BulletType::Acp45,
        2
    ),
    Weapon {
        pick: Pick::Usp,
        ..weapon!(
            "usp",
            &[
                "weapons/usp1.wav",
                "weapons/usp2.wav",
                "weapons/usp_unsil-1.wav"
            ],
            Anim::Usp,
            std_shell(PSHELL, 36.0, -14.0, 14.0),
            BulletType::Acp45,
            2
        )
    },
    Weapon {
        shots: 6,
        spread: Spread::Fixed(0.0725),
        ..weapon!(
            "xm1014",
            &["weapons/xm1014-1.wav"],
            Anim::Random(1, 2),
            std_shell(SHOTGUN_SHELL, 22.0, -9.0, 11.0),
            BulletType::Buckshot,
            1
        )
    },
];

/// The events this client has no gameplay handler for are the ones the server's own game never
/// sends (`events/new.sc`) and Condition Zero's.
fn resolve(name: &str, ev: &QueuedEvent, env: &FxEnv, rng: &mut FxRandom) -> EventFx {
    let Some(script) = name
        .strip_prefix("events/")
        .and_then(|n| n.strip_suffix(".sc"))
    else {
        return EventFx::Unhandled;
    };
    let script = script.to_ascii_lowercase();
    match script.as_str() {
        "decal_reset" => return EventFx::DecalReset,
        "createexplo" => return EventFx::CreateExplo,
        "createsmoke" => {
            // `SG_Detonate` sets `iparam2 = 1, bparam2 = 0` for the burst; `SG_Smoke` sets
            // `iparam2 = 4` (with `bparam2 = 6` truncated to `0` by the 1-bit wire delta).
            return EventFx::CreateSmoke {
                first: ev.args.bparam2 == 0 && ev.args.iparam2 != 4,
                args: ev.args,
            };
        }
        "vehicle" => return vehicle(ev),
        _ => {}
    }
    let Some(w) = WEAPONS.iter().find(|w| w.name == script) else {
        return EventFx::Unhandled;
    };
    if w.name == "knife" {
        return knife(w, ev, env, rng);
    }
    EventFx::Weapon(fire(w, ev, env, rng))
}

/// `EV_Knife`: the swing animation (from `iparam1`) for the local player and the miss sound.
fn knife(w: &Weapon, ev: &QueuedEvent, env: &FxEnv, rng: &mut FxRandom) -> EventFx {
    let local = env.is_local(ev.args.entindex);
    EventFx::Knife {
        local,
        view_anim: local.then_some(ViewAnim {
            sequence: ev.args.iparam1,
            body: 2,
        }),
        sound: event_sound(
            w.sounds[rng.long(0, w.sounds.len() as i32 - 1) as usize],
            ev,
        ),
    }
}

/// `EV_Vehicle`: `iparam1` packs the volume (bits 0 to 5, in fortieths), the pitch (bits 6 to
/// 11, in tens) and the noise (bits 12 to 14); `bparam1` stops the sound.
fn vehicle(ev: &QueuedEvent) -> EventFx {
    const NOISES: [&str; 6] = [
        "plats/vehicle1.wav",
        "plats/vehicle2.wav",
        "plats/vehicle3.wav",
        "plats/vehicle4.wav",
        "plats/vehicle6.wav",
        "plats/vehicle7.wav",
    ];
    let params = ev.args.iparam1 as u16;
    let noise = usize::from((params >> 12) & 0x7);
    let Some(name) = NOISES.get(noise) else {
        return EventFx::Unhandled;
    };
    let entity = ev.args.entindex.clamp(0, i32::from(u16::MAX)) as u16;
    if ev.args.bparam1 != 0 {
        return EventFx::Vehicle(SoundCommand::Stop {
            entity,
            channel: chan::STATIC,
        });
    }
    let volume = f32::from(params & 0x3f) / 40.0;
    let pitch = (10 * u32::from((params >> 6) & 0x3f)).min(255) as u8;
    EventFx::Vehicle(SoundCommand::Start(SoundStart {
        name: Some((*name).to_string()),
        index: 0,
        entity,
        channel: chan::STATIC,
        origin: ev.args.origin,
        volume,
        attenuation: ATTN_NORM,
        pitch: pitch.max(2),
        flags: 0,
    }))
}

/// `PLAY_EVENT_SOUND`: `EV_PlaySound( idx, origin, CHAN_WEAPON, name, VOL_NORM, ATTN_NORM, 0,
/// PITCH_NORM )` (assumption: the macro is in a header that was not fetched).
fn event_sound(name: &str, ev: &QueuedEvent) -> SoundStart {
    SoundStart::by_name(
        name,
        ev.args.entindex.clamp(0, i32::from(u16::MAX)) as u16,
        chan::WEAPON,
        ev.args.origin,
        1.0,
        ATTN_NORM,
        PITCH_NORM,
    )
}

/// The `AngleVectors` of the engine: forward, right and up for pitch, yaw and roll in degrees.
pub fn angle_vectors(angles: Vec3) -> (Vec3, Vec3, Vec3) {
    let (sp, cp) = angles.x.to_radians().sin_cos();
    let (sy, cy) = angles.y.to_radians().sin_cos();
    let (sr, cr) = angles.z.to_radians().sin_cos();
    let forward = Vec3::new(cp * cy, cp * sy, -sp);
    let right = Vec3::new(-sr * sp * cy + cr * sy, -sr * sp * sy - cr * cy, -sr * cp);
    let up = Vec3::new(cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp);
    (forward, right, up)
}

/// Height of the eye of a ducking player above its origin (ours, from [`DEFAULT_VIEWHEIGHT`]).
pub const DUCK_VIEWHEIGHT: f32 = 12.0;

/// Where the muzzle side of a player's gun is: the eye. The local player's eye is its predicted
/// `view_ofs`; any other player's is the origin plus the standard eye height. Our own design.
pub fn gun_position(origin: Vec3, local: bool, ducking: bool, env: &FxEnv) -> Vec3 {
    if local {
        origin + env.local_view_ofs
    } else {
        let h = if ducking {
            DUCK_VIEWHEIGHT
        } else {
            DEFAULT_VIEWHEIGHT
        };
        origin + Vec3::new(0.0, 0.0, h)
    }
}

/// The ejection of a shell, clean-room: it starts at `eye` moved by the weapon's
/// `[forward, up, right]` offsets (from the GPL-2+ event files) and leaves with the player's
/// velocity plus a push of our own choosing, to the side of the offset (right for a right-hand
/// gun), upwards and a little along the aim: side 50..100, up 100..150, forward -20..20 units
/// per second.
fn eject_shell(
    shell: &Shell,
    offsets: [f32; 3],
    eye: Vec3,
    (forward, right, up): (Vec3, Vec3, Vec3),
    env: &FxEnv,
    local: bool,
    rng: &mut FxRandom,
) -> ShellEject {
    let [f, u, r] = offsets;
    let side = if r < 0.0 { -1.0 } else { 1.0 };
    let origin = eye + forward * f + up * u + right * r;
    let base = if local {
        env.local_velocity
    } else {
        Vec3::ZERO
    };
    let velocity = base
        + right * (side * rng.float(50.0, 100.0))
        + up * rng.float(100.0, 150.0)
        + forward * rng.float(-20.0, 20.0);
    ShellEject {
        origin,
        velocity,
        yaw: rng.float(0.0, 360.0),
        model: shell.model,
        kind: if shell.model == SHOTGUN_SHELL {
            te_bounce::SHOTSHELL
        } else {
            te_bounce::SHELL
        },
    }
}

/// A weapon script (`EV_Fire*`).
fn fire(w: &Weapon, ev: &QueuedEvent, env: &FxEnv, rng: &mut FxRandom) -> WeaponFireFx {
    let a = &ev.args;
    let idx = a.entindex;
    let local = env.is_local(idx);
    let origin = a.origin;
    let angles = if w.aim_div != 0.0 {
        Vec3::new(
            a.iparam1 as f32 / w.aim_div + a.angles.x,
            a.iparam2 as f32 / w.aim_div + a.angles.y,
            a.angles.z,
        )
    } else {
        a.angles
    };
    let (forward, right, up) = angle_vectors(angles);

    let eye = gun_position(origin, local, a.ducking != 0, env);

    let (view_anim, muzzle_flash) = if local {
        let seq = pick_anim(w, a, env, rng);
        let flash = match w.anim {
            // Silenced M4A1 (`bparam1 != 0`) and silenced/shielded USP omit muzzle flash.
            Anim::M4a1 => a.bparam1 == 0,
            Anim::Usp => a.bparam2 == 0 && !env.holding_shield,
            _ => true,
        };
        (seq.map(|sequence| ViewAnim { sequence, body: 2 }), flash)
    } else {
        (None, false)
    };

    let shell = w.shell.map(|s| {
        // Another player's gun is always drawn on the right-hand side.
        let offsets = if !local || env.right_hand {
            s.right
        } else {
            s.left
        };
        eject_shell(&s, offsets, eye, (forward, right, up), env, local, rng)
    });

    let sound = Some(event_sound(pick_sound(w, a, env, rng), ev));

    // `EV_FireElite` moves the muzzle five units to its own side.
    let mut gun = eye;
    if matches!(w.anim, Anim::Elite) {
        gun += if w.name == "elite_right" {
            right * 5.0
        } else {
            right * -5.0
        };
    }
    let spread = match w.spread {
        Spread::Args => [a.fparam1, a.fparam2],
        Spread::ArgsMilli => [a.fparam1 / 1000.0, a.fparam2 / 1000.0],
        Spread::Fixed(v) => [v, v],
        Spread::Elite => [a.fparam2, a.iparam1 as f32 / 100.0],
    };
    let bullets = (w.shots > 0 && w.name != "knife").then_some(BulletFire {
        entity: idx,
        origin: gun,
        forward,
        right,
        up,
        shots: w.shots,
        spread,
        distance: w.distance,
        bullet: w.bullet,
        penetration: w.penetration,
    });

    WeaponFireFx {
        weapon: w.name,
        entity: idx,
        local,
        view_anim,
        muzzle_flash,
        shell,
        sound,
        bullets,
    }
}

fn pick_anim(w: &Weapon, a: &EventArgs, env: &FxEnv, rng: &mut FxRandom) -> Option<i32> {
    Some(match w.anim {
        Anim::Random(first, last) => rng.long(first, last),
        Anim::EmptyAware { shoot, empty } => {
            if a.bparam1 == 0 {
                rng.long(shoot.0, shoot.1)
            } else {
                empty
            }
        }
        // `M4A1_SHOOT1..3` when silenced, `M4A1_UNSIL_SHOOT1..3` otherwise.
        Anim::M4a1 => {
            if a.bparam1 != 0 {
                rng.long(1, 3)
            } else {
                rng.long(8, 10)
            }
        }
        Anim::Usp => {
            let unsilenced = a.bparam2 == 0;
            let empty = a.bparam1 != 0;
            if env.holding_shield {
                if empty { 3 } else { rng.long(1, 2) }
            } else if unsilenced {
                if empty { 12 } else { rng.long(9, 11) }
            } else if empty {
                4
            } else {
                rng.long(1, 3)
            }
        }
        Anim::Glock => {
            // `WPNSTATE_GLOCK18_BURST_MODE` is bit 1 of the weapon state.
            let burst = env.weapon_state & 2 != 0;
            if a.bparam1 == 0 {
                if env.holding_shield {
                    rng.long(1, 2)
                } else if burst {
                    3
                } else {
                    5
                }
            } else if env.holding_shield {
                3
            } else {
                6
            }
        }
        Anim::P228 => {
            if a.bparam1 == 0 {
                if env.holding_shield {
                    rng.long(1, 2)
                } else {
                    rng.long(1, 3)
                }
            } else if env.holding_shield {
                3
            } else {
                4
            }
        }
        // `ELITE_SHOOTLEFT1..5` and `LAST`, from the time since the last shot (`fparam1`) and
        // the rounds left (`iparam2`); the right gun's sequences come 6 later.
        Anim::Elite => {
            let diff = a.fparam1;
            let mut seq = if a.iparam2 == 0 {
                7
            } else if diff >= 0.5 {
                6
            } else if diff >= 0.4 {
                5
            } else if diff >= 0.3 {
                4
            } else if diff >= 0.2 {
                3
            } else {
                2
            };
            if w.name == "elite_right" {
                seq += 6;
            }
            seq
        }
        Anim::Knife => a.iparam1,
    })
}

fn pick_sound(w: &Weapon, a: &EventArgs, env: &FxEnv, rng: &mut FxRandom) -> &'static str {
    let last = w.sounds.len() as i32 - 1;
    match w.pick {
        Pick::Random => w.sounds[rng.long(0, last) as usize],
        Pick::M4a1 => {
            if a.bparam1 != 0 {
                w.sounds[0]
            } else {
                w.sounds[rng.long(1, 2) as usize]
            }
        }
        Pick::Usp => {
            if a.bparam2 == 0 {
                w.sounds[2]
            } else {
                w.sounds[rng.long(0, 1) as usize]
            }
        }
        Pick::Glock => {
            let burst = env.weapon_state & 2 != 0;
            if burst && !env.holding_shield {
                w.sounds[0]
            } else {
                w.sounds[1]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempents::NoWorld;
    use cs_proto48::delta::GoldSrcEntityState;
    use cs_proto48::event::WireEvent;
    use cs_proto48::messages::ResourceEntry;

    fn events_table(names: &[&str]) -> PrecacheTables {
        let mut t = PrecacheTables::default();
        let entries: Vec<ResourceEntry> = names
            .iter()
            .enumerate()
            .map(|(i, n)| ResourceEntry {
                res_type: crate::resources::res_type::EVENTSCRIPT,
                filename: (*n).to_string(),
                index: i as u16 + 1,
                download_size: 0,
                flags: 0,
                md5: None,
                reserved: None,
            })
            .collect();
        t.add(&entries);
        t
    }

    fn state(number: u16, origin: Vec3, angles: Vec3) -> GoldSrcEntityState {
        GoldSrcEntityState {
            number,
            origin,
            angles,
            ..Default::default()
        }
    }

    fn wire(
        index: u16,
        packet_index: Option<u16>,
        args: Option<EventArgs>,
        delay: Option<u16>,
    ) -> SvcMessage {
        SvcMessage::Event(vec![WireEvent {
            index,
            packet_index,
            args,
            fire_time: delay,
        }])
    }

    #[test]
    fn weapon_fire_event_produces_view_anim_and_shell() {
        let precache = events_table(&["events/ak47.sc"]);
        let mut entities = BTreeMap::new();
        entities.insert(0, state(0, Vec3::ZERO, Vec3::ZERO));
        entities.insert(
            3,
            state(3, Vec3::new(100.0, 200.0, 50.0), Vec3::new(0.0, 90.0, 0.0)),
        );
        let mut env = FxEnv::new(10.0, &precache, &entities, &NoWorld);
        env.local_player = 3;
        env.local_view_ofs = Vec3::new(0.0, 0.0, 17.0);

        let mut q = EventQueue::default();
        // The invoker is the second entity of the update; the args carry the recoil only.
        let args = EventArgs {
            iparam1: 150,
            iparam2: -50,
            fparam1: 0.02,
            fparam2: 0.03,
            ..Default::default()
        };
        q.handle(&wire(1, Some(1), Some(args), None), &env);
        assert_eq!(q.len(), 1);
        let mut rng = FxRandom::new(2);
        let fired = q.fire_due(&env, &mut rng);
        assert!(q.is_empty());
        assert_eq!(fired.len(), 1);
        let EventFx::Weapon(fx) = &fired[0].fx else {
            panic!("{:?}", fired[0].fx);
        };
        assert_eq!(fx.weapon, "ak47");
        assert_eq!(fx.entity, 3);
        assert!(fx.local);
        let anim = fx
            .view_anim
            .expect("the local player sees the view-model animation");
        assert!(
            (3..=5).contains(&anim.sequence),
            "AK47_SHOOT1..3, got {anim:?}"
        );
        assert_eq!(anim.body, 2);
        assert!(fx.muzzle_flash);

        let shell = fx.shell.as_ref().expect("an AK-47 ejects a shell");
        assert_eq!(shell.model, "models/rshell.mdl");
        assert_eq!(shell.kind, te_bounce::SHELL);
        // The origin and the angles came from the entity; the shell leaves from the gun, which
        // is the eye (view height 17) moved by the weapon's offsets (20 forward, 8 down, 10 to
        // the left for a left-handed view).
        assert!((fired[0].args.origin - Vec3::new(100.0, 200.0, 50.0)).length() < 1e-3);
        let (forward, _right, _up) = angle_vectors(Vec3::new(1.5, 90.0 - 0.5, 0.0));
        let eye = Vec3::new(100.0, 200.0, 67.0);
        let (f, r, u) = (forward, _right, _up);
        let expect = eye + f * 20.0 + u * -8.0 + r * -10.0;
        assert!((shell.origin - expect).length() < 1e-3);
        assert!(shell.velocity.length() > 50.0);

        let sound = fx.sound.as_ref().unwrap();
        assert!(sound.name.as_deref().unwrap().starts_with("weapons/ak47-"));
        assert_eq!((sound.channel, sound.entity), (chan::WEAPON, 3));

        let b = fx.bullets.as_ref().unwrap();
        assert_eq!((b.shots, b.distance, b.penetration), (1, 8192.0, 2));
        assert_eq!(b.bullet, BulletType::Mm762);
        assert_eq!(b.spread, [0.02, 0.03]);
        // The gun is at the eye.
        assert!((b.origin - Vec3::new(100.0, 200.0, 67.0)).length() < 1e-3);
        assert!((b.forward - forward).length() < 1e-5);
    }

    #[test]
    fn another_player_gets_the_remote_shell_and_no_view_anim() {
        let precache = events_table(&["events/deagle.sc"]);
        let mut entities = BTreeMap::new();
        entities.insert(5, state(5, Vec3::new(0.0, 0.0, 10.0), Vec3::ZERO));
        let mut env = FxEnv::new(1.0, &precache, &entities, &NoWorld);
        env.local_player = 2;
        let mut q = EventQueue::default();
        q.handle(&wire(1, Some(0), None, None), &env);
        let fired = q.fire_due(&env, &mut FxRandom::new(9));
        let EventFx::Weapon(fx) = &fired[0].fx else {
            panic!();
        };
        assert!(!fx.local);
        assert!(fx.view_anim.is_none());
        assert!(!fx.muzzle_flash);
        // From the standard eye height above the other player's origin, to its right.
        let shell = fx.shell.as_ref().unwrap();
        assert!((shell.origin.z - (10.0 + DEFAULT_VIEWHEIGHT - 11.0)).abs() < 1e-3);
        assert!(shell.velocity.length() > 50.0);
        assert_eq!(fired[0].args.angles, Vec3::ZERO);
    }

    #[test]
    fn a_players_pitch_is_unpacked() {
        let precache = events_table(&["events/awp.sc"]);
        let mut entities = BTreeMap::new();
        entities.insert(4, state(4, Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0)));
        let env = FxEnv::new(0.0, &precache, &entities, &NoWorld);
        let mut q = EventQueue::default();
        q.handle(&wire(1, Some(0), None, None), &env);
        let fired = q.fire_due(&env, &mut FxRandom::default());
        assert_eq!(fired[0].args.angles.x, -30.0);
        assert_eq!(fired[0].args.entindex, 4);
    }

    #[test]
    fn a_delayed_event_waits_for_its_time() {
        let precache = events_table(&["events/awp.sc"]);
        let mut entities = BTreeMap::new();
        entities.insert(4, state(4, Vec3::ZERO, Vec3::ZERO));
        let mut env = FxEnv::new(10.0, &precache, &entities, &NoWorld);
        let mut q = EventQueue::default();
        // 250 hundredths of a second.
        q.handle(&wire(1, Some(0), None, Some(250)), &env);
        let mut rng = FxRandom::default();
        assert!(q.fire_due(&env, &mut rng).is_empty());
        env.time = 12.4;
        assert!(q.fire_due(&env, &mut rng).is_empty());
        env.time = 12.5;
        assert_eq!(q.fire_due(&env, &mut rng).len(), 1);
        assert!(q.is_empty());
    }

    #[test]
    fn an_event_without_an_invoker_is_not_queued() {
        let precache = events_table(&["events/awp.sc"]);
        let entities = BTreeMap::new();
        let env = FxEnv::new(0.0, &precache, &entities, &NoWorld);
        let mut q = EventQueue::default();
        q.handle(&wire(1, None, None, None), &env);
        assert!(q.is_empty());
    }

    #[test]
    fn a_reliable_event_takes_the_slot_of_an_unreliable_one() {
        let precache = events_table(&["events/awp.sc"]);
        let entities = BTreeMap::new();
        let env = FxEnv::new(0.0, &precache, &entities, &NoWorld);
        let mut q = EventQueue::default();
        for _ in 0..MAX_EVENT_QUEUE {
            q.queue(fev::SERVER, 1, 5.0, 0.0, EventArgs::default(), Vec3::ZERO);
        }
        // Full, and unreliable ones do not make room.
        q.queue(fev::SERVER, 2, 0.0, 0.0, EventArgs::default(), Vec3::ZERO);
        assert_eq!(q.dropped, 1);
        q.handle(
            &SvcMessage::EventReliable {
                index: 3,
                args: EventArgs::default(),
                fire_time: None,
            },
            &env,
        );
        assert_eq!(q.dropped, 1);
        assert_eq!(q.len(), MAX_EVENT_QUEUE);
        let slot0 = q.slots[0].as_ref().unwrap();
        assert_eq!((slot0.index, slot0.flags), (3, fev::RELIABLE | fev::SERVER));
        // A second reliable one skips the first and takes the next unreliable slot.
        q.queue(
            fev::RELIABLE | fev::SERVER,
            4,
            0.0,
            0.0,
            EventArgs::default(),
            Vec3::ZERO,
        );
        assert_eq!(q.slots[1].as_ref().unwrap().index, 4);
    }

    #[test]
    fn scripts_without_a_handler_are_reported() {
        let precache = events_table(&["events/new.sc", "events/czerods.sc"]);
        let entities = BTreeMap::new();
        let env = FxEnv::new(0.0, &precache, &entities, &NoWorld);
        let mut q = EventQueue::default();
        q.queue(fev::SERVER, 1, 0.0, 0.0, EventArgs::default(), Vec3::ZERO);
        q.queue(fev::SERVER, 2, 0.0, 0.0, EventArgs::default(), Vec3::ZERO);
        q.queue(fev::SERVER, 77, 0.0, 0.0, EventArgs::default(), Vec3::ZERO);
        let fired = q.fire_due(&env, &mut FxRandom::default());
        assert_eq!(fired.len(), 3);
        assert!(fired.iter().all(|f| f.fx == EventFx::Unhandled));
        assert_eq!(fired[2].name, None);
    }

    #[test]
    fn m4a1_and_usp_read_their_silencer_flags() {
        let precache = events_table(&["events/m4a1.sc", "events/usp.sc"]);
        let mut entities = BTreeMap::new();
        entities.insert(1, state(1, Vec3::ZERO, Vec3::ZERO));
        let mut env = FxEnv::new(0.0, &precache, &entities, &NoWorld);
        env.local_player = 1;
        let mut rng = FxRandom::new(4);

        let ev = |index: u16, bparam1: i32, bparam2: i32| QueuedEvent {
            index,
            flags: fev::SERVER,
            fire_time: 0.0,
            args: EventArgs {
                entindex: 1,
                bparam1,
                bparam2,
                ..Default::default()
            },
            velocity: Vec3::ZERO,
        };
        for _ in 0..20 {
            // bparam1 = silenced.
            let EventFx::Weapon(sil) = resolve("events/m4a1.sc", &ev(1, 1, 0), &env, &mut rng)
            else {
                panic!();
            };
            assert!((1..=3).contains(&sil.view_anim.unwrap().sequence));
            assert_eq!(
                sil.sound.unwrap().name.as_deref(),
                Some("weapons/m4a1-1.wav")
            );
            let EventFx::Weapon(un) = resolve("events/m4a1.sc", &ev(1, 0, 0), &env, &mut rng)
            else {
                panic!();
            };
            assert!((8..=10).contains(&un.view_anim.unwrap().sequence));
            assert!(
                un.sound
                    .unwrap()
                    .name
                    .unwrap()
                    .starts_with("weapons/m4a1_unsil-")
            );
        }
        // USP: `bparam2` clear is unsilenced (makes a muzzle flash); `bparam2` non-zero is silenced (no flash).
        let EventFx::Weapon(on) = resolve("events/usp.sc", &ev(2, 0, 0), &env, &mut rng) else {
            panic!();
        };
        assert!(on.muzzle_flash);
        assert!((9..=11).contains(&on.view_anim.unwrap().sequence));
        assert_eq!(
            on.sound.unwrap().name.as_deref(),
            Some("weapons/usp_unsil-1.wav")
        );
        let EventFx::Weapon(off) = resolve("events/usp.sc", &ev(2, 1, 1), &env, &mut rng) else {
            panic!();
        };
        assert!(!off.muzzle_flash);
        assert_eq!(off.view_anim.unwrap().sequence, 4);
    }

    #[test]
    fn shotguns_fire_a_cone_of_pellets_and_the_knife_swings() {
        let precache = events_table(&["events/m3.sc", "events/knife.sc"]);
        let mut entities = BTreeMap::new();
        entities.insert(1, state(1, Vec3::ZERO, Vec3::ZERO));
        let mut env = FxEnv::new(0.0, &precache, &entities, &NoWorld);
        env.local_player = 1;
        let mut rng = FxRandom::new(8);
        let args = EventArgs {
            entindex: 1,
            iparam1: 5,
            ..Default::default()
        };
        let ev = |args| QueuedEvent {
            index: 1,
            flags: fev::SERVER,
            fire_time: 0.0,
            args,
            velocity: Vec3::ZERO,
        };
        let EventFx::Weapon(m3) = resolve("events/m3.sc", &ev(args), &env, &mut rng) else {
            panic!();
        };
        let b = m3.bullets.unwrap();
        assert_eq!(
            (b.shots, b.bullet, b.penetration),
            (9, BulletType::Buckshot, 1)
        );
        assert_eq!(b.spread, [0.0675, 0.0675]);
        assert!(m3.shell.is_none());
        let EventFx::Knife {
            local,
            view_anim,
            sound,
        } = resolve("events/knife.sc", &ev(args), &env, &mut rng)
        else {
            panic!();
        };
        assert!(local);
        assert_eq!(view_anim.unwrap().sequence, 5);
        assert_eq!(sound.name.as_deref(), Some("weapons/knife_miss1.wav"));
    }

    #[test]
    fn every_server_event_has_a_handler_or_is_named() {
        // The scripts `cs-game` precaches.
        let names = [
            "ak47",
            "aug",
            "awp",
            "createexplo",
            "createsmoke",
            "deagle",
            "decal_reset",
            "elite_left",
            "elite_right",
            "famas",
            "fiveseven",
            "g3sg1",
            "galil",
            "glock18",
            "knife",
            "m249",
            "m3",
            "m4a1",
            "mac10",
            "mp5n",
            "p228",
            "p90",
            "scout",
            "sg550",
            "sg552",
            "tmp",
            "ump45",
            "usp",
            "xm1014",
        ];
        let entities = BTreeMap::new();
        let precache = PrecacheTables::default();
        let env = FxEnv::new(0.0, &precache, &entities, &NoWorld);
        let mut rng = FxRandom::default();
        for n in names {
            let ev = QueuedEvent {
                index: 1,
                flags: fev::SERVER,
                fire_time: 0.0,
                args: EventArgs::default(),
                velocity: Vec3::ZERO,
            };
            let fx = resolve(&format!("events/{n}.sc"), &ev, &env, &mut rng);
            assert_ne!(fx, EventFx::Unhandled, "{n}");
        }
    }

    #[test]
    fn a_vehicle_event_starts_and_stops_its_sound() {
        let ev = |bparam1: i32| QueuedEvent {
            index: 1,
            flags: fev::SERVER,
            fire_time: 0.0,
            args: EventArgs {
                entindex: 9,
                iparam1: (2 << 12) | (30 << 6) | 20,
                bparam1,
                ..Default::default()
            },
            velocity: Vec3::ZERO,
        };
        let EventFx::Vehicle(SoundCommand::Start(s)) = vehicle(&ev(0)) else {
            panic!();
        };
        assert_eq!(s.name.as_deref(), Some("plats/vehicle3.wav"));
        assert_eq!(s.volume, 0.5);
        assert_eq!(s.pitch, 255);
        assert_eq!(
            vehicle(&ev(1)),
            EventFx::Vehicle(SoundCommand::Stop {
                entity: 9,
                channel: chan::STATIC
            })
        );
    }

    #[test]
    fn client_playback_ignores_server_events() {
        let precache = events_table(&["events/ak47.sc"]);
        let entities = BTreeMap::new();
        let mut env = FxEnv::new(3.0, &precache, &entities, &NoWorld);
        env.local_player = 6;
        let mut q = EventQueue::default();
        q.playback(fev::SERVER, 1, 0.0, EventArgs::default(), &env);
        q.playback(fev::NOTHOST, 9, 0.0, EventArgs::default(), &env);
        assert!(q.is_empty());
        q.playback(fev::NOTHOST, 1, 0.5, EventArgs::default(), &env);
        let ev = q.slots[0].as_ref().unwrap();
        assert_eq!(ev.flags, fev::CLIENT);
        assert_eq!(ev.args.entindex, 6);
        assert_eq!(ev.fire_time, 3.5);
    }

    /// The arguments the server's weapons hand to `PLAYBACK_EVENT_FULL`, built with the casts
    /// of `cs-game` (`weapons/rifles.rs` `int_x1e7`, `pistols.rs` `(p.x * 100.0) as i32`,
    /// `snipers.rs`), and fired through the queue.
    #[test]
    fn the_servers_own_event_arguments_turn_the_aim_by_the_right_divisor() {
        let precache = events_table(&[
            "events/famas.sc",
            "events/galil.sc",
            "events/usp.sc",
            "events/scout.sc",
            "events/glock18.sc",
        ]);
        let mut entities = BTreeMap::new();
        entities.insert(1, state(1, Vec3::ZERO, Vec3::ZERO));
        let mut env = FxEnv::new(0.0, &precache, &entities, &NoWorld);
        env.local_player = 1;
        let punch = Vec3::new(-2.0, 0.5, 0.0);
        let x1e7 = |v: f32| (v * 10_000_000.0) as i32;
        let x100 = |v: f32| (v * 100.0) as i32;
        let spread = (0.03_f32, -0.02_f32);
        let cases: [(u16, i32, i32); 5] = [
            (1, x1e7(punch.x), x1e7(punch.y)),
            (2, x1e7(punch.x), x1e7(punch.y)),
            (3, x100(punch.x), 0),
            (4, x100(punch.x), x100(punch.y)),
            (5, x100(punch.x), x100(punch.y)),
        ];
        for (index, iparam1, iparam2) in cases {
            let mut q = EventQueue::default();
            q.queue(
                fev::SERVER,
                index,
                0.0,
                0.0,
                EventArgs {
                    entindex: 1,
                    fparam1: spread.0,
                    fparam2: spread.1,
                    iparam1,
                    iparam2,
                    ..Default::default()
                },
                Vec3::ZERO,
            );
            let fired = q.fire_due(&env, &mut FxRandom::new(5));
            let EventFx::Weapon(fx) = &fired[0].fx else {
                panic!("{:?}", fired[0].fx);
            };
            let b = fx.bullets.as_ref().unwrap();
            // The aim is the punch itself, not the punch times 100000.
            let y = if index == 3 { 0.0 } else { punch.y };
            let (expect, ..) = angle_vectors(Vec3::new(punch.x, y, 0.0));
            assert!(
                (b.forward - expect).length() < 1e-3,
                "event {index}: {:?} vs {expect:?}",
                b.forward
            );
            let div = if index == 4 { 1000.0 } else { 1.0 };
            assert_eq!(b.spread, [spread.0 / div, spread.1 / div]);
        }
    }

    #[test]
    fn angle_vectors_match_the_engine_convention() {
        let (f, r, u) = angle_vectors(Vec3::new(0.0, 90.0, 0.0));
        assert!((f - Vec3::Y).length() < 1e-6);
        assert!((r - Vec3::X).length() < 1e-6);
        assert!((u - Vec3::Z).length() < 1e-6);
        let (f, _, u) = angle_vectors(Vec3::new(90.0, 0.0, 0.0));
        assert!((f - Vec3::NEG_Z).length() < 1e-6);
        assert!((u - Vec3::X).length() < 1e-6);
    }

    #[test]
    fn shells_leave_to_the_right_of_a_right_handed_view() {
        let precache = PrecacheTables::default();
        let entities = BTreeMap::new();
        let mut env = FxEnv::new(0.0, &precache, &entities, &NoWorld);
        env.right_hand = true;
        env.local_player = 1;
        let w = WEAPONS.iter().find(|w| w.name == "ak47").unwrap();
        let ev = QueuedEvent {
            index: 1,
            flags: 0,
            fire_time: 0.0,
            args: EventArgs {
                entindex: 1,
                origin: Vec3::ZERO,
                angles: Vec3::new(0.0, 90.0, 0.0),
                ..Default::default()
            },
            velocity: Vec3::ZERO,
        };
        let mut rng = FxRandom::new(5);
        let fx = fire(w, &ev, &env, &mut rng);
        let shell = fx.shell.unwrap();
        let (_, right, _) = angle_vectors(Vec3::new(0.0, 90.0, 0.0));
        assert!(shell.origin.dot(right) > 5.0);
        assert!(shell.velocity.dot(right) > 40.0);
        assert!(shell.velocity.z > 90.0);
        // A left-handed view mirrors it.
        env.right_hand = false;
        let fx = fire(w, &ev, &env, &mut rng);
        assert!(fx.shell.unwrap().velocity.dot(right) < -40.0);
    }

    #[test]
    fn local_event_falls_back_to_local_origin_when_entity_origin_is_zero() {
        let precache = events_table(&["events/glock18.sc"]);
        let mut entities = BTreeMap::new();
        entities.insert(1, state(1, Vec3::ZERO, Vec3::ZERO));
        let mut env = FxEnv::new(1.0, &precache, &entities, &NoWorld);
        env.local_player = 1;
        env.local_origin = Vec3::new(100.0, 200.0, 30.0);

        let mut q = EventQueue::default();
        let ev = cs_proto48::event::WireEvent {
            index: 1,
            packet_index: Some(0),
            fire_time: None,
            args: Some(EventArgs::default()),
        };
        q.handle(&SvcMessage::Event(vec![ev]), &env);
        let mut rng = FxRandom::new(1);
        let fired = q.fire_due(&env, &mut rng);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].args.origin, Vec3::new(100.0, 200.0, 30.0));
    }

    #[test]
    fn createsmoke_distinguishes_detonate_burst_from_subsequent_smoke_ticks() {
        let precache = events_table(&["events/createsmoke.sc"]);
        let entities = BTreeMap::new();
        let env = FxEnv::new(0.0, &precache, &entities, &NoWorld);
        let mut rng = FxRandom::default();

        let burst = QueuedEvent {
            index: 1,
            flags: fev::SERVER,
            fire_time: 0.0,
            args: EventArgs {
                iparam2: 1,
                bparam2: 0,
                ..Default::default()
            },
            velocity: Vec3::ZERO,
        };
        assert!(matches!(
            resolve("events/createsmoke.sc", &burst, &env, &mut rng),
            EventFx::CreateSmoke { first: true, .. }
        ));

        // Subsequent 1Hz `SG_Smoke` ticks send `iparam2 = 4` (with `bparam2` delta-truncated to 0).
        let follow_up = QueuedEvent {
            index: 1,
            flags: fev::SERVER,
            fire_time: 0.0,
            args: EventArgs {
                iparam1: 250,
                iparam2: 4,
                bparam2: 0,
                ..Default::default()
            },
            velocity: Vec3::ZERO,
        };
        assert!(matches!(
            resolve("events/createsmoke.sc", &follow_up, &env, &mut rng),
            EventFx::CreateSmoke { first: false, .. }
        ));
    }
}
