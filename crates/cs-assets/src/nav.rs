// SPDX-License-Identifier: GPL-3.0-or-later
// Ported from ReGameDLL_CS `regamedll/game_shared/bot/nav.h`, `nav_file.cpp`,
// `nav_area.h`, and `nav_area.cpp`.

//! Valve Counter-Strike 1.6 / Condition Zero binary navigation mesh (`.nav`) codec.
//!
//! Supports `.nav` versions `1..=5` (`0xFEEDFACE` magic header), place name dictionaries,
//! [`NavArea`] geometry, [`NavAttributes`] bitflags, 4-way cardinal links, [`HidingSpot`]
//! entries, [`ApproachSpot`] transitions, [`SpotEncounter`] tables, and [`NavLadder`] metadata.

use std::fmt;
use std::ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign};

use cs_vfs::Vfs;
use glam::{Vec2, Vec3};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Binary `.nav` file magic number (`0xFEEDFACE` little-endian).
pub const NAV_MAGIC_NUMBER: u32 = 0xFEED_FACE;
/// Earliest supported `.nav` binary format version.
pub const NAV_MIN_VERSION: u32 = 1;
/// Latest Valve CS 1.6 / Condition Zero `.nav` binary format version.
pub const NAV_VERSION: u32 = 5;
/// Undefined place dictionary entry ID.
pub const UNDEFINED_PLACE: u16 = 0;
/// Maximum number of approach areas stored per `NavArea` in Valve `.nav` files.
pub const MAX_APPROACH_AREAS: usize = 16;

// Bounded parser limits for untrusted `.nav` byte streams.
const MAX_PLACE_COUNT: usize = 4096;
const MAX_PLACE_NAME_LEN: usize = 256;
const MAX_AREA_COUNT: usize = 65_536;
const MAX_CONNECTIONS_PER_DIR: usize = 4096;
const MAX_ENCOUNTERS_PER_AREA: usize = 4096;

/// Errors returned while parsing, validating, or loading a `.nav` file.
#[derive(Debug, Error)]
pub enum NavError {
    #[error("invalid .nav magic number: 0x{0:08X} (expected 0x{NAV_MAGIC_NUMBER:08X})")]
    InvalidMagic(u32),
    #[error("unsupported .nav version {0} (supported {NAV_MIN_VERSION}..={NAV_VERSION})")]
    UnsupportedVersion(u32),
    #[error("truncated .nav data while reading {0}")]
    Truncated(&'static str),
    #[error("non-finite floating point value in {0}")]
    NonFiniteFloat(&'static str),
    #[error("invalid place name length {0} (must be 1..={MAX_PLACE_NAME_LEN})")]
    InvalidPlaceLength(u16),
    #[error("invalid UTF-8 in place name")]
    InvalidUtf8,
    #[error("{what} count {count} exceeds safety limit {max}")]
    CountLimitExceeded {
        what: &'static str,
        count: usize,
        max: usize,
    },
    #[error("invalid direction byte {0}")]
    InvalidDirection(u8),
    #[error("invalid traverse type byte {0}")]
    InvalidTraverseType(u8),
    #[error("approach spot count {0} exceeds maximum {MAX_APPROACH_AREAS}")]
    TooManyApproachSpots(u8),
    #[error("invalid place dictionary entry {entry} (dictionary has {dict_len} entries)")]
    InvalidPlaceEntry { entry: u16, dict_len: usize },
    #[error("unexpected {0} trailing bytes at end of .nav file")]
    TrailingData(usize),
    #[error("VFS error: {0}")]
    Vfs(#[from] cs_vfs::VfsError),
}

/// Cardinal directions in `.nav` meshes (`NORTH = 0, EAST = 1, SOUTH = 2, WEST = 3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum NavDir {
    North = 0,
    East = 1,
    South = 2,
    West = 3,
}

impl NavDir {
    pub const ALL: [Self; 4] = [Self::North, Self::East, Self::South, Self::West];

    pub const fn as_index(self) -> usize {
        self as usize
    }

    pub const fn from_u8(val: u8) -> Result<Self, NavError> {
        match val {
            0 => Ok(Self::North),
            1 => Ok(Self::East),
            2 => Ok(Self::South),
            3 => Ok(Self::West),
            _ => Err(NavError::InvalidDirection(val)),
        }
    }

    pub const fn opposite(self) -> Self {
        match self {
            Self::North => Self::South,
            Self::East => Self::West,
            Self::South => Self::North,
            Self::West => Self::East,
        }
    }

    pub const fn left(self) -> Self {
        match self {
            Self::North => Self::West,
            Self::South => Self::East,
            Self::East => Self::North,
            Self::West => Self::South,
        }
    }

    pub const fn right(self) -> Self {
        match self {
            Self::North => Self::East,
            Self::South => Self::West,
            Self::East => Self::South,
            Self::West => Self::North,
        }
    }

    /// GoldSrc 2D direction vector (`NORTH = -Y`, `EAST = +X`, `SOUTH = +Y`, `WEST = -X`).
    pub const fn to_vec2(self) -> Vec2 {
        match self {
            Self::North => Vec2::new(0.0, -1.0),
            Self::East => Vec2::new(1.0, 0.0),
            Self::South => Vec2::new(0.0, 1.0),
            Self::West => Vec2::new(-1.0, 0.0),
        }
    }

    /// GoldSrc yaw angle in degrees matching `DirectionToAngle` in `nav.h`.
    pub const fn to_angle_deg(self) -> f32 {
        match self {
            Self::North => 270.0,
            Self::East => 0.0,
            Self::South => 90.0,
            Self::West => 180.0,
        }
    }
}

/// Area corner index (`NORTH_WEST = 0 .. SOUTH_WEST = 3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum NavCorner {
    NorthWest = 0,
    NorthEast = 1,
    SouthEast = 2,
    SouthWest = 3,
}

/// Possible traversal transitions between adjacent navigation areas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum NavTraverseType {
    GoNorth = 0,
    GoEast = 1,
    GoSouth = 2,
    GoWest = 3,
    GoLadderUp = 4,
    GoLadderDown = 5,
    GoJump = 6,
    None = 7,
}

impl NavTraverseType {
    pub const fn from_u8(val: u8) -> Result<Self, NavError> {
        match val {
            0 => Ok(Self::GoNorth),
            1 => Ok(Self::GoEast),
            2 => Ok(Self::GoSouth),
            3 => Ok(Self::GoWest),
            4 => Ok(Self::GoLadderUp),
            5 => Ok(Self::GoLadderDown),
            6 => Ok(Self::GoJump),
            7 => Ok(Self::None),
            _ => Err(NavError::InvalidTraverseType(val)),
        }
    }

    pub const fn from_dir(dir: NavDir) -> Self {
        match dir {
            NavDir::North => Self::GoNorth,
            NavDir::East => Self::GoEast,
            NavDir::South => Self::GoSouth,
            NavDir::West => Self::GoWest,
        }
    }
}

/// Ladder traversal direction on a [`NavArea`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum LadderDirection {
    Up = 0,
    Down = 1,
}

/// Navigation area attribute bitflags (`NAV_CROUCH` .. `NAV_STAND`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct NavAttribute(u32);

impl NavAttribute {
    pub const NONE: Self = Self(0);
    pub const CROUCH: Self = Self(0x0001);
    pub const JUMP: Self = Self(0x0002);
    pub const PRECISE: Self = Self(0x0004);
    pub const NO_JUMP: Self = Self(0x0008);
    pub const STOP: Self = Self(0x0010);
    pub const RUN: Self = Self(0x0020);
    pub const WALK: Self = Self(0x0040);
    pub const AVOID: Self = Self(0x0080);
    pub const TRANSIENT: Self = Self(0x0100);
    pub const DONT_HIDE: Self = Self(0x0200);
    pub const STAND: Self = Self(0x0400);

    pub const ALL_MASK: u32 = 0x07FF;

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn from_bits_truncate(bits: u32) -> Self {
        Self(bits & Self::ALL_MASK)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    pub fn remove(&mut self, other: Self) {
        self.0 &= !other.0;
    }
}

impl BitOr for NavAttribute {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for NavAttribute {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for NavAttribute {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self::Output {
        Self(self.0 & rhs.0)
    }
}

impl BitAndAssign for NavAttribute {
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0;
    }
}

impl fmt::Display for NavAttribute {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:04X}", self.0)
    }
}

/// Hiding spot classification bitflags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct HidingSpotFlags(u8);

impl HidingSpotFlags {
    pub const NONE: Self = Self(0);
    pub const IN_COVER: Self = Self(0x01);
    pub const GOOD_SNIPER_SPOT: Self = Self(0x02);
    pub const IDEAL_SNIPER_SPOT: Self = Self(0x04);
    pub const EXPOSED: Self = Self(0x08);

    pub const ALL_MASK: u8 = 0x0F;

    pub const fn from_bits_truncate(bits: u8) -> Self {
        Self(bits & Self::ALL_MASK)
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub const fn intersects(self, other: Self) -> bool {
        (self.0 & other.0) != 0
    }

    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

impl BitOr for HidingSpotFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for HidingSpotFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for HidingSpotFlags {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self::Output {
        Self(self.0 & rhs.0)
    }
}

/// Tactical hiding/sniper/cover point within a [`NavArea`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HidingSpot {
    pub id: u32,
    pub pos: Vec3,
    pub flags: HidingSpotFlags,
}

impl HidingSpot {
    pub const fn has_good_cover(&self) -> bool {
        self.flags.contains(HidingSpotFlags::IN_COVER)
    }

    pub const fn is_good_sniper_spot(&self) -> bool {
        self.flags.contains(HidingSpotFlags::GOOD_SNIPER_SPOT)
    }

    pub const fn is_ideal_sniper_spot(&self) -> bool {
        self.flags.contains(HidingSpotFlags::IDEAL_SNIPER_SPOT)
    }

    pub const fn is_exposed(&self) -> bool {
        self.flags.contains(HidingSpotFlags::EXPOSED)
    }
}

/// Approach area metadata used by tactical/hearing logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApproachSpot {
    pub here_id: u32,
    pub prev_id: u32,
    pub prev_to_here_how: NavTraverseType,
    pub next_id: u32,
    pub here_to_next_how: NavTraverseType,
}

/// Ordered hiding spot entry along a [`SpotEncounter`] path.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpotOrder {
    pub spot_id: u32,
    /// Normalized position `[0.0, 1.0]` along the encounter path segment.
    pub t: f32,
}

/// Line-of-sight encounter table for traversing a [`NavArea`] from `from_id` to `to_id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpotEncounter {
    pub from_id: u32,
    pub from_dir: NavDir,
    pub to_id: u32,
    pub to_dir: NavDir,
    pub spots: Vec<SpotOrder>,
}

/// Navigation ladder descriptor linking bottom and top areas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NavLadder {
    pub id: u32,
    pub top: Vec3,
    pub bottom: Vec3,
    pub width: f32,
    pub length: f32,
    pub dir: NavDir,
    pub top_forward_area_id: Option<u32>,
    pub top_left_area_id: Option<u32>,
    pub top_right_area_id: Option<u32>,
    pub top_behind_area_id: Option<u32>,
    pub bottom_area_id: Option<u32>,
    pub is_dangling: bool,
}

/// Rectangular walkable navigation area (`m_nwCorner` .. `m_seCorner` + implicit `ne_z`/`sw_z`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NavArea {
    pub id: u32,
    pub attributes: NavAttribute,
    /// North-West corner (`min_x, min_y, nw_z`).
    pub nw_corner: Vec3,
    /// South-East corner (`max_x, max_y, se_z`).
    pub se_corner: Vec3,
    /// Elevation at North-East corner (`se_corner.x, nw_corner.y`).
    pub ne_z: f32,
    /// Elevation at South-West corner (`nw_corner.x, se_corner.y`).
    pub sw_z: f32,
    /// Connected area IDs per cardinal direction `[North, East, South, West]`.
    pub connections: [Vec<u32>; 4],
    pub hiding_spots: Vec<HidingSpot>,
    pub approach_spots: Vec<ApproachSpot>,
    pub encounters: Vec<SpotEncounter>,
    /// 1-based index into [`NavFile::places`], or [`UNDEFINED_PLACE`] (`0`).
    pub place_id: u16,
    /// IDs of ladders ascending from this area (`LADDER_UP`).
    pub ladders_up: Vec<u32>,
    /// IDs of ladders descending from this area (`LADDER_DOWN`).
    pub ladders_down: Vec<u32>,
}

impl NavArea {
    /// Center point of the area (`(nw + se) * 0.5`).
    pub fn center(&self) -> Vec3 {
        (self.nw_corner + self.se_corner) * 0.5
    }

    pub fn size_x(&self) -> f32 {
        self.se_corner.x - self.nw_corner.x
    }

    pub fn size_y(&self) -> f32 {
        self.se_corner.y - self.nw_corner.y
    }

    pub fn is_degenerate(&self) -> bool {
        self.nw_corner.x >= self.se_corner.x || self.nw_corner.y >= self.se_corner.y
    }

    pub fn corner(&self, c: NavCorner) -> Vec3 {
        match c {
            NavCorner::NorthWest => self.nw_corner,
            NavCorner::NorthEast => Vec3::new(self.se_corner.x, self.nw_corner.y, self.ne_z),
            NavCorner::SouthEast => self.se_corner,
            NavCorner::SouthWest => Vec3::new(self.nw_corner.x, self.se_corner.y, self.sw_z),
        }
    }

    /// Bilinear interpolation of area floor elevation at `(x, y)` (`CNavArea::GetZ`).
    pub fn z_at(&self, x: f32, y: f32) -> f32 {
        let dx = self.se_corner.x - self.nw_corner.x;
        let dy = self.se_corner.y - self.nw_corner.y;
        if dx.abs() < f32::EPSILON || dy.abs() < f32::EPSILON {
            return self.ne_z;
        }
        let u = ((x - self.nw_corner.x) / dx).clamp(0.0, 1.0);
        let v = ((y - self.nw_corner.y) / dy).clamp(0.0, 1.0);
        let north_z = self.nw_corner.z + u * (self.ne_z - self.nw_corner.z);
        let south_z = self.sw_z + u * (self.se_corner.z - self.sw_z);
        north_z + v * (south_z - north_z)
    }

    /// Unit surface normal of the area quad (`GetAreaSlope` in `nav_area.h`).
    pub fn surface_normal(&self) -> Vec3 {
        let u = Vec3::new(
            self.se_corner.x - self.nw_corner.x,
            0.0,
            self.ne_z - self.nw_corner.z,
        );
        let v = Vec3::new(
            0.0,
            self.se_corner.y - self.nw_corner.y,
            self.sw_z - self.nw_corner.z,
        );
        u.cross(v).normalize_or_zero()
    }

    /// Returns `true` if `(pos.x, pos.y)` lies inside the area's 2D bounds.
    pub fn is_overlapping_xy(&self, pos: Vec3) -> bool {
        pos.x >= self.nw_corner.x
            && pos.x <= self.se_corner.x
            && pos.y >= self.nw_corner.y
            && pos.y <= self.se_corner.y
    }

    /// Closest point on the area surface to `pos` (`CNavArea::GetClosestPointOnArea`).
    pub fn closest_point(&self, pos: Vec3) -> Vec3 {
        let x = pos.x.clamp(self.nw_corner.x, self.se_corner.x);
        let y = pos.y.clamp(self.nw_corner.y, self.se_corner.y);
        Vec3::new(x, y, self.z_at(x, y))
    }

    /// Squared distance from `pos` to this area (`CNavArea::GetDistanceSquaredToPoint`).
    pub fn distance_squared_to_point(&self, pos: Vec3) -> f32 {
        if pos.x < self.nw_corner.x {
            if pos.y < self.nw_corner.y {
                self.nw_corner.distance_squared(pos)
            } else if pos.y > self.se_corner.y {
                self.corner(NavCorner::SouthWest).distance_squared(pos)
            } else {
                let d = self.nw_corner.x - pos.x;
                d * d
            }
        } else if pos.x > self.se_corner.x {
            if pos.y < self.nw_corner.y {
                self.corner(NavCorner::NorthEast).distance_squared(pos)
            } else if pos.y > self.se_corner.y {
                self.se_corner.distance_squared(pos)
            } else {
                let d = pos.x - self.se_corner.x;
                d * d
            }
        } else if pos.y < self.nw_corner.y {
            let d = self.nw_corner.y - pos.y;
            d * d
        } else if pos.y > self.se_corner.y {
            let d = pos.y - self.se_corner.y;
            d * d
        } else {
            let d = self.z_at(pos.x, pos.y) - pos.z;
            d * d
        }
    }

    /// Returns `true` if `other_id` is connected in `dir`.
    pub fn is_connected_in_dir(&self, other_id: u32, dir: NavDir) -> bool {
        self.connections[dir.as_index()].contains(&other_id)
    }

    /// Returns `true` if `other_id` is connected in any cardinal direction or via ladder.
    pub fn is_connected_any(&self, other_id: u32) -> bool {
        if other_id == self.id {
            return true;
        }
        self.connections
            .iter()
            .any(|list| list.contains(&other_id))
    }

    /// Compute `(portal_center, half_width)` along `dir` to `other`.
    pub fn compute_portal(&self, other: &Self, dir: NavDir) -> (Vec3, f32) {
        match dir {
            NavDir::North | NavDir::South => {
                let y = if dir == NavDir::North {
                    self.nw_corner.y
                } else {
                    self.se_corner.y
                };
                let left = self
                    .nw_corner
                    .x
                    .max(other.nw_corner.x)
                    .clamp(self.nw_corner.x, self.se_corner.x);
                let right = self
                    .se_corner
                    .x
                    .min(other.se_corner.x)
                    .clamp(self.nw_corner.x, self.se_corner.x);
                let x = (left + right) * 0.5;
                let hw = ((right - left) * 0.5).max(0.0);
                (Vec3::new(x, y, self.z_at(x, y)), hw)
            }
            NavDir::East | NavDir::West => {
                let x = if dir == NavDir::West {
                    self.nw_corner.x
                } else {
                    self.se_corner.x
                };
                let top = self
                    .nw_corner
                    .y
                    .max(other.nw_corner.y)
                    .clamp(self.nw_corner.y, self.se_corner.y);
                let bottom = self
                    .se_corner
                    .y
                    .min(other.se_corner.y)
                    .clamp(self.nw_corner.y, self.se_corner.y);
                let y = (top + bottom) * 0.5;
                let hw = ((bottom - top) * 0.5).max(0.0);
                (Vec3::new(x, y, self.z_at(x, y)), hw)
            }
        }
    }
}

/// Complete parsed or generated `.nav` navigation mesh file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NavFile {
    pub version: u32,
    pub bsp_size: u32,
    pub places: Vec<String>,
    pub areas: Vec<NavArea>,
    pub ladders: Vec<NavLadder>,
}

impl Default for NavFile {
    fn default() -> Self {
        Self {
            version: NAV_VERSION,
            bsp_size: 0,
            places: Vec::new(),
            areas: Vec::new(),
            ladders: Vec::new(),
        }
    }
}

impl NavFile {
    /// Load and parse `maps/<map_name>.nav` from the VFS.
    pub fn load(vfs: &Vfs, map_name: &str) -> Result<Self, NavError> {
        let bytes = vfs.read_bytes(&format!("maps/{map_name}.nav"))?;
        Self::parse(&bytes)
    }

    /// Parse a binary `.nav` file (versions `1..=5`) with bounded allocations.
    pub fn parse(bytes: &[u8]) -> Result<Self, NavError> {
        let mut r = Reader::new(bytes);
        let magic = r.read_u32("magic")?;
        if magic != NAV_MAGIC_NUMBER {
            return Err(NavError::InvalidMagic(magic));
        }

        let version = r.read_u32("version")?;
        if !(NAV_MIN_VERSION..=NAV_VERSION).contains(&version) {
            return Err(NavError::UnsupportedVersion(version));
        }

        let bsp_size = if version >= 4 {
            r.read_u32("bsp_size")?
        } else {
            0
        };

        let places = if version >= 5 {
            let place_count = r.read_u16("place_count")? as usize;
            if place_count > MAX_PLACE_COUNT {
                return Err(NavError::CountLimitExceeded {
                    what: "place_count",
                    count: place_count,
                    max: MAX_PLACE_COUNT,
                });
            }
            // Minimum 3 bytes per place (u16 len + at least 1 NUL byte).
            if place_count * 3 > r.remaining() {
                return Err(NavError::Truncated("place dictionary"));
            }
            let mut out = Vec::with_capacity(place_count);
            for _ in 0..place_count {
                let len = r.read_u16("place_name_len")?;
                if len == 0 || (len as usize) > MAX_PLACE_NAME_LEN {
                    return Err(NavError::InvalidPlaceLength(len));
                }
                let raw = r.read_bytes(len as usize, "place_name")?;
                let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
                let name = std::str::from_utf8(&raw[..end])
                    .map_err(|_| NavError::InvalidUtf8)?
                    .to_string();
                out.push(name);
            }
            out
        } else {
            Vec::new()
        };

        let area_count = r.read_u32("area_count")? as usize;
        if area_count > MAX_AREA_COUNT {
            return Err(NavError::CountLimitExceeded {
                what: "area_count",
                count: area_count,
                max: MAX_AREA_COUNT,
            });
        }
        // Each area needs at least 58 bytes in v1..v5.
        let min_area_bytes = 4 + 1 + 24 + 8 + 16 + 1 + 1 + 4;
        if area_count.saturating_mul(min_area_bytes) > r.remaining() {
            return Err(NavError::Truncated("areas"));
        }

        let mut next_v1_spot_id: u32 = 1;
        let mut areas = Vec::with_capacity(area_count);
        for _ in 0..area_count {
            let area = Self::parse_area(&mut r, version, places.len(), &mut next_v1_spot_id)?;
            areas.push(area);
        }

        if r.remaining() > 0 {
            return Err(NavError::TrailingData(r.remaining()));
        }

        Ok(Self {
            version,
            bsp_size,
            places,
            areas,
            ladders: Vec::new(),
        })
    }

    fn parse_area(
        r: &mut Reader<'_>,
        version: u32,
        place_dict_len: usize,
        next_v1_spot_id: &mut u32,
    ) -> Result<NavArea, NavError> {
        let id = r.read_u32("area_id")?;
        let attr_byte = r.read_u8("area_attributes")?;
        let attributes = NavAttribute::from_bits_truncate(attr_byte as u32);
        let nw_corner = r.read_vec3("nw_corner")?;
        let se_corner = r.read_vec3("se_corner")?;
        let ne_z = r.read_f32("ne_z")?;
        let sw_z = r.read_f32("sw_z")?;

        let mut connections = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
        for slot in &mut connections {
            let count = r.read_u32("connection_count")? as usize;
            if count > MAX_CONNECTIONS_PER_DIR {
                return Err(NavError::CountLimitExceeded {
                    what: "connection_count",
                    count,
                    max: MAX_CONNECTIONS_PER_DIR,
                });
            }
            if count.saturating_mul(4) > r.remaining() {
                return Err(NavError::Truncated("area connections"));
            }
            let mut ids = Vec::with_capacity(count);
            for _ in 0..count {
                ids.push(r.read_u32("connected_area_id")?);
            }
            *slot = ids;
        }

        let hiding_count = r.read_u8("hiding_spot_count")? as usize;
        let mut hiding_spots = Vec::with_capacity(hiding_count.min(r.remaining() / 12));
        if version == 1 {
            if hiding_count.saturating_mul(12) > r.remaining() {
                return Err(NavError::Truncated("v1 hiding spots"));
            }
            for _ in 0..hiding_count {
                let pos = r.read_vec3("hiding_spot_pos")?;
                let spot_id = *next_v1_spot_id;
                *next_v1_spot_id = next_v1_spot_id.saturating_add(1);
                hiding_spots.push(HidingSpot {
                    id: spot_id,
                    pos,
                    flags: HidingSpotFlags::IN_COVER,
                });
            }
        } else {
            if hiding_count.saturating_mul(17) > r.remaining() {
                return Err(NavError::Truncated("hiding spots"));
            }
            for _ in 0..hiding_count {
                let spot_id = r.read_u32("hiding_spot_id")?;
                let pos = r.read_vec3("hiding_spot_pos")?;
                let flags = HidingSpotFlags::from_bits_truncate(r.read_u8("hiding_spot_flags")?);
                if spot_id >= *next_v1_spot_id {
                    *next_v1_spot_id = spot_id.saturating_add(1);
                }
                hiding_spots.push(HidingSpot {
                    id: spot_id,
                    pos,
                    flags,
                });
            }
        }

        let approach_count = r.read_u8("approach_count")?;
        if (approach_count as usize) > MAX_APPROACH_AREAS {
            return Err(NavError::TooManyApproachSpots(approach_count));
        }
        let mut approach_spots = Vec::with_capacity(approach_count as usize);
        for _ in 0..approach_count {
            let here_id = r.read_u32("approach_here_id")?;
            let prev_id = r.read_u32("approach_prev_id")?;
            let prev_to_here_how = NavTraverseType::from_u8(r.read_u8("prev_to_here_how")?)?;
            let next_id = r.read_u32("approach_next_id")?;
            let here_to_next_how = NavTraverseType::from_u8(r.read_u8("here_to_next_how")?)?;
            approach_spots.push(ApproachSpot {
                here_id,
                prev_id,
                prev_to_here_how,
                next_id,
                here_to_next_how,
            });
        }

        let encounter_count = r.read_u32("encounter_count")? as usize;
        if encounter_count > MAX_ENCOUNTERS_PER_AREA {
            return Err(NavError::CountLimitExceeded {
                what: "encounter_count",
                count: encounter_count,
                max: MAX_ENCOUNTERS_PER_AREA,
            });
        }

        if version < 3 {
            // v1/v2 stored raw path vectors + float offsets and terminated area load here.
            if encounter_count.saturating_mul(33) > r.remaining() {
                return Err(NavError::Truncated("v1/v2 encounters"));
            }
            for _ in 0..encounter_count {
                let _from_id = r.read_u32("legacy_enc_from_id")?;
                let _to_id = r.read_u32("legacy_enc_to_id")?;
                let _from_pos = r.read_vec3("legacy_enc_from_pos")?;
                let _to_pos = r.read_vec3("legacy_enc_to_pos")?;
                let spot_count = r.read_u8("legacy_enc_spot_count")? as usize;
                if spot_count.saturating_mul(16) > r.remaining() {
                    return Err(NavError::Truncated("v1/v2 encounter spots"));
                }
                for _ in 0..spot_count {
                    let _pos = r.read_vec3("legacy_enc_spot_pos")?;
                    let _t = r.read_f32("legacy_enc_spot_t")?;
                }
            }
            return Ok(NavArea {
                id,
                attributes,
                nw_corner,
                se_corner,
                ne_z,
                sw_z,
                connections,
                hiding_spots,
                approach_spots,
                encounters: Vec::new(),
                place_id: UNDEFINED_PLACE,
                ladders_up: Vec::new(),
                ladders_down: Vec::new(),
            });
        }

        if encounter_count.saturating_mul(11) > r.remaining() {
            return Err(NavError::Truncated("encounters"));
        }
        let mut encounters = Vec::with_capacity(encounter_count);
        for _ in 0..encounter_count {
            let from_id = r.read_u32("encounter_from_id")?;
            let from_dir = NavDir::from_u8(r.read_u8("encounter_from_dir")?)?;
            let to_id = r.read_u32("encounter_to_id")?;
            let to_dir = NavDir::from_u8(r.read_u8("encounter_to_dir")?)?;
            let spot_count = r.read_u8("encounter_spot_count")? as usize;
            if spot_count.saturating_mul(5) > r.remaining() {
                return Err(NavError::Truncated("encounter spots"));
            }
            let mut spots = Vec::with_capacity(spot_count);
            for _ in 0..spot_count {
                let spot_id = r.read_u32("encounter_spot_id")?;
                let t_byte = r.read_u8("encounter_spot_t")?;
                spots.push(SpotOrder {
                    spot_id,
                    t: f32::from(t_byte) / 255.0,
                });
            }
            encounters.push(SpotEncounter {
                from_id,
                from_dir,
                to_id,
                to_dir,
                spots,
            });
        }

        let place_id = if version >= 5 {
            let entry = r.read_u16("place_entry")?;
            if entry as usize > place_dict_len {
                return Err(NavError::InvalidPlaceEntry {
                    entry,
                    dict_len: place_dict_len,
                });
            }
            entry
        } else {
            UNDEFINED_PLACE
        };

        Ok(NavArea {
            id,
            attributes,
            nw_corner,
            se_corner,
            ne_z,
            sw_z,
            connections,
            hiding_spots,
            approach_spots,
            encounters,
            place_id,
            ladders_up: Vec::new(),
            ladders_down: Vec::new(),
        })
    }

    /// Serialize this navigation mesh in Valve `.nav` v5 binary format.
    pub fn to_bytes(&self) -> Result<Vec<u8>, NavError> {
        if self.places.len() > MAX_PLACE_COUNT {
            return Err(NavError::CountLimitExceeded {
                what: "place_count",
                count: self.places.len(),
                max: MAX_PLACE_COUNT,
            });
        }
        if self.areas.len() > MAX_AREA_COUNT {
            return Err(NavError::CountLimitExceeded {
                what: "area_count",
                count: self.areas.len(),
                max: MAX_AREA_COUNT,
            });
        }

        let mut buf = Vec::with_capacity(64 + self.areas.len() * 80);
        buf.extend_from_slice(&NAV_MAGIC_NUMBER.to_le_bytes());
        buf.extend_from_slice(&NAV_VERSION.to_le_bytes());
        buf.extend_from_slice(&self.bsp_size.to_le_bytes());

        // Place dictionary (1-based indices referenced by areas).
        buf.extend_from_slice(&(self.places.len() as u16).to_le_bytes());
        for name in &self.places {
            let raw = name.as_bytes();
            let total_len = raw.len() + 1;
            if total_len > MAX_PLACE_NAME_LEN {
                return Err(NavError::InvalidPlaceLength(
                    total_len.min(u16::MAX as usize) as u16,
                ));
            }
            buf.extend_from_slice(&(total_len as u16).to_le_bytes());
            buf.extend_from_slice(raw);
            buf.push(0);
        }

        buf.extend_from_slice(&(self.areas.len() as u32).to_le_bytes());
        for area in &self.areas {
            Self::write_area(&mut buf, area, self.places.len())?;
        }

        Ok(buf)
    }

    fn write_area(buf: &mut Vec<u8>, area: &NavArea, place_dict_len: usize) -> Result<(), NavError> {
        buf.extend_from_slice(&area.id.to_le_bytes());
        buf.push((area.attributes.bits() & 0xFF) as u8);
        write_vec3(buf, area.nw_corner);
        write_vec3(buf, area.se_corner);
        buf.extend_from_slice(&area.ne_z.to_le_bytes());
        buf.extend_from_slice(&area.sw_z.to_le_bytes());

        for dir_conns in &area.connections {
            buf.extend_from_slice(&(dir_conns.len() as u32).to_le_bytes());
            for &id in dir_conns {
                buf.extend_from_slice(&id.to_le_bytes());
            }
        }

        let hiding_len = area.hiding_spots.len().min(255);
        buf.push(hiding_len as u8);
        for spot in area.hiding_spots.iter().take(hiding_len) {
            buf.extend_from_slice(&spot.id.to_le_bytes());
            write_vec3(buf, spot.pos);
            buf.push(spot.flags.bits());
        }

        if area.approach_spots.len() > MAX_APPROACH_AREAS {
            return Err(NavError::TooManyApproachSpots(
                area.approach_spots.len().min(255) as u8,
            ));
        }
        buf.push(area.approach_spots.len() as u8);
        for app in &area.approach_spots {
            buf.extend_from_slice(&app.here_id.to_le_bytes());
            buf.extend_from_slice(&app.prev_id.to_le_bytes());
            buf.push(app.prev_to_here_how as u8);
            buf.extend_from_slice(&app.next_id.to_le_bytes());
            buf.push(app.here_to_next_how as u8);
        }

        buf.extend_from_slice(&(area.encounters.len() as u32).to_le_bytes());
        for enc in &area.encounters {
            buf.extend_from_slice(&enc.from_id.to_le_bytes());
            buf.push(enc.from_dir as u8);
            buf.extend_from_slice(&enc.to_id.to_le_bytes());
            buf.push(enc.to_dir as u8);
            let spot_len = enc.spots.len().min(255);
            buf.push(spot_len as u8);
            for order in enc.spots.iter().take(spot_len) {
                buf.extend_from_slice(&order.spot_id.to_le_bytes());
                let t_byte = (order.t.clamp(0.0, 1.0) * 255.0).round() as u8;
                buf.push(t_byte);
            }
        }

        if area.place_id as usize > place_dict_len {
            return Err(NavError::InvalidPlaceEntry {
                entry: area.place_id,
                dict_len: place_dict_len,
            });
        }
        buf.extend_from_slice(&area.place_id.to_le_bytes());
        Ok(())
    }
}

fn write_vec3(buf: &mut Vec<u8>, v: Vec3) {
    buf.extend_from_slice(&v.x.to_le_bytes());
    buf.extend_from_slice(&v.y.to_le_bytes());
    buf.extend_from_slice(&v.z.to_le_bytes());
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    fn read_bytes(&mut self, len: usize, label: &'static str) -> Result<&'a [u8], NavError> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or(NavError::Truncated(label))?;
        let slice = self
            .bytes
            .get(self.pos..end)
            .ok_or(NavError::Truncated(label))?;
        self.pos = end;
        Ok(slice)
    }

    fn read_u8(&mut self, label: &'static str) -> Result<u8, NavError> {
        Ok(self.read_bytes(1, label)?[0])
    }

    fn read_u16(&mut self, label: &'static str) -> Result<u16, NavError> {
        let b = self.read_bytes(2, label)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn read_u32(&mut self, label: &'static str) -> Result<u32, NavError> {
        let b = self.read_bytes(4, label)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_f32(&mut self, label: &'static str) -> Result<f32, NavError> {
        let b = self.read_bytes(4, label)?;
        let val = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        if !val.is_finite() {
            return Err(NavError::NonFiniteFloat(label));
        }
        Ok(val)
    }

    fn read_vec3(&mut self, label: &'static str) -> Result<Vec3, NavError> {
        Ok(Vec3::new(
            self.read_f32(label)?,
            self.read_f32(label)?,
            self.read_f32(label)?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_nav_file() -> NavFile {
        NavFile {
            version: NAV_VERSION,
            bsp_size: 1_234_567,
            places: vec!["BombsiteA".to_string(), "CTSpawn".to_string()],
            areas: vec![
                NavArea {
                    id: 1,
                    attributes: NavAttribute::CROUCH | NavAttribute::PRECISE,
                    nw_corner: Vec3::new(0.0, 0.0, 10.0),
                    se_corner: Vec3::new(100.0, 100.0, 30.0),
                    ne_z: 20.0,
                    sw_z: 20.0,
                    connections: [vec![], vec![2], vec![], vec![]],
                    hiding_spots: vec![HidingSpot {
                        id: 10,
                        pos: Vec3::new(15.0, 15.0, 13.0),
                        flags: HidingSpotFlags::IN_COVER | HidingSpotFlags::GOOD_SNIPER_SPOT,
                    }],
                    approach_spots: vec![ApproachSpot {
                        here_id: 1,
                        prev_id: 0,
                        prev_to_here_how: NavTraverseType::None,
                        next_id: 2,
                        here_to_next_how: NavTraverseType::GoEast,
                    }],
                    encounters: vec![SpotEncounter {
                        from_id: 1,
                        from_dir: NavDir::West,
                        to_id: 2,
                        to_dir: NavDir::East,
                        spots: vec![SpotOrder {
                            spot_id: 10,
                            t: 128.0 / 255.0,
                        }],
                    }],
                    place_id: 1,
                    ladders_up: Vec::new(),
                    ladders_down: Vec::new(),
                },
                NavArea {
                    id: 2,
                    attributes: NavAttribute::JUMP | NavAttribute::NO_JUMP,
                    nw_corner: Vec3::new(100.0, 0.0, 20.0),
                    se_corner: Vec3::new(200.0, 100.0, 30.0),
                    ne_z: 20.0,
                    sw_z: 30.0,
                    connections: [vec![], vec![], vec![], vec![1]],
                    hiding_spots: Vec::new(),
                    approach_spots: Vec::new(),
                    encounters: Vec::new(),
                    place_id: 2,
                    ladders_up: Vec::new(),
                    ladders_down: Vec::new(),
                },
            ],
            ladders: Vec::new(),
        }
    }

    #[test]
    fn roundtrip_v5_nav_file() {
        let nav = sample_nav_file();
        let bytes = nav.to_bytes().expect("serialize v5 nav");
        let parsed = NavFile::parse(&bytes).expect("parse serialized v5 nav");
        assert_eq!(parsed.version, NAV_VERSION);
        assert_eq!(parsed.bsp_size, 1_234_567);
        assert_eq!(parsed.places, vec!["BombsiteA", "CTSpawn"]);
        assert_eq!(parsed.areas.len(), 2);
        assert_eq!(parsed.areas[0], nav.areas[0]);
        assert_eq!(parsed.areas[1], nav.areas[1]);
    }

    #[test]
    fn parses_legacy_versions_1_through_4() {
        // v1: magic + version(1) + area_count(1) + area with 1 v1 hiding spot + 1 legacy encounter.
        let mut v1 = Vec::new();
        v1.extend_from_slice(&NAV_MAGIC_NUMBER.to_le_bytes());
        v1.extend_from_slice(&1u32.to_le_bytes());
        v1.extend_from_slice(&1u32.to_le_bytes());
        v1.extend_from_slice(&7u32.to_le_bytes());
        v1.push(NavAttribute::CROUCH.bits() as u8);
        write_vec3(&mut v1, Vec3::new(-50.0, -50.0, 0.0));
        write_vec3(&mut v1, Vec3::new(50.0, 50.0, 0.0));
        v1.extend_from_slice(&0.0f32.to_le_bytes());
        v1.extend_from_slice(&0.0f32.to_le_bytes());
        for _ in 0..4 {
            v1.extend_from_slice(&0u32.to_le_bytes());
        }
        v1.push(1); // 1 hiding spot (v1 = Vec3 only)
        write_vec3(&mut v1, Vec3::new(1.0, 2.0, 3.0));
        v1.push(0); // 0 approach
        v1.extend_from_slice(&1u32.to_le_bytes()); // 1 legacy encounter
        v1.extend_from_slice(&7u32.to_le_bytes());
        v1.extend_from_slice(&8u32.to_le_bytes());
        write_vec3(&mut v1, Vec3::ZERO);
        write_vec3(&mut v1, Vec3::X);
        v1.push(1);
        write_vec3(&mut v1, Vec3::Y);
        v1.extend_from_slice(&0.5f32.to_le_bytes());

        let parsed_v1 = NavFile::parse(&v1).expect("parse v1");
        assert_eq!(parsed_v1.version, 1);
        assert_eq!(parsed_v1.areas[0].hiding_spots.len(), 1);
        assert_eq!(
            parsed_v1.areas[0].hiding_spots[0].flags,
            HidingSpotFlags::IN_COVER
        );

        // v4: includes bsp_size, v2 hiding spots, v3 encounters, no place dictionary.
        let mut v4 = Vec::new();
        v4.extend_from_slice(&NAV_MAGIC_NUMBER.to_le_bytes());
        v4.extend_from_slice(&4u32.to_le_bytes());
        v4.extend_from_slice(&999u32.to_le_bytes());
        v4.extend_from_slice(&1u32.to_le_bytes());
        v4.extend_from_slice(&1u32.to_le_bytes());
        v4.push(0);
        write_vec3(&mut v4, Vec3::new(0.0, 0.0, 0.0));
        write_vec3(&mut v4, Vec3::new(25.0, 25.0, 0.0));
        v4.extend_from_slice(&0.0f32.to_le_bytes());
        v4.extend_from_slice(&0.0f32.to_le_bytes());
        for _ in 0..4 {
            v4.extend_from_slice(&0u32.to_le_bytes());
        }
        v4.push(1);
        v4.extend_from_slice(&5u32.to_le_bytes());
        write_vec3(&mut v4, Vec3::new(10.0, 10.0, 0.0));
        v4.push(HidingSpotFlags::IDEAL_SNIPER_SPOT.bits());
        v4.push(0);
        v4.extend_from_slice(&0u32.to_le_bytes());

        let parsed_v4 = NavFile::parse(&v4).expect("parse v4");
        assert_eq!(parsed_v4.version, 4);
        assert_eq!(parsed_v4.bsp_size, 999);
        assert!(parsed_v4.areas[0].hiding_spots[0].is_ideal_sniper_spot());
    }

    #[test]
    fn z_interpolation_and_closest_point_match_quad() {
        let area = &sample_nav_file().areas[0];
        assert!((area.z_at(0.0, 0.0) - 10.0).abs() < 1e-4);
        assert!((area.z_at(100.0, 0.0) - 20.0).abs() < 1e-4);
        assert!((area.z_at(0.0, 100.0) - 20.0).abs() < 1e-4);
        assert!((area.z_at(100.0, 100.0) - 30.0).abs() < 1e-4);
        assert!((area.z_at(50.0, 50.0) - 20.0).abs() < 1e-4);
        let cp = area.closest_point(Vec3::new(-10.0, 50.0, 0.0));
        assert_eq!(cp, Vec3::new(0.0, 50.0, 15.0));
    }

    #[test]
    fn rejects_malformed_and_oversized_inputs() {
        // Bad magic
        let err = NavFile::parse(&[0, 1, 2, 3, 5, 0, 0, 0]).unwrap_err();
        assert!(matches!(err, NavError::InvalidMagic(_)));

        // Unsupported version 0 and 6
        let mut bad_ver = Vec::new();
        bad_ver.extend_from_slice(&NAV_MAGIC_NUMBER.to_le_bytes());
        bad_ver.extend_from_slice(&6u32.to_le_bytes());
        assert!(matches!(
            NavFile::parse(&bad_ver).unwrap_err(),
            NavError::UnsupportedVersion(6)
        ));

        // Forged huge area_count on tiny buffer (must reject without huge allocation)
        let mut forged = Vec::new();
        forged.extend_from_slice(&NAV_MAGIC_NUMBER.to_le_bytes());
        forged.extend_from_slice(&5u32.to_le_bytes());
        forged.extend_from_slice(&0u32.to_le_bytes()); // bsp_size
        forged.extend_from_slice(&0u16.to_le_bytes()); // place_count
        forged.extend_from_slice(&u32::MAX.to_le_bytes()); // area_count
        assert!(matches!(
            NavFile::parse(&forged).unwrap_err(),
            NavError::CountLimitExceeded { .. }
        ));

        // Truncated valid v5 file
        let valid = sample_nav_file().to_bytes().unwrap();
        assert!(matches!(
            NavFile::parse(&valid[..valid.len() - 5]).unwrap_err(),
            NavError::Truncated(_)
        ));

        // Trailing garbage
        let mut trailing = valid.clone();
        trailing.push(0xAA);
        assert!(matches!(
            NavFile::parse(&trailing).unwrap_err(),
            NavError::TrailingData(1)
        ));

        // Invalid place length 0
        let mut bad_place = Vec::new();
        bad_place.extend_from_slice(&NAV_MAGIC_NUMBER.to_le_bytes());
        bad_place.extend_from_slice(&5u32.to_le_bytes());
        bad_place.extend_from_slice(&0u32.to_le_bytes());
        bad_place.extend_from_slice(&1u16.to_le_bytes());
        bad_place.extend_from_slice(&0u16.to_le_bytes());
        bad_place.extend_from_slice(&[0u8; 4]);
        assert!(matches!(
            NavFile::parse(&bad_place).unwrap_err(),
            NavError::InvalidPlaceLength(0)
        ));
    }
}
