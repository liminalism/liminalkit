//! The little vector algebra curved windows need. **EXPERIMENTAL.**
//!
//! Two types, in `f64`: a planet is millions of metres across and a window on
//! it is placed to a fraction of a pixel, which `f32` cannot hold. A host
//! converts at the boundary with [`V3::from_array`] and
//! [`M3::from_quat_xyzw`]; nothing here is meant to replace its own maths
//! library.

use std::ops::{Add, Mul, Neg, Sub};

/// A point or direction in three dimensions.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct V3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl V3 {
    pub const ZERO: V3 = V3::new(0.0, 0.0, 0.0);
    pub const X: V3 = V3::new(1.0, 0.0, 0.0);
    pub const Y: V3 = V3::new(0.0, 1.0, 0.0);
    pub const Z: V3 = V3::new(0.0, 0.0, 1.0);

    pub const fn new(x: f64, y: f64, z: f64) -> V3 {
        V3 { x, y, z }
    }

    pub const fn from_array(a: [f64; 3]) -> V3 {
        V3::new(a[0], a[1], a[2])
    }

    pub const fn to_array(self) -> [f64; 3] {
        [self.x, self.y, self.z]
    }

    pub fn dot(self, o: V3) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    pub fn cross(self, o: V3) -> V3 {
        V3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    pub fn length(self) -> f64 {
        self.dot(self).sqrt()
    }

    /// The unit vector along this one; `None` for a zero or non-finite one.
    pub fn try_normalize(self) -> Option<V3> {
        let length = self.length();
        (length.is_finite() && length > 1e-300).then(|| self * (1.0 / length))
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
}

impl Add for V3 {
    type Output = V3;
    fn add(self, o: V3) -> V3 {
        V3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}

impl Sub for V3 {
    type Output = V3;
    fn sub(self, o: V3) -> V3 {
        V3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

impl Mul<f64> for V3 {
    type Output = V3;
    fn mul(self, k: f64) -> V3 {
        V3::new(self.x * k, self.y * k, self.z * k)
    }
}

impl Neg for V3 {
    type Output = V3;
    fn neg(self) -> V3 {
        V3::new(-self.x, -self.y, -self.z)
    }
}

/// A 3×3 matrix by columns; here always a rotation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct M3 {
    pub cols: [V3; 3],
}

impl M3 {
    pub const IDENTITY: M3 = M3 {
        cols: [V3::X, V3::Y, V3::Z],
    };

    /// The rotation of a unit quaternion given as `[x, y, z, w]`, the order
    /// `glam`'s `to_array` uses. A zero quaternion is the identity.
    pub fn from_quat_xyzw(q: [f64; 4]) -> M3 {
        let norm = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
        if !(norm.is_finite() && norm > 1e-300) {
            return M3::IDENTITY;
        }
        let [x, y, z, w] = q.map(|c| c / norm);
        M3 {
            cols: [
                V3::new(
                    1.0 - 2.0 * (y * y + z * z),
                    2.0 * (x * y + z * w),
                    2.0 * (x * z - y * w),
                ),
                V3::new(
                    2.0 * (x * y - z * w),
                    1.0 - 2.0 * (x * x + z * z),
                    2.0 * (y * z + x * w),
                ),
                V3::new(
                    2.0 * (x * z + y * w),
                    2.0 * (y * z - x * w),
                    1.0 - 2.0 * (x * x + y * y),
                ),
            ],
        }
    }

    /// Rotation by `angle` radians about a unit `axis` (right-handed).
    pub fn rodrigues(axis: V3, angle: f64) -> M3 {
        let (s, c) = angle.sin_cos();
        let t = 1.0 - c;
        let V3 { x, y, z } = axis;
        M3 {
            cols: [
                V3::new(t * x * x + c, t * x * y + s * z, t * x * z - s * y),
                V3::new(t * x * y - s * z, t * y * y + c, t * y * z + s * x),
                V3::new(t * x * z + s * y, t * y * z - s * x, t * z * z + c),
            ],
        }
    }

    pub fn transpose(self) -> M3 {
        let [a, b, c] = self.cols;
        M3 {
            cols: [
                V3::new(a.x, b.x, c.x),
                V3::new(a.y, b.y, c.y),
                V3::new(a.z, b.z, c.z),
            ],
        }
    }

    pub fn mul_vec(self, v: V3) -> V3 {
        self.cols[0] * v.x + self.cols[1] * v.y + self.cols[2] * v.z
    }

    /// `self × o`: apply `o`, then `self`.
    pub fn compose(self, o: M3) -> M3 {
        M3 {
            cols: o.cols.map(|c| self.mul_vec(c)),
        }
    }
}

/// Mix a value into a running fingerprint. Not a cryptographic hash: it only
/// has to change when what it summarises does, to key a mesh cache.
pub(crate) fn mix(seed: u64, bits: u64) -> u64 {
    (seed ^ bits)
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        .rotate_left(29)
        ^ 0x2545_f491_4f6c_dd1d
}

pub(crate) fn mix_v3(seed: u64, v: V3) -> u64 {
    mix(mix(mix(seed, v.x.to_bits()), v.y.to_bits()), v.z.to_bits())
}

pub(crate) fn mix_m3(seed: u64, m: M3) -> u64 {
    m.cols.iter().fold(seed, |s, &c| mix_v3(s, c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: V3, b: V3) -> bool {
        (a - b).length() < 1e-12
    }

    #[test]
    fn a_quaternion_becomes_the_rotation_it_describes() {
        // A quarter turn about z: x goes to y.
        let half = std::f64::consts::FRAC_PI_4;
        let m = M3::from_quat_xyzw([0.0, 0.0, half.sin(), half.cos()]);
        assert!(close(m.mul_vec(V3::X), V3::Y));
        assert!(close(m.mul_vec(V3::Y), -V3::X));
        assert!(close(m.mul_vec(V3::Z), V3::Z));
        // Unnormalised input is normalised; a zero quaternion is harmless.
        let scaled = M3::from_quat_xyzw([0.0, 0.0, 3.0 * half.sin(), 3.0 * half.cos()]);
        assert!(close(scaled.mul_vec(V3::X), V3::Y));
        assert_eq!(M3::from_quat_xyzw([0.0; 4]), M3::IDENTITY);
    }

    #[test]
    fn rodrigues_agrees_with_the_quaternion_and_inverts_by_transpose() {
        let axis = V3::new(1.0, 2.0, -0.5).try_normalize().unwrap();
        let angle = 0.83_f64;
        let (s, c) = (angle / 2.0).sin_cos();
        let quat = M3::from_quat_xyzw([axis.x * s, axis.y * s, axis.z * s, c]);
        let rod = M3::rodrigues(axis, angle);
        let v = V3::new(0.3, -1.1, 2.0);
        assert!(close(quat.mul_vec(v), rod.mul_vec(v)));
        assert!(close(rod.transpose().mul_vec(rod.mul_vec(v)), v));
        assert!(close(rod.mul_vec(axis), axis), "the axis does not move");
        assert!(close(M3::rodrigues(axis, 0.0).mul_vec(v), v));
        assert!(close(rod.compose(rod.transpose()).mul_vec(v), v));
    }

    #[test]
    fn normalising_refuses_what_has_no_direction() {
        assert!(V3::ZERO.try_normalize().is_none());
        assert!(V3::new(f64::NAN, 0.0, 0.0).try_normalize().is_none());
        assert!(V3::new(f64::INFINITY, 0.0, 0.0).try_normalize().is_none());
        let unit = V3::new(3.0, 4.0, 0.0).try_normalize().unwrap();
        assert!((unit.length() - 1.0).abs() < 1e-15);
        assert!(close(V3::X.cross(V3::Y), V3::Z));
    }

    #[test]
    fn a_fingerprint_moves_when_its_input_does() {
        let a = mix_v3(1, V3::new(1.0, 2.0, 3.0));
        assert_ne!(a, mix_v3(1, V3::new(1.0, 2.0, 3.000_000_1)));
        assert_ne!(a, mix_v3(1, V3::new(2.0, 1.0, 3.0)));
        assert_eq!(a, mix_v3(1, V3::new(1.0, 2.0, 3.0)));
    }
}
