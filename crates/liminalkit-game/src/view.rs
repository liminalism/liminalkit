//! The camera, as a curved window needs to know it. **EXPERIMENTAL.**
//!
//! This crate owns no camera. The host has one, with its own conventions, and
//! describes it each frame through [`View`]: where a world point lands on
//! screen, and which ray a pixel looks along. [`Pinhole`] is a ready
//! implementation for the usual perspective camera, built from five vectors
//! and an angle the host already has.

use crate::math::{mix, mix_v3, V3};

/// A half-line: where it starts and its unit direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    pub origin: V3,
    pub dir: V3,
}

impl Ray {
    pub fn at(self, t: f64) -> V3 {
        self.origin + self.dir * t
    }
}

/// A world point on screen: device pixels (top-left origin, pixel `(i, j)`
/// centred at `(i + 0.5, j + 0.5)`) and its distance in front of the viewer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Projected {
    pub px: [f64; 2],
    pub depth: f64,
}

/// The host's camera, and nothing else: no planet, no occlusion.
pub trait View {
    /// The size of the target, in device pixels.
    fn viewport(&self) -> [u32; 2];
    /// Where world point `p` lands. `None` behind the viewer or not finite.
    fn project(&self, p: V3) -> Option<Projected>;
    /// The ray through a pixel position.
    fn ray(&self, px: [f64; 2]) -> Option<Ray>;
    /// Unit vector from `p` toward the viewer.
    fn toward_viewer(&self, p: V3) -> V3;
    /// Metres one pixel covers on a surface facing the viewer at `p`.
    fn footprint_m(&self, p: V3) -> f64;
    /// A value that changes whenever any answer above would. Meshes are
    /// rebuilt only when it does, so a wrong one shows as a window that
    /// stops following the camera.
    fn fingerprint(&self) -> u64;
}

/// A perspective camera: an eye, an orthonormal basis and a field of view.
///
/// `right`, `up` and `back` are the camera's axes in world space (`back`
/// points from the scene toward the viewer, so the camera looks along
/// `-back`). For a camera whose orientation is a quaternion `q`, they are
/// `q * X`, `q * Y` and `q * Z`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pinhole {
    pub eye: V3,
    pub right: V3,
    pub up: V3,
    pub back: V3,
    /// `tan(fov_y / 2)`.
    pub tan_half_fov_y: f64,
    pub viewport: [u32; 2],
}

impl Pinhole {
    /// A camera under which the plane `z = 0` maps one to one onto pixels
    /// (world x right, y down, z into the screen), seen from `perspective_px`
    /// in front of its centre — the CSS `perspective` model. For hosts with
    /// no 3D scene that still want a panel to tilt or wrap a cylinder.
    pub fn css(viewport: [u32; 2], perspective_px: f64) -> Pinhole {
        let (w, h) = (f64::from(viewport[0]), f64::from(viewport[1]));
        let distance = perspective_px.max(1.0);
        Pinhole {
            eye: V3::new(w / 2.0, h / 2.0, -distance),
            right: V3::X,
            up: -V3::Y,
            back: -V3::Z,
            tan_half_fov_y: (h / 2.0).max(0.5) / distance,
            viewport,
        }
    }

    /// Pixels per unit of `tan` from the optical axis.
    fn focal(&self) -> f64 {
        f64::from(self.viewport[1]) / 2.0 / self.tan_half_fov_y
    }
}

impl View for Pinhole {
    fn viewport(&self) -> [u32; 2] {
        self.viewport
    }

    fn project(&self, p: V3) -> Option<Projected> {
        let d = p - self.eye;
        let depth = -d.dot(self.back);
        if !(depth.is_finite() && depth > 1e-9) {
            return None;
        }
        let focal = self.focal();
        let px = [
            f64::from(self.viewport[0]) / 2.0 + d.dot(self.right) / depth * focal,
            f64::from(self.viewport[1]) / 2.0 - d.dot(self.up) / depth * focal,
        ];
        (px[0].is_finite() && px[1].is_finite()).then_some(Projected { px, depth })
    }

    fn ray(&self, px: [f64; 2]) -> Option<Ray> {
        let focal = self.focal();
        let x = (px[0] - f64::from(self.viewport[0]) / 2.0) / focal;
        let y = -(px[1] - f64::from(self.viewport[1]) / 2.0) / focal;
        let dir = (self.right * x + self.up * y - self.back).try_normalize()?;
        self.eye.is_finite().then_some(Ray {
            origin: self.eye,
            dir,
        })
    }

    fn toward_viewer(&self, p: V3) -> V3 {
        (self.eye - p).try_normalize().unwrap_or(self.back)
    }

    fn footprint_m(&self, p: V3) -> f64 {
        let depth = -(p - self.eye).dot(self.back);
        depth.max(0.0) / self.focal()
    }

    fn fingerprint(&self) -> u64 {
        let mut f = mix_v3(0x51_6e_68_6f, self.eye);
        for v in [self.right, self.up, self.back] {
            f = mix_v3(f, v);
        }
        f = mix(f, self.tan_half_fov_y.to_bits());
        mix(
            f,
            u64::from(self.viewport[0]) << 32 | u64::from(self.viewport[1]),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> Pinhole {
        // Looking down -z from (1, 2, 10), 55° tall, like the game it is for.
        Pinhole {
            eye: V3::new(1.0, 2.0, 10.0),
            right: V3::X,
            up: V3::Y,
            back: V3::Z,
            tan_half_fov_y: (27.5_f64).to_radians().tan(),
            viewport: [1280, 720],
        }
    }

    #[test]
    fn the_ray_through_a_projected_point_passes_through_it() {
        let view = camera();
        for p in [
            V3::new(0.0, 0.0, 0.0),
            V3::new(3.0, -2.5, -4.0),
            V3::new(-6.0, 4.0, 2.0),
        ] {
            let on_screen = view.project(p).expect("in front");
            let ray = view.ray(on_screen.px).expect("a ray");
            let along = (p - ray.origin).dot(ray.dir);
            let miss = (ray.at(along) - p).length();
            assert!(miss < 1e-9, "{miss}");
            assert!((on_screen.depth - (view.eye.z - p.z)).abs() < 1e-12);
        }
    }

    #[test]
    fn the_optical_axis_is_the_centre_and_y_runs_down() {
        let view = camera();
        let centre = view.project(V3::new(1.0, 2.0, 0.0)).unwrap();
        assert!((centre.px[0] - 640.0).abs() < 1e-9 && (centre.px[1] - 360.0).abs() < 1e-9);
        let above = view.project(V3::new(1.0, 3.0, 0.0)).unwrap();
        assert!(above.px[1] < centre.px[1], "world up is screen up");
        let right = view.project(V3::new(2.0, 2.0, 0.0)).unwrap();
        assert!(right.px[0] > centre.px[0]);
    }

    #[test]
    fn what_is_behind_or_not_a_number_does_not_project() {
        let view = camera();
        assert!(view.project(V3::new(0.0, 0.0, 11.0)).is_none());
        assert!(view.project(V3::new(0.0, 0.0, 10.0)).is_none());
        assert!(view.project(V3::new(f64::NAN, 0.0, 0.0)).is_none());
        assert!(view.project(V3::new(f64::INFINITY, 0.0, 0.0)).is_none());
    }

    #[test]
    fn a_pixel_footprint_is_the_size_that_projects_to_one_pixel() {
        let view = camera();
        let p = V3::new(1.0, 2.0, -30.0);
        let size = view.footprint_m(p);
        let a = view.project(p).unwrap();
        let b = view.project(p + V3::X * size).unwrap();
        assert!((b.px[0] - a.px[0] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn the_css_camera_maps_its_plane_onto_pixels_exactly() {
        let view = Pinhole::css([800, 600], 1200.0);
        for (x, y) in [(0.0, 0.0), (800.0, 600.0), (123.5, 456.25)] {
            let p = view.project(V3::new(x, y, 0.0)).unwrap();
            assert!((p.px[0] - x).abs() < 1e-9 && (p.px[1] - y).abs() < 1e-9);
            assert!((view.footprint_m(V3::new(x, y, 0.0)) - 1.0).abs() < 1e-12);
        }
        // Further away is smaller, toward the centre.
        let far = view.project(V3::new(0.0, 0.0, 1200.0)).unwrap();
        assert!((far.px[0] - 200.0).abs() < 1e-9 && (far.px[1] - 150.0).abs() < 1e-9);
    }

    #[test]
    fn the_fingerprint_follows_every_field() {
        let base = camera();
        let mut moved = base;
        moved.eye.x += 1e-6;
        let mut turned = base;
        turned.right = V3::new(0.0, 0.0, -1.0);
        let mut zoomed = base;
        zoomed.tan_half_fov_y *= 1.01;
        let mut resized = base;
        resized.viewport = [720, 1280];
        assert_eq!(base.fingerprint(), camera().fingerprint());
        for other in [moved, turned, zoomed, resized] {
            assert_ne!(base.fingerprint(), other.fingerprint());
        }
    }
}
