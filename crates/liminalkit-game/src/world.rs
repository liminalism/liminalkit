//! Surfaces a window can lie on: a sphere, a cylinder, a plane.
//! **EXPERIMENTAL.**
//!
//! A window is a flat sheet. Laying it on a surface needs one thing from the
//! surface: a way to carry a point of the sheet, given as an offset in metres
//! from the sheet's anchor, to a point of the surface, and back. That pair is
//! [`Surface3::exp`] and [`Surface3::log`], named for the exponential map
//! they are: walk from the anchor along the surface, in the offset's
//! direction, for the offset's length.
//!
//! The choice of that map over a projection is what makes a window behave
//! like a sticker. Distances from the anchor along the surface are exact, so
//! unrolling the sheet about any point is well defined; the inverse is closed
//! form; and the map commutes with moving the surface rigidly, so dragging a
//! window is one rotation of its [`Frame`] and the point under the cursor
//! stays under the cursor. On a sphere the only distortion is across the
//! direction of travel, by `sin θ / θ`: a third of a percent at five degrees
//! from the anchor.
//!
//! Surfaces are described in their own *body* coordinates and carried into
//! the world by a [`Pose`] the host supplies each frame, so a window pinned to
//! a turning planet turns with it and nothing here knows about time.

use crate::math::{mix, mix_m3, mix_v3, M3, V3};
use crate::view::Ray;

/// Where a body sits in the world: `world = rotation × body + origin`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub origin: V3,
    pub rotation: M3,
}

impl Pose {
    pub const IDENTITY: Pose = Pose {
        origin: V3::ZERO,
        rotation: M3::IDENTITY,
    };

    pub fn to_world(self, body: V3) -> V3 {
        self.rotation.mul_vec(body) + self.origin
    }

    pub fn to_body(self, world: V3) -> V3 {
        self.rotation.transpose().mul_vec(world - self.origin)
    }

    pub fn dir_to_world(self, body: V3) -> V3 {
        self.rotation.mul_vec(body)
    }

    pub fn dir_to_body(self, world: V3) -> V3 {
        self.rotation.transpose().mul_vec(world)
    }

    fn fingerprint(self, seed: u64) -> u64 {
        mix_m3(mix_v3(seed, self.origin), self.rotation)
    }
}

/// A window's anchor on a surface, in body coordinates: a point of the
/// surface and an orthonormal basis there. `x` is the window's right, `y` its
/// up, `n` the side it is read from (`x × y = n`).
///
/// A frame rather than a latitude, longitude and heading: it has no pole, and
/// moving the window is a rotation of the whole frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub origin: V3,
    pub x: V3,
    pub y: V3,
    pub n: V3,
}

/// A point of a surface, in body coordinates, and the surface's normal there
/// (on the side windows are read from).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfacePoint {
    pub pos: V3,
    pub normal: V3,
}

/// A surface windows can be laid on. Everything is in body coordinates except
/// [`Surface3::occludes`].
pub trait Surface3 {
    fn pose(&self) -> Pose;
    /// The point `d` metres from `frame`'s anchor along the surface (`d[0]`
    /// along the frame's x, `d[1]` along its y), raised `lift_m` off it.
    fn exp(&self, frame: &Frame, d: [f64; 2], lift_m: f64) -> SurfacePoint;
    /// The inverse of [`Surface3::exp`] for a point on the surface. `None`
    /// where it has no answer (the far side of a sphere from the anchor).
    fn log(&self, frame: &Frame, p: V3) -> Option<[f64; 2]>;
    /// The nearest point ahead of a ray on the surface raised by `lift_m`, as
    /// a distance along the ray.
    fn intersect(&self, ray: Ray, lift_m: f64) -> Option<f64>;
    /// `frame` moved rigidly along the surface so that surface point `from`
    /// arrives at `to`. `None` when the move is not defined (antipodes).
    fn slide(&self, frame: &Frame, from: V3, to: V3) -> Option<Frame>;
    /// A frame anchored at surface point `p` whose up is `up_hint` flattened
    /// into the surface. `None` if the hint has no component along it.
    fn frame_at(&self, p: V3, up_hint: V3) -> Option<Frame>;
    /// Whether the surface itself hides *world* point `p` from a viewer in
    /// direction `toward_viewer`: a point lifted off a sphere, past its limb.
    fn occludes(&self, p_world: V3, toward_viewer: V3) -> bool;
    /// The furthest from its anchor, in metres, any part of a window may be.
    fn max_extent_m(&self) -> f64;
    /// Changes whenever any answer above would.
    fn fingerprint(&self) -> u64;
}

/// `sin θ / θ`, by series near zero where the quotient loses its digits.
fn sinc(theta: f64) -> f64 {
    if theta.abs() < 1e-4 {
        let t2 = theta * theta;
        1.0 - t2 / 6.0 + t2 * t2 / 120.0
    } else {
        theta.sin() / theta
    }
}

/// The nearest `t > min_t` at which `origin + t × dir` meets a sphere of
/// `radius` about the coordinate origin. The roots are formed by product, not
/// by subtracting nearly equal numbers, so a grazing ray from far away keeps
/// its precision.
fn ray_sphere(origin: V3, dir: V3, radius: f64, min_t: f64) -> Option<f64> {
    let b = origin.dot(dir);
    let c = origin.dot(origin) - radius * radius;
    let discriminant = b * b - c;
    if discriminant.is_nan() || discriminant < 0.0 {
        return None;
    }
    let root = discriminant.sqrt();
    let (near, far) = if b < 0.0 {
        let q = -b + root;
        (if q != 0.0 { c / q } else { 0.0 }, q)
    } else {
        let q = -b - root;
        (q, if q != 0.0 { c / q } else { 0.0 })
    };
    [near, far]
        .into_iter()
        .find(|&t| t.is_finite() && t > min_t)
}

/// A sphere centred on its body origin: a planet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sphere {
    pub radius_m: f64,
    pub pose: Pose,
}

impl Surface3 for Sphere {
    fn pose(&self) -> Pose {
        self.pose
    }

    fn exp(&self, frame: &Frame, d: [f64; 2], lift_m: f64) -> SurfacePoint {
        let theta = d[0].hypot(d[1]) / self.radius_m;
        let tangent = frame.x * d[0] + frame.y * d[1];
        let unit = frame.n * theta.cos() + tangent * (sinc(theta) / self.radius_m);
        // Rounding leaves `unit` a hair off length one; the surface point
        // must be exactly on the sphere for `log` to return what went in.
        let unit = unit.try_normalize().unwrap_or(frame.n);
        SurfacePoint {
            pos: unit * (self.radius_m + lift_m),
            normal: unit,
        }
    }

    fn log(&self, frame: &Frame, p: V3) -> Option<[f64; 2]> {
        let q = p.try_normalize()?;
        let along = q.dot(frame.n);
        let across = q - frame.n * along;
        let s = across.length();
        if s < 1e-15 {
            return (along > 0.0).then_some([0.0, 0.0]);
        }
        let scale = self.radius_m * s.atan2(along) / s;
        Some([across.dot(frame.x) * scale, across.dot(frame.y) * scale])
    }

    fn intersect(&self, ray: Ray, lift_m: f64) -> Option<f64> {
        ray_sphere(ray.origin, ray.dir, self.radius_m + lift_m, 0.0)
    }

    fn slide(&self, frame: &Frame, from: V3, to: V3) -> Option<Frame> {
        let (a, b) = (from.try_normalize()?, to.try_normalize()?);
        let axis = a.cross(b);
        let (s, c) = (axis.length(), a.dot(b));
        if s < 1e-15 {
            return (c > 0.0).then_some(*frame);
        }
        let turn = M3::rodrigues(axis * (1.0 / s), s.atan2(c));
        // Re-square the basis every time: a drag is thousands of these.
        let n = turn.mul_vec(frame.n).try_normalize()?;
        let x = turn.mul_vec(frame.x);
        let x = (x - n * x.dot(n)).try_normalize()?;
        Some(Frame {
            origin: n * self.radius_m,
            x,
            y: n.cross(x),
            n,
        })
    }

    fn frame_at(&self, p: V3, up_hint: V3) -> Option<Frame> {
        let n = p.try_normalize()?;
        let y = (up_hint - n * up_hint.dot(n)).try_normalize()?;
        Some(Frame {
            origin: n * self.radius_m,
            x: y.cross(n),
            y,
            n,
        })
    }

    fn occludes(&self, p_world: V3, toward_viewer: V3) -> bool {
        // Slightly inside the surface, so a point resting on it is not hidden
        // by the ground it rests on.
        let radius = self.radius_m * (1.0 - 1e-9);
        ray_sphere(p_world - self.pose.origin, toward_viewer, radius, 0.0).is_some()
    }

    fn max_extent_m(&self) -> f64 {
        0.8 * std::f64::consts::FRAC_PI_2 * self.radius_m
    }

    fn fingerprint(&self) -> u64 {
        self.pose
            .fingerprint(mix(0x73_70_68, self.radius_m.to_bits()))
    }
}

/// An endless cylinder about its body z axis: a curved wall of instruments.
/// With `inside` the viewer stands within it and windows are read from the
/// axis side.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cylinder {
    pub radius_m: f64,
    pub pose: Pose,
    pub inside: bool,
}

impl Cylinder {
    /// Outward radial unit vector at `p`; `None` on the axis.
    fn radial(p: V3) -> Option<V3> {
        V3::new(p.x, p.y, 0.0).try_normalize()
    }

    fn facing(&self) -> f64 {
        if self.inside {
            -1.0
        } else {
            1.0
        }
    }

    /// The radius a point lifted `lift_m` toward the reader sits at.
    fn lifted(&self, lift_m: f64) -> f64 {
        (self.radius_m + self.facing() * lift_m).max(0.0)
    }

    /// The nearest `t > 0` at which a ray meets the cylinder of `radius`.
    fn ray(&self, origin: V3, dir: V3, radius: f64) -> Option<f64> {
        let a = dir.x * dir.x + dir.y * dir.y;
        if a < 1e-300 {
            return None;
        }
        let b = origin.x * dir.x + origin.y * dir.y;
        let c = origin.x * origin.x + origin.y * origin.y - radius * radius;
        let discriminant = b * b - a * c;
        if discriminant.is_nan() || discriminant < 0.0 {
            return None;
        }
        let root = discriminant.sqrt();
        [(-b - root) / a, (-b + root) / a]
            .into_iter()
            .find(|&t| t.is_finite() && t > 1e-9)
    }
}

impl Surface3 for Cylinder {
    fn pose(&self) -> Pose {
        self.pose
    }

    fn exp(&self, frame: &Frame, d: [f64; 2], lift_m: f64) -> SurfacePoint {
        // A cylinder unrolls flat without stretching: split the offset into
        // its part along the axis and its part round it.
        let outward = Self::radial(frame.origin).unwrap_or(V3::X);
        let round = V3::Z.cross(outward);
        let tangent = frame.x * d[0] + frame.y * d[1];
        let turn = M3::rodrigues(V3::Z, tangent.dot(round) / self.radius_m);
        let radial = turn.mul_vec(outward);
        SurfacePoint {
            pos: radial * self.lifted(lift_m) + V3::Z * (frame.origin.z + tangent.dot(V3::Z)),
            normal: radial * self.facing(),
        }
    }

    fn log(&self, frame: &Frame, p: V3) -> Option<[f64; 2]> {
        let outward = Self::radial(frame.origin)?;
        let radial = Self::radial(p)?;
        let round = V3::Z.cross(outward);
        let angle = radial.dot(round).atan2(radial.dot(outward));
        let tangent = round * (angle * self.radius_m) + V3::Z * (p.z - frame.origin.z);
        Some([tangent.dot(frame.x), tangent.dot(frame.y)])
    }

    fn intersect(&self, ray: Ray, lift_m: f64) -> Option<f64> {
        self.ray(ray.origin, ray.dir, self.lifted(lift_m))
    }

    fn slide(&self, frame: &Frame, from: V3, to: V3) -> Option<Frame> {
        let (a, b) = (Self::radial(from)?, Self::radial(to)?);
        let angle = a.cross(b).z.atan2(a.dot(b));
        let turn = M3::rodrigues(V3::Z, angle);
        let outward = turn.mul_vec(Self::radial(frame.origin)?);
        Some(Frame {
            origin: outward * self.radius_m + V3::Z * (frame.origin.z + to.z - from.z),
            x: turn.mul_vec(frame.x),
            y: turn.mul_vec(frame.y),
            n: outward * self.facing(),
        })
    }

    fn frame_at(&self, p: V3, up_hint: V3) -> Option<Frame> {
        let outward = Self::radial(p)?;
        let n = outward * self.facing();
        let y = (up_hint - n * up_hint.dot(n)).try_normalize()?;
        Some(Frame {
            origin: outward * self.radius_m + V3::Z * p.z,
            x: y.cross(n),
            y,
            n,
        })
    }

    fn occludes(&self, p_world: V3, toward_viewer: V3) -> bool {
        if self.inside {
            return false;
        }
        let radius = self.radius_m * (1.0 - 1e-9);
        self.ray(
            self.pose.to_body(p_world),
            self.pose.dir_to_body(toward_viewer),
            radius,
        )
        .is_some()
    }

    fn max_extent_m(&self) -> f64 {
        0.8 * std::f64::consts::PI * self.radius_m
    }

    fn fingerprint(&self) -> u64 {
        let seed = mix(0x63_79_6c, self.radius_m.to_bits());
        self.pose.fingerprint(mix(seed, u64::from(self.inside)))
    }
}

/// The plane `z = 0` of its body, read from `+z`: a flat panel that can still
/// be tilted in perspective by posing it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane {
    pub pose: Pose,
}

impl Surface3 for Plane {
    fn pose(&self) -> Pose {
        self.pose
    }

    fn exp(&self, frame: &Frame, d: [f64; 2], lift_m: f64) -> SurfacePoint {
        SurfacePoint {
            pos: frame.origin + frame.x * d[0] + frame.y * d[1] + V3::Z * lift_m,
            normal: V3::Z,
        }
    }

    fn log(&self, frame: &Frame, p: V3) -> Option<[f64; 2]> {
        let offset = p - frame.origin;
        Some([offset.dot(frame.x), offset.dot(frame.y)])
    }

    fn intersect(&self, ray: Ray, lift_m: f64) -> Option<f64> {
        let t = (lift_m - ray.origin.z) / ray.dir.z;
        (t.is_finite() && t > 0.0).then_some(t)
    }

    fn slide(&self, frame: &Frame, from: V3, to: V3) -> Option<Frame> {
        let moved = to - from;
        Some(Frame {
            origin: frame.origin + V3::new(moved.x, moved.y, 0.0),
            ..*frame
        })
    }

    fn frame_at(&self, p: V3, up_hint: V3) -> Option<Frame> {
        let y = V3::new(up_hint.x, up_hint.y, 0.0).try_normalize()?;
        Some(Frame {
            origin: V3::new(p.x, p.y, 0.0),
            x: y.cross(V3::Z),
            y,
            n: V3::Z,
        })
    }

    fn occludes(&self, _p_world: V3, _toward_viewer: V3) -> bool {
        false
    }

    fn max_extent_m(&self) -> f64 {
        f64::INFINITY
    }

    fn fingerprint(&self) -> u64 {
        self.pose.fingerprint(0x70_6c_6e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: f64 = 6_371_000.0;

    fn planet() -> Sphere {
        Sphere {
            radius_m: R,
            pose: Pose::IDENTITY,
        }
    }

    fn tilted_frame(surface: &Sphere) -> Frame {
        let p = V3::new(0.3, 0.5, 0.8).try_normalize().unwrap() * R;
        surface.frame_at(p, V3::new(0.2, 1.0, -0.1)).unwrap()
    }

    fn assert_orthonormal(f: &Frame) {
        for (a, b, expect) in [
            (f.x, f.x, 1.0),
            (f.y, f.y, 1.0),
            (f.n, f.n, 1.0),
            (f.x, f.y, 0.0),
            (f.x, f.n, 0.0),
            (f.y, f.n, 0.0),
        ] {
            assert!((a.dot(b) - expect).abs() < 1e-12);
        }
        assert!((f.x.cross(f.y) - f.n).length() < 1e-12, "right-handed");
    }

    #[test]
    fn a_frame_is_square_and_sits_on_the_surface() {
        let frame = tilted_frame(&planet());
        assert_orthonormal(&frame);
        assert!((frame.origin.length() - R).abs() < 1e-6);
        assert!((frame.origin * (1.0 / R) - frame.n).length() < 1e-12);
        assert!(
            planet().frame_at(V3::Y * R, V3::Y).is_none(),
            "no up along the normal"
        );
        assert!(planet().frame_at(V3::ZERO, V3::Y).is_none());
    }

    #[test]
    fn the_sphere_map_inverts_from_a_hair_to_most_of_the_way_round() {
        let globe = planet();
        let frame = tilted_frame(&globe);
        for theta in [
            0.0,
            1e-9,
            1e-5,
            0.01,
            0.5,
            1.5,
            2.5,
            0.9 * std::f64::consts::PI,
        ] {
            for bearing in [0.0_f64, 0.7, 2.0, 4.1] {
                let d = [theta * R * bearing.cos(), theta * R * bearing.sin()];
                let point = globe.exp(&frame, d, 0.0);
                assert!((point.pos.length() - R).abs() < 1e-6);
                let back = globe.log(&frame, point.pos).expect("not the antipode");
                let error = (back[0] - d[0]).hypot(back[1] - d[1]);
                assert!(
                    error < 1e-6 * (1.0 + theta * R * 1e-6),
                    "{theta} {bearing}: {error}"
                );
            }
        }
        assert_eq!(globe.log(&frame, frame.origin), Some([0.0, 0.0]));
        assert!(globe.log(&frame, -frame.origin).is_none(), "the antipode");
    }

    #[test]
    fn distance_from_the_anchor_along_the_surface_is_the_offset_length() {
        let globe = planet();
        let frame = tilted_frame(&globe);
        let d = [900_000.0, -400_000.0];
        let point = globe.exp(&frame, d, 0.0);
        let angle = (point.pos * (1.0 / R)).dot(frame.n).acos();
        assert!((angle * R - d[0].hypot(d[1])).abs() < 1e-3);
        // Lift moves along the normal only.
        let lifted = globe.exp(&frame, d, 5_000.0);
        assert!((lifted.pos - point.pos - point.normal * 5_000.0).length() < 1e-6);
    }

    #[test]
    fn the_map_commutes_with_turning_the_planet() {
        // The property that makes a drag one rotation.
        let globe = planet();
        let frame = tilted_frame(&globe);
        let turn = M3::rodrigues(V3::new(1.0, -2.0, 0.5).try_normalize().unwrap(), 0.9);
        let turned = Frame {
            origin: turn.mul_vec(frame.origin),
            x: turn.mul_vec(frame.x),
            y: turn.mul_vec(frame.y),
            n: turn.mul_vec(frame.n),
        };
        let d = [250_000.0, 120_000.0];
        let a = turn.mul_vec(globe.exp(&frame, d, 0.0).pos);
        let b = globe.exp(&turned, d, 0.0).pos;
        assert!((a - b).length() < 1e-6);
    }

    #[test]
    fn sliding_carries_the_grabbed_point_to_the_target_exactly() {
        let globe = planet();
        let frame = tilted_frame(&globe);
        let grab = [140_000.0, -60_000.0];
        let from = globe.exp(&frame, grab, 0.0).pos;
        let to = V3::new(-0.2, 0.6, 0.77).try_normalize().unwrap() * R;
        let slid = globe.slide(&frame, from, to).unwrap();
        assert_orthonormal(&slid);
        assert!((globe.exp(&slid, grab, 0.0).pos - to).length() < 1e-6);
        // No move at all is no change; to the antipode there is no answer.
        assert_eq!(globe.slide(&frame, from, from), Some(frame));
        assert!(globe.slide(&frame, from, -from).is_none());
    }

    #[test]
    fn a_long_drag_does_not_skew_the_frame() {
        let globe = planet();
        let mut frame = tilted_frame(&globe);
        for step in 0..10_000 {
            let from = frame.origin;
            let wobble = (f64::from(step) * 0.37).sin();
            let to = globe
                .exp(&frame, [800.0 + 300.0 * wobble, 500.0 * wobble], 0.0)
                .pos;
            frame = globe.slide(&frame, from, to).unwrap();
        }
        assert_orthonormal(&frame);
        assert!((frame.origin.length() - R).abs() < 1e-6);
    }

    #[test]
    fn a_ray_finds_the_near_side_and_keeps_its_precision_when_grazing() {
        let globe = planet();
        let eye = V3::new(0.0, 0.0, R + 400_000.0);
        let down = Ray {
            origin: eye,
            dir: -V3::Z,
        };
        assert!((globe.intersect(down, 0.0).unwrap() - 400_000.0).abs() < 1e-6);
        assert!((globe.intersect(down, 1_000.0).unwrap() - 399_000.0).abs() < 1e-6);
        // Aim at a point near the limb: the hit must be that point.
        let target = globe.exp(
            &globe.frame_at(V3::Z * R, V3::Y).unwrap(),
            [0.33 * R, 0.0],
            0.0,
        );
        let dir = (target.pos - eye).try_normalize().unwrap();
        let t = globe.intersect(Ray { origin: eye, dir }, 0.0).unwrap();
        assert!((eye + dir * t - target.pos).length() < 1e-5);
        let away = Ray {
            origin: eye,
            dir: V3::Z,
        };
        assert!(globe.intersect(away, 0.0).is_none());
        let wide = Ray {
            origin: eye,
            dir: V3::X,
        };
        assert!(globe.intersect(wide, 0.0).is_none());
    }

    #[test]
    fn the_planet_hides_what_is_lifted_behind_its_limb_and_nothing_in_front() {
        let globe = planet();
        let eye = V3::new(0.0, 0.0, R * 3.0);
        let toward = |p: V3| (eye - p).try_normalize().unwrap();
        let front = V3::Z * R;
        assert!(
            !globe.occludes(front, toward(front)),
            "resting on the near side"
        );
        let lifted = V3::Z * (R + 10_000.0);
        assert!(!globe.occludes(lifted, toward(lifted)));
        let behind = V3::new(0.5 * R, 0.0, -R * 1.2);
        assert!(globe.occludes(behind, toward(behind)));
        // Its foot on the ground is past the horizon (which is at z = R / 3
        // from here), but it stands high enough to be seen over it.
        let over_the_limb = V3::new(R * 1.05, 0.0, R * 0.25);
        assert!(
            !globe.occludes(over_the_limb, toward(over_the_limb)),
            "peeking over"
        );
    }

    #[test]
    fn a_cylinder_unrolls_without_stretching_at_any_heading() {
        let wall = Cylinder {
            radius_m: 2.0,
            pose: Pose::IDENTITY,
            inside: true,
        };
        let frame = wall
            .frame_at(V3::new(2.0, 0.0, 0.3), V3::new(0.0, 0.4, 1.0))
            .unwrap();
        assert!(
            (frame.n - -V3::X).length() < 1e-12,
            "read from the axis side"
        );
        assert!((frame.x.cross(frame.y) - frame.n).length() < 1e-12);
        let (a, b) = ([0.7, -0.2], [-0.4, 0.5]);
        let (pa, pb) = (wall.exp(&frame, a, 0.0), wall.exp(&frame, b, 0.0));
        for p in [pa, pb] {
            assert!((p.pos.x.hypot(p.pos.y) - 2.0).abs() < 1e-12);
        }
        for (d, p) in [(a, pa), (b, pb)] {
            let back = wall.log(&frame, p.pos).unwrap();
            assert!((back[0] - d[0]).hypot(back[1] - d[1]) < 1e-12);
        }
        // Lift goes toward the reader, who is inside.
        let lifted = wall.exp(&frame, a, 0.25);
        assert!((lifted.pos.x.hypot(lifted.pos.y) - 1.75).abs() < 1e-12);
        // A ray from the axis hits the wall at its radius.
        let ray = Ray {
            origin: V3::new(0.0, 0.0, 0.3),
            dir: V3::X,
        };
        assert!((wall.intersect(ray, 0.0).unwrap() - 2.0).abs() < 1e-12);
        assert!(!wall.occludes(pa.pos, V3::X));
        // Sliding carries the grabbed point.
        let to = V3::new(0.0, 2.0, 1.0);
        let slid = wall.slide(&frame, pa.pos, to).unwrap();
        assert!((wall.exp(&slid, a, 0.0).pos - to).length() < 1e-12);
    }

    #[test]
    fn a_plane_is_the_identity_map() {
        let plane = Plane {
            pose: Pose::IDENTITY,
        };
        let frame = plane.frame_at(V3::new(5.0, 6.0, 9.0), V3::Y).unwrap();
        assert_eq!(frame.origin, V3::new(5.0, 6.0, 0.0));
        assert!(
            (frame.x - V3::X).length() < 1e-15,
            "x is right when y is up"
        );
        let p = plane.exp(&frame, [2.0, 3.0], 1.0);
        assert_eq!(p.pos, V3::new(7.0, 9.0, 1.0));
        assert_eq!(plane.log(&frame, V3::new(7.0, 9.0, 0.0)), Some([2.0, 3.0]));
        let ray = Ray {
            origin: V3::new(0.0, 0.0, 4.0),
            dir: -V3::Z,
        };
        assert_eq!(plane.intersect(ray, 0.0), Some(4.0));
        assert!(plane.max_extent_m().is_infinite());
    }

    #[test]
    fn a_pose_carries_points_both_ways_and_marks_the_fingerprint() {
        let pose = Pose {
            origin: V3::new(10.0, 0.0, -3.0),
            rotation: M3::rodrigues(V3::Y, 0.6),
        };
        let p = V3::new(1.0, 2.0, 3.0);
        assert!((pose.to_body(pose.to_world(p)) - p).length() < 1e-12);
        assert!((pose.dir_to_body(pose.dir_to_world(p)) - p).length() < 1e-12);
        let still = planet();
        let turned = Sphere { pose, ..still };
        assert_ne!(still.fingerprint(), turned.fingerprint());
        assert_eq!(still.fingerprint(), planet().fingerprint());
    }
}
