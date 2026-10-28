//! SVG path data (`<path d="…">`) into a [`Path`].
//!
//! The whole path grammar: `M L H V C S Q T A Z`, absolute and relative,
//! implicit repetition, `M` followed by extra pairs as line-tos, numbers with
//! exponents and without separators (`1.5.5`, `10-5`), and compact arc flags
//! (`a5 5 0 1010 0`). Arcs are converted to cubic Béziers per the SVG
//! specification's endpoint-to-centre conversion (SVG 1.1, appendix F.6),
//! a quarter turn at most per cubic.
//!
//! [`Path`] flattens curves in device pixels as they are appended, so the
//! mapping from the path's user units to pixels — an [`SvgTransform`] — is
//! applied *before* flattening, not after: a 24-unit icon drawn at 96 px gets
//! curves smooth at 96 px.
//!
//! This is the icon pipeline's input: any icon set published as SVG path
//! data (filled glyph style) can be drawn without an SVG renderer. Strokes,
//! transforms on groups, and everything else in SVG beyond the path grammar
//! are out of scope by design.

use std::fmt;

use crate::path::Path;

/// Where in `d` parsing failed, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SvgPathError {
    pub offset: usize,
    pub reason: &'static str,
}

impl fmt::Display for SvgPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SVG path data, byte {}: {}", self.offset, self.reason)
    }
}

impl std::error::Error for SvgPathError {}

/// User units to device pixels: a uniform scale, then an offset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SvgTransform {
    pub scale: f32,
    pub dx: f32,
    pub dy: f32,
}

impl SvgTransform {
    pub const IDENTITY: SvgTransform = SvgTransform {
        scale: 1.0,
        dx: 0.0,
        dy: 0.0,
    };

    /// Fit a square `view_box`-unit drawing into a `size`-pixel square whose
    /// top-left corner is `(x, y)`.
    pub fn fit(view_box: f32, size: f32, x: f32, y: f32) -> SvgTransform {
        SvgTransform {
            scale: size / view_box,
            dx: x,
            dy: y,
        }
    }

    fn apply(self, (x, y): (f64, f64)) -> (f32, f32) {
        (
            x as f32 * self.scale + self.dx,
            y as f32 * self.scale + self.dy,
        )
    }
}

impl Path {
    /// Append SVG path data, mapped through `transform`. On error nothing
    /// useful can be said about what was appended; discard the path.
    pub fn append_svg(
        &mut self,
        d: &str,
        transform: SvgTransform,
    ) -> Result<&mut Path, SvgPathError> {
        Builder {
            path: self,
            transform,
            current: (0.0, 0.0),
            start: (0.0, 0.0),
            reflect_cubic: None,
            reflect_quad: None,
            needs_move: false,
        }
        .run(d)?;
        Ok(self)
    }

    /// A path from SVG path data alone.
    pub fn from_svg(d: &str, transform: SvgTransform) -> Result<Path, SvgPathError> {
        let mut path = Path::new();
        path.append_svg(d, transform)?;
        Ok(path)
    }
}

struct Lexer<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Lexer<'_> {
    fn error(&self, reason: &'static str) -> SvgPathError {
        SvgPathError {
            offset: self.pos,
            reason,
        }
    }

    fn skip_separators(&mut self) {
        while self
            .bytes
            .get(self.pos)
            .is_some_and(|b| b.is_ascii_whitespace() || *b == b',')
        {
            self.pos += 1;
        }
    }

    fn done(&mut self) -> bool {
        self.skip_separators();
        self.pos >= self.bytes.len()
    }

    /// A command letter, if one is next.
    fn command(&mut self) -> Option<u8> {
        self.skip_separators();
        let byte = *self.bytes.get(self.pos)?;
        // `e`/`E` only ever appear inside a number, never as a command.
        if byte.is_ascii_alphabetic() && byte != b'e' && byte != b'E' {
            self.pos += 1;
            Some(byte)
        } else {
            None
        }
    }

    fn number(&mut self) -> Result<f64, SvgPathError> {
        self.skip_separators();
        let start = self.pos;
        let at = |pos: usize| self.bytes.get(pos).copied();
        let mut end = start;
        if matches!(at(end), Some(b'-' | b'+')) {
            end += 1;
        }
        let digits = |mut pos: usize| {
            while at(pos).is_some_and(|b| b.is_ascii_digit()) {
                pos += 1;
            }
            pos
        };
        let int_end = digits(end);
        let mut mantissa_digits = int_end - end;
        end = int_end;
        if at(end) == Some(b'.') {
            let frac_end = digits(end + 1);
            mantissa_digits += frac_end - (end + 1);
            end = frac_end;
        }
        if mantissa_digits == 0 {
            return Err(self.error("expected a number"));
        }
        if matches!(at(end), Some(b'e' | b'E')) {
            let mut exp = end + 1;
            if matches!(at(exp), Some(b'-' | b'+')) {
                exp += 1;
            }
            let exp_end = digits(exp);
            if exp_end > exp {
                end = exp_end;
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..end]).expect("ASCII digits");
        let value: f64 = text.parse().map_err(|_| self.error("malformed number"))?;
        if !value.is_finite() {
            return Err(self.error("number out of range"));
        }
        self.pos = end;
        Ok(value)
    }

    /// An arc flag: exactly one `0` or `1`, which may abut what follows.
    fn flag(&mut self) -> Result<bool, SvgPathError> {
        self.skip_separators();
        let flag = match self.bytes.get(self.pos) {
            Some(b'0') => false,
            Some(b'1') => true,
            _ => return Err(self.error("expected an arc flag (0 or 1)")),
        };
        self.pos += 1;
        Ok(flag)
    }

    fn pair(&mut self) -> Result<(f64, f64), SvgPathError> {
        Ok((self.number()?, self.number()?))
    }
}

struct Builder<'a> {
    path: &'a mut Path,
    transform: SvgTransform,
    /// In user units.
    current: (f64, f64),
    start: (f64, f64),
    /// The second control point of the last cubic, for `S`.
    reflect_cubic: Option<(f64, f64)>,
    /// The control point of the last quadratic, for `T`.
    reflect_quad: Option<(f64, f64)>,
    /// After `Z`, the next segment starts from the subpath's start.
    needs_move: bool,
}

impl Builder<'_> {
    fn run(mut self, d: &str) -> Result<(), SvgPathError> {
        let mut lexer = Lexer {
            bytes: d.as_bytes(),
            pos: 0,
        };
        let mut previous: Option<u8> = None;
        while !lexer.done() {
            let command = match lexer.command() {
                Some(c) => c,
                None => match previous {
                    // Extra coordinates repeat the command; after a move
                    // they are line-tos.
                    Some(b'M') => b'L',
                    Some(b'm') => b'l',
                    Some(c) if !matches!(c, b'Z' | b'z') => c,
                    _ => return Err(lexer.error("expected a command")),
                },
            };
            if previous.is_none() && !matches!(command, b'M' | b'm') {
                return Err(lexer.error("path data must begin with a move"));
            }
            let relative = command.is_ascii_lowercase();
            let base = if relative { self.current } else { (0.0, 0.0) };
            let offset = |(x, y): (f64, f64)| (x + base.0, y + base.1);
            let mut cubic_control = None;
            let mut quad_control = None;
            match command.to_ascii_uppercase() {
                b'M' => {
                    let to = offset(lexer.pair()?);
                    let (x, y) = self.transform.apply(to);
                    self.path.move_to(x, y);
                    self.current = to;
                    self.start = to;
                    self.needs_move = false;
                }
                b'L' => {
                    let to = offset(lexer.pair()?);
                    self.line(to);
                }
                b'H' => {
                    let x = lexer.number()? + base.0;
                    self.line((x, self.current.1));
                }
                b'V' => {
                    let y = lexer.number()? + base.1;
                    self.line((self.current.0, y));
                }
                b'C' => {
                    let c1 = offset(lexer.pair()?);
                    let c2 = offset(lexer.pair()?);
                    let to = offset(lexer.pair()?);
                    self.cubic(c1, c2, to);
                    cubic_control = Some(c2);
                }
                b'S' => {
                    let c1 = self.reflected(self.reflect_cubic);
                    let c2 = offset(lexer.pair()?);
                    let to = offset(lexer.pair()?);
                    self.cubic(c1, c2, to);
                    cubic_control = Some(c2);
                }
                b'Q' => {
                    let c = offset(lexer.pair()?);
                    let to = offset(lexer.pair()?);
                    self.quad(c, to);
                    quad_control = Some(c);
                }
                b'T' => {
                    let c = self.reflected(self.reflect_quad);
                    let to = offset(lexer.pair()?);
                    self.quad(c, to);
                    quad_control = Some(c);
                }
                b'A' => {
                    let rx = lexer.number()?;
                    let ry = lexer.number()?;
                    let rotation = lexer.number()?;
                    let large = lexer.flag()?;
                    let sweep = lexer.flag()?;
                    let to = offset(lexer.pair()?);
                    self.arc(rx, ry, rotation, large, sweep, to);
                }
                b'Z' => {
                    self.path.close();
                    self.current = self.start;
                    self.needs_move = true;
                }
                _ => return Err(lexer.error("unknown command")),
            }
            self.reflect_cubic = cubic_control;
            self.reflect_quad = quad_control;
            previous = Some(command);
        }
        Ok(())
    }

    fn reflected(&self, control: Option<(f64, f64)>) -> (f64, f64) {
        match control {
            Some((x, y)) => (2.0 * self.current.0 - x, 2.0 * self.current.1 - y),
            None => self.current,
        }
    }

    fn begin_segment(&mut self) {
        if self.needs_move {
            let (x, y) = self.transform.apply(self.start);
            self.path.move_to(x, y);
            self.needs_move = false;
        }
    }

    fn line(&mut self, to: (f64, f64)) {
        self.begin_segment();
        let (x, y) = self.transform.apply(to);
        self.path.line_to(x, y);
        self.current = to;
    }

    fn quad(&mut self, c: (f64, f64), to: (f64, f64)) {
        self.begin_segment();
        let (cx, cy) = self.transform.apply(c);
        let (x, y) = self.transform.apply(to);
        self.path.quad_to(cx, cy, x, y);
        self.current = to;
    }

    fn cubic(&mut self, c1: (f64, f64), c2: (f64, f64), to: (f64, f64)) {
        self.begin_segment();
        let (c1x, c1y) = self.transform.apply(c1);
        let (c2x, c2y) = self.transform.apply(c2);
        let (x, y) = self.transform.apply(to);
        self.path.cubic_to(c1x, c1y, c2x, c2y, x, y);
        self.current = to;
    }

    /// SVG 1.1 F.6.5: endpoint parameterization to centre parameterization,
    /// then at most a quarter turn per cubic.
    fn arc(&mut self, rx: f64, ry: f64, rotation: f64, large: bool, sweep: bool, to: (f64, f64)) {
        let from = self.current;
        if from == to {
            return;
        }
        let (mut rx, mut ry) = (rx.abs(), ry.abs());
        if rx == 0.0 || ry == 0.0 {
            self.line(to);
            return;
        }
        let (sin, cos) = rotation.to_radians().sin_cos();
        let dx2 = (from.0 - to.0) / 2.0;
        let dy2 = (from.1 - to.1) / 2.0;
        let x1p = cos * dx2 + sin * dy2;
        let y1p = -sin * dx2 + cos * dy2;
        // Radii too small to reach are scaled up just enough (F.6.6).
        let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
        if lambda > 1.0 {
            let root = lambda.sqrt();
            rx *= root;
            ry *= root;
        }
        let num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
        let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
        let sign = if large == sweep { -1.0 } else { 1.0 };
        let coef = sign * (num / den).max(0.0).sqrt();
        let cxp = coef * rx * y1p / ry;
        let cyp = -coef * ry * x1p / rx;
        let cx = cos * cxp - sin * cyp + (from.0 + to.0) / 2.0;
        let cy = sin * cxp + cos * cyp + (from.1 + to.1) / 2.0;

        let angle =
            |u: (f64, f64), v: (f64, f64)| (u.0 * v.1 - u.1 * v.0).atan2(u.0 * v.0 + u.1 * v.1);
        let u = ((x1p - cxp) / rx, (y1p - cyp) / ry);
        let v = ((-x1p - cxp) / rx, (-y1p - cyp) / ry);
        let theta = angle((1.0, 0.0), u);
        let mut delta = angle(u, v);
        let tau = std::f64::consts::TAU;
        if !sweep && delta > 0.0 {
            delta -= tau;
        } else if sweep && delta < 0.0 {
            delta += tau;
        }

        let segments = (delta.abs() / (tau / 4.0)).ceil().max(1.0) as usize;
        let step = delta / segments as f64;
        let k = 4.0 / 3.0 * (step / 4.0).tan();
        let point = |ux: f64, uy: f64| {
            (
                cx + rx * cos * ux - ry * sin * uy,
                cy + rx * sin * ux + ry * cos * uy,
            )
        };
        for i in 0..segments {
            let a1 = theta + step * i as f64;
            let a2 = a1 + step;
            let (s1, c1) = a1.sin_cos();
            let (s2, c2) = a2.sin_cos();
            let control1 = point(c1 - k * s1, s1 + k * c1);
            let control2 = point(c2 + k * s2, s2 - k * c2);
            let end = if i + 1 == segments { to } else { point(c2, s2) };
            self.cubic(control1, control2, end);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::{FillRule, RasterKernel};

    /// Total coverage in pixel units (255 = one pixel).
    fn area(path: &mut Path, size: usize) -> f64 {
        let mut kernel = RasterKernel::new();
        let mut total = 0u64;
        path.fill_with(
            &mut kernel,
            size,
            size,
            FillRule::NonZero,
            |_, x0, x1, cov| {
                total += cov[x0..=x1].iter().map(|&c| u64::from(c)).sum::<u64>();
            },
        );
        total as f64 / 255.0
    }

    fn svg(d: &str, scale: f32) -> Path {
        Path::from_svg(
            d,
            SvgTransform {
                scale,
                dx: 0.0,
                dy: 0.0,
            },
        )
        .unwrap()
    }

    #[test]
    fn lines_absolute_and_relative_draw_the_same_square() {
        let mut absolute = svg("M10 10 L50 10 L50 50 L10 50 Z", 1.0);
        let mut relative = svg("m10 10 h40 v40 h-40 z", 1.0);
        let mut implicit = svg("M10 10 50 10 50 50 10 50z", 1.0);
        for path in [&mut absolute, &mut relative, &mut implicit] {
            assert!((area(path, 64) - 1600.0).abs() < 0.5);
        }
    }

    #[test]
    fn two_arcs_make_a_circle_of_the_right_area() {
        // r = 5 user units at 8x: a 40 px radius.
        let mut circle = svg("M5 0A5 5 0 1 1 5 10A5 5 0 1 1 5 0Z", 8.0);
        let mut shifted = Path::from_svg(
            "M5 0A5 5 0 1 1 5 10A5 5 0 1 1 5 0Z",
            SvgTransform {
                scale: 8.0,
                dx: 4.0,
                dy: 4.0,
            },
        )
        .unwrap();
        let expected = std::f64::consts::PI * 40.0 * 40.0;
        for path in [&mut circle, &mut shifted] {
            let got = area(path, 96);
            assert!(
                (got - expected).abs() / expected < 0.002,
                "{got} vs {expected}"
            );
        }
        let b = shifted.bounds().unwrap();
        assert!(
            (b[0] - 4.0).abs() < 0.05 && (b[2] - 84.0).abs() < 0.05,
            "{b:?}"
        );
    }

    #[test]
    fn compact_numbers_and_flags_parse() {
        // `10-5.5` is two numbers, `.5.5` is two more, `1e1` has an
        // exponent, and `1010` is two flags and a coordinate.
        let mut path = svg("M0,0L10-5.5.5.5l1e1 0a5 5 0 1010 0z", 1.0);
        let b = path.bounds().unwrap();
        assert!(b[2] > 10.0);
        assert!(!path.geometry().1.is_empty());
    }

    #[test]
    fn smooth_curves_reflect_the_previous_control_point() {
        let mut smooth = svg("M0 20 C0 0 20 0 20 20 S40 40 40 20 Z", 2.0);
        let mut explicit = svg("M0 20 C0 0 20 0 20 20 C20 40 40 40 40 20 Z", 2.0);
        assert!((area(&mut smooth, 96) - area(&mut explicit, 96)).abs() < 0.01);
        let mut t = svg("M0 20 Q10 0 20 20 T40 20 Z", 2.0);
        let mut q = svg("M0 20 Q10 0 20 20 Q30 40 40 20 Z", 2.0);
        assert!((area(&mut t, 96) - area(&mut q, 96)).abs() < 0.01);
    }

    #[test]
    fn a_segment_after_close_starts_from_the_subpath_start() {
        // Two triangles sharing the start point; the second begins after Z
        // without a move.
        let mut path = svg("M0 0 L10 0 L0 10 Z L-10 0 L0 -10 Z", 1.0);
        assert_eq!(path.geometry().1.len(), 2);
    }

    #[test]
    fn malformed_data_is_refused_with_where_and_why() {
        let t = SvgTransform::IDENTITY;
        assert_eq!(
            Path::from_svg("L1 2", t).unwrap_err().reason,
            "path data must begin with a move"
        );
        assert_eq!(
            Path::from_svg("M1", t).unwrap_err().reason,
            "expected a number"
        );
        assert_eq!(
            Path::from_svg("M0 0 X", t).unwrap_err().reason,
            "unknown command"
        );
        assert_eq!(
            Path::from_svg("M0 0 A5 5 0 2 0 1 1", t).unwrap_err().reason,
            "expected an arc flag (0 or 1)"
        );
        let error = Path::from_svg("M0 0 L5 .", t).unwrap_err();
        assert_eq!(error.offset, 8);
        assert!(Path::from_svg("", t).unwrap().is_empty());
    }

    #[test]
    fn degenerate_arcs_are_lines_or_nothing() {
        let mut flat = svg("M0 0 A0 5 0 0 1 10 0 L10 10 Z", 1.0);
        assert!((area(&mut flat, 16) - 50.0).abs() < 0.5);
        // Radii too small to reach are scaled up: a half circle of radius 5.
        let mut scaled = svg("M0 0 A1 1 0 0 1 10 0 Z", 4.0);
        let half = std::f64::consts::PI * 20.0 * 20.0 / 2.0;
        // y goes negative (above the origin); shift it into view.
        let mut shifted = Path::from_svg(
            "M0 0 A1 1 0 0 1 10 0 Z",
            SvgTransform {
                scale: 4.0,
                dx: 0.0,
                dy: 30.0,
            },
        )
        .unwrap();
        assert!(area(&mut scaled, 64) < half);
        assert!((area(&mut shifted, 64) - half).abs() / half < 0.01);
    }
}
