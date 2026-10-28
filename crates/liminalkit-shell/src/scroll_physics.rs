//! EXPERIMENTAL / UNTESTED: original scroll equations and policy. Defaults have
//! not been calibrated on hardware. Positions are caller-selected units; time is
//! seconds. Integrates exponential drag and damped springs analytically, including
//! the exact time a free fling crosses a boundary. No external physics library.
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Boundary {
    Clamp,
    Elastic,
}
#[derive(Debug, Clone, Copy)]
pub struct ScrollConfig {
    /// Exponential velocity decay, inverse seconds.
    pub friction: f64,
    /// Undamped angular frequency, radians/second.
    pub spring_frequency: f64,
    /// 1 is critical damping; below 1 oscillates, above 1 is overdamped.
    pub damping_ratio: f64,
    pub boundary: Boundary,
    /// Direct-drag overscroll distance (asymptotic). Catching an existing
    /// overscroll expands this distance to preserve continuity.
    pub elastic_limit: f64,
    pub min_fling_velocity: f64,
    pub max_fling_velocity: f64,
    /// Optional equally spaced snap points starting at the lower bound.
    pub snap_interval: Option<f64>,
}
impl Default for ScrollConfig {
    fn default() -> Self {
        Self {
            friction: 6.0,
            spring_frequency: 18.0,
            damping_ratio: 1.0,
            boundary: Boundary::Elastic,
            elastic_limit: 100.0,
            min_fling_velocity: 40.0,
            max_fling_velocity: 6000.0,
            snap_interval: None,
        }
    }
}
impl ScrollConfig {
    pub fn validate(self) -> Result<Self, &'static str> {
        if [
            self.friction,
            self.spring_frequency,
            self.damping_ratio,
            self.elastic_limit,
            self.max_fling_velocity,
        ]
        .iter()
        .any(|v| !v.is_finite() || *v <= 0.0)
            || !self.min_fling_velocity.is_finite()
            || self.min_fling_velocity < 0.0
            || self.min_fling_velocity > self.max_fling_velocity
            || self
                .snap_interval
                .is_some_and(|v| !v.is_finite() || v <= 0.0)
        {
            return Err("scroll parameters must be finite with positive friction/spring/limits and valid velocity range");
        }
        Ok(self)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollMotion {
    Idle,
    Dragging,
    Fling,
    Spring,
}

#[derive(Debug, Clone)]
pub struct ScrollAxis {
    config: ScrollConfig,
    min: f64,
    max: f64,
    position: f64,
    velocity: f64,
    raw_drag: f64,
    drag_limit: f64,
    target: f64,
    motion: ScrollMotion,
}
impl ScrollAxis {
    pub fn new(min: f64, max: f64, config: ScrollConfig) -> Result<Self, &'static str> {
        if !min.is_finite() || !max.is_finite() || min > max {
            return Err("invalid scroll bounds");
        }
        Ok(Self {
            config: config.validate()?,
            min,
            max,
            position: min,
            velocity: 0.0,
            raw_drag: min,
            drag_limit: config.elastic_limit,
            target: min,
            motion: ScrollMotion::Idle,
        })
    }
    pub fn position(&self) -> f64 {
        self.position
    }
    pub fn velocity(&self) -> f64 {
        self.velocity
    }
    pub fn motion(&self) -> ScrollMotion {
        self.motion
    }
    pub fn animating(&self) -> bool {
        matches!(self.motion, ScrollMotion::Fling | ScrollMotion::Spring)
    }
    pub fn config(&self) -> ScrollConfig {
        self.config
    }
    /// Reconfiguration cancels motion; the caller must also cancel recognizers.
    pub fn configure(&mut self, config: ScrollConfig) -> Result<(), &'static str> {
        self.config = config.validate()?;
        self.jump_to(self.position);
        Ok(())
    }
    pub fn set_bounds(&mut self, min: f64, max: f64) -> Result<(), &'static str> {
        if !min.is_finite() || !max.is_finite() || min > max {
            return Err("invalid scroll bounds");
        }
        self.min = min;
        self.max = max;
        self.jump_to(self.position);
        Ok(())
    }
    pub fn jump_to(&mut self, position: f64) {
        if !position.is_finite() {
            return;
        }
        self.position = position.clamp(self.min, self.max);
        self.raw_drag = self.position;
        self.velocity = 0.0;
        self.motion = ScrollMotion::Idle;
    }
    /// Catch a moving surface at its current position. Return true to suppress
    /// activation by this contact. Advance to the press timestamp before calling.
    pub fn catch(&mut self) -> bool {
        let moving = self.animating();
        self.velocity = 0.0;
        self.motion = ScrollMotion::Dragging;
        let edge = self.position.clamp(self.min, self.max);
        let outside = self.position - edge;
        // Invert the rubber-band mapping so a caught overscroll does not jump.
        self.drag_limit = self
            .config
            .elastic_limit
            .max(outside.abs() + self.config.elastic_limit);
        let fraction = (outside.abs() / self.drag_limit).min(1.0 - 1e-9);
        self.raw_drag = edge - outside.signum() * self.drag_limit * (-fraction).ln_1p();
        if outside == 0.0 {
            self.raw_drag = self.position;
        }
        moving
    }
    /// Move the content offset (usually the negative finger delta). In Clamp
    /// mode return the unconsumed delta for a captured parent scroll region.
    /// Elastic mode owns the drag and returns zero; don't chain it as well.
    pub fn drag_by(&mut self, delta: f64) -> f64 {
        if !delta.is_finite() || self.motion != ScrollMotion::Dragging {
            return 0.0;
        }
        if self.config.boundary == Boundary::Clamp {
            let old = self.position;
            self.position = (old + delta).clamp(self.min, self.max);
            self.raw_drag = self.position;
            return delta - (self.position - old);
        }
        self.raw_drag += delta;
        let edge = self.raw_drag.clamp(self.min, self.max);
        let outside = self.raw_drag - edge;
        self.position = edge
            + outside.signum() * self.drag_limit * (1.0 - (-outside.abs() / self.drag_limit).exp());
        0.0
    }
    /// Release with content velocity. A cancellation should use cancel instead.
    pub fn release(&mut self, velocity: f64) {
        if self.motion != ScrollMotion::Dragging {
            return;
        }
        self.velocity = if velocity.is_finite() {
            velocity.clamp(
                -self.config.max_fling_velocity,
                self.config.max_fling_velocity,
            )
        } else {
            0.0
        };
        if self.velocity.abs() < self.config.min_fling_velocity {
            self.velocity = 0.0;
        }
        let edge = self.position.clamp(self.min, self.max);
        if edge != self.position {
            self.spring_to(edge);
        } else if let Some(interval) = self.config.snap_interval {
            let projected = self.position + self.velocity / self.config.friction;
            let target = self.min + ((projected - self.min) / interval).round() * interval;
            self.spring_to(target.clamp(self.min, self.max));
        } else {
            self.motion = if self.velocity == 0.0 {
                ScrollMotion::Idle
            } else {
                ScrollMotion::Fling
            };
        }
    }
    /// No fling or activation on focus/surface loss. Return directly to bounds.
    pub fn cancel(&mut self) {
        self.jump_to(self.position);
    }
    fn spring_to(&mut self, target: f64) {
        self.target = target;
        self.motion = ScrollMotion::Spring;
    }
    pub fn advance(&mut self, elapsed: Duration) {
        let dt = elapsed.as_secs_f64();
        if dt == 0.0 {
            return;
        }
        if self.motion == ScrollMotion::Fling {
            let f = self.config.friction;
            let decay = (-f * dt).exp();
            let next = self.position + self.velocity / f * (1.0 - decay);
            let edge = next.clamp(self.min, self.max);
            if edge != next {
                let ratio = (1.0 - (edge - self.position) * f / self.velocity)
                    .clamp(f64::MIN_POSITIVE, 1.0);
                let crossing = -ratio.ln() / f;
                self.position = edge;
                self.velocity *= ratio;
                if self.config.boundary == Boundary::Clamp {
                    self.velocity = 0.0;
                    self.motion = ScrollMotion::Idle;
                } else {
                    self.spring_to(edge);
                    self.advance_spring((dt - crossing).max(0.0));
                }
            } else {
                self.position = next;
                self.velocity *= decay;
                if self.velocity.abs() < 0.1 {
                    // Consume the analytic tail, independent of the frame where we stop.
                    self.position = (self.position + self.velocity / f).clamp(self.min, self.max);
                    self.velocity = 0.0;
                    self.motion = ScrollMotion::Idle;
                }
            }
        } else if self.motion == ScrollMotion::Spring {
            self.advance_spring(dt);
        }
    }
    fn advance_spring(&mut self, dt: f64) {
        let w = self.config.spring_frequency;
        let z = self.config.damping_ratio;
        let x = self.position - self.target;
        let v = self.velocity;
        let (next, velocity) = if (z - 1.0).abs() < 1e-6 {
            let b = v + w * x;
            let decay = (-w * dt).exp();
            ((x + b * dt) * decay, (v - w * b * dt) * decay)
        } else if z < 1.0 {
            let a = z * w;
            let b = w * (1.0 - z * z).sqrt();
            let c = (v + a * x) / b;
            let (sin, cos) = (b * dt).sin_cos();
            let decay = (-a * dt).exp();
            (
                decay * (x * cos + c * sin),
                decay * ((-a * x + b * c) * cos + (-a * c - b * x) * sin),
            )
        } else {
            let root = (z * z - 1.0).sqrt();
            let r1 = -w / (z + root);
            let r2 = -w * (z + root);
            let c1 = (v - r2 * x) / (r1 - r2);
            let c2 = x - c1;
            (
                c1 * (r1 * dt).exp() + c2 * (r2 * dt).exp(),
                r1 * c1 * (r1 * dt).exp() + r2 * c2 * (r2 * dt).exp(),
            )
        };
        self.position = self.target + next;
        self.velocity = velocity;
        if self.config.boundary == Boundary::Clamp {
            let bounded = self.position.clamp(self.min, self.max);
            if bounded != self.position {
                self.position = bounded;
                self.velocity = 0.0;
            }
        }
        if next.abs() < 0.01 && self.velocity.abs() < 0.1 {
            self.position = self.target;
            self.velocity = 0.0;
            self.motion = ScrollMotion::Idle;
        }
    }
}
