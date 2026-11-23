//! GoldSrc right-handed Z-up camera with exact CS 1.6 mouse sensitivity math (`m_yaw = 0.022`).

use glam::{Mat4, Vec3};

#[derive(Debug, Clone)]
pub struct Camera {
    pub position: Vec3,
    /// Horizontal look angle in degrees (0 = +X, 90 = +Y, CCW around +Z).
    pub yaw_deg: f32,
    /// Vertical look angle in degrees (-89 = up, +89 = down, matching GoldSrc pitch).
    pub pitch_deg: f32,
    /// Roll of the view (the punch angle's roll), degrees.
    pub roll_deg: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            position: Vec3::new(0.0, 0.0, 64.0),
            yaw_deg: 0.0,
            pitch_deg: 0.0,
            roll_deg: 0.0,
        }
    }
}

impl Camera {
    pub fn forward(&self) -> Vec3 {
        let yaw = self.yaw_deg.to_radians();
        let pitch = self.pitch_deg.to_radians();
        Vec3::new(
            yaw.cos() * pitch.cos(),
            yaw.sin() * pitch.cos(),
            -pitch.sin(),
        )
        .normalize()
    }

    /// The up vector of the view: world up turned about the view direction by the roll.
    fn view_up(&self) -> Vec3 {
        if !self.roll_deg.is_finite() || self.roll_deg == 0.0 {
            return Vec3::Z;
        }
        let fwd = self.forward();
        if !fwd.is_finite() || fwd.length_squared() < 1e-6 {
            return Vec3::Z;
        }
        glam::Quat::from_axis_angle(fwd.normalize(), self.roll_deg.to_radians()) * Vec3::Z
    }

    pub fn right(&self) -> Vec3 {
        let yaw = self.yaw_deg.to_radians();
        // Right vector in horizontal XY plane for right-handed Z-up
        Vec3::new(yaw.sin(), -yaw.cos(), 0.0).normalize()
    }

    /// Apply raw mouse delta using exact GoldSrc `sensitivity * m_yaw / m_pitch` degrees per count.
    pub fn apply_mouse_delta(&mut self, dx: f32, dy: f32, sensitivity: f32, m_yaw: f32, m_pitch: f32) {
        if !(dx.is_finite()
            && dy.is_finite()
            && sensitivity.is_finite()
            && m_yaw.is_finite()
            && m_pitch.is_finite())
        {
            return;
        }
        let yaw = if self.yaw_deg.is_finite() { self.yaw_deg } else { 0.0 };
        let pitch = if self.pitch_deg.is_finite() { self.pitch_deg } else { 0.0 };
        self.yaw_deg = (yaw - dx * sensitivity * m_yaw).rem_euclid(360.0);
        self.pitch_deg = (pitch + dy * sensitivity * m_pitch).clamp(-89.0, 89.0);
    }

    pub fn view_matrix(&self) -> Mat4 {
        look_to_rh(self.position, self.forward(), self.view_up())
    }

    /// Rotation-only view matrix for skybox rendering (centered on camera).
    pub fn sky_view_matrix(&self) -> Mat4 {
        look_to_rh(Vec3::ZERO, self.forward(), self.view_up())
    }

    /// Compute wgpu-compatible right-handed perspective matrix (`z in [0, 1]`)
    /// using GoldSrc Hor+ FOV (horizontal `fov_deg` at 4:3 -> constant vertical FOV).
    pub fn proj_matrix(&self, viewport_w: f32, viewport_h: f32, fov_deg: f32, aspect_mode: &str) -> Mat4 {
        let safe_fov = if fov_deg.is_finite() { fov_deg } else { 90.0 };
        let fov_clamped = safe_fov.clamp(10.0, 120.0);
        // Classic 4:3 vertical FOV corresponding to horizontal fov_default (90 deg -> ~73.74 deg)
        let vfov = 2.0 * ((fov_clamped.to_radians() * 0.5).tan() * (3.0 / 4.0)).atan();
        let aspect = if aspect_mode.eq_ignore_ascii_case("4:3_stretch") {
            4.0 / 3.0
        } else if viewport_w.is_finite() && viewport_h.is_finite() {
            (viewport_w.max(1.0) / viewport_h.max(1.0)).max(0.1)
        } else {
            4.0 / 3.0
        };
        perspective_rh_zo(vfov, aspect, 4.0, 16384.0)
    }

    /// Projection matrix with `z_near = 1.0` for first-person weapon viewmodels.
    pub fn viewmodel_proj_matrix(&self, viewport_w: f32, viewport_h: f32, aspect_mode: &str) -> Mat4 {
        let vfov = 2.0 * ((90.0f32.to_radians() * 0.5).tan() * (3.0 / 4.0)).atan();
        let aspect = if aspect_mode.eq_ignore_ascii_case("4:3_stretch") {
            4.0 / 3.0
        } else if viewport_w.is_finite() && viewport_h.is_finite() {
            (viewport_w.max(1.0) / viewport_h.max(1.0)).max(0.1)
        } else {
            4.0 / 3.0
        };
        perspective_rh_zo(vfov, aspect, 1.0, 256.0)
    }
}

/// Right-handed look-to view matrix mapping `dir` to `-Z` and `up` to `+Y` in view space.
fn look_to_rh(eye: Vec3, dir: Vec3, up: Vec3) -> Mat4 {
    let f = dir.normalize();
    let s = f.cross(up).normalize();
    let u = s.cross(f);
    Mat4::from_cols(
        glam::Vec4::new(s.x, u.x, -f.x, 0.0),
        glam::Vec4::new(s.y, u.y, -f.y, 0.0),
        glam::Vec4::new(s.z, u.z, -f.z, 0.0),
        glam::Vec4::new(-s.dot(eye), -u.dot(eye), f.dot(eye), 1.0),
    )
}

/// Right-handed perspective projection mapping depth to `[0, 1]` for `wgpu`.
fn perspective_rh_zo(fov_y_radians: f32, aspect_ratio: f32, z_near: f32, z_far: f32) -> Mat4 {
    let f = 1.0 / (0.5 * fov_y_radians).tan();
    let r = z_far / (z_near - z_far);
    Mat4::from_cols(
        glam::Vec4::new(f / aspect_ratio, 0.0, 0.0, 0.0),
        glam::Vec4::new(0.0, f, 0.0, 0.0),
        glam::Vec4::new(0.0, 0.0, r, -1.0),
        glam::Vec4::new(0.0, 0.0, r * z_near, 0.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_projects_point_in_front_into_wgpu_ndc() {
        let cam = Camera::default();
        let vp = cam.proj_matrix(1280.0, 800.0, 90.0, "widescreen") * cam.view_matrix();
        let target = cam.position + cam.forward() * 100.0;
        let clip = vp * target.extend(1.0);
        let ndc = clip.truncate() / clip.w;
        assert!(ndc.x.abs() < 1e-4);
        assert!(ndc.y.abs() < 1e-4);
        assert!(ndc.z > 0.0 && ndc.z < 1.0);
    }

    #[test]
    fn roll_sign_and_non_finite_guards_hold() {
        let mut cam = Camera {
            roll_deg: 90.0,
            ..Camera::default()
        };
        let up = cam.view_up();
        assert!((up - Vec3::new(0.0, -1.0, 0.0)).length() < 1e-5, "{up:?}");

        cam.apply_mouse_delta(f32::NAN, 10.0, 3.0, 0.022, 0.022);
        assert!(cam.yaw_deg.is_finite() && cam.pitch_deg.is_finite());

        cam.roll_deg = f32::NAN;
        assert_eq!(cam.view_up(), Vec3::Z);
        let vm = cam.view_matrix();
        let pm = cam.proj_matrix(f32::NAN, 800.0, f32::NAN, "widescreen");
        assert!(vm.is_finite() && pm.is_finite());
    }
}
