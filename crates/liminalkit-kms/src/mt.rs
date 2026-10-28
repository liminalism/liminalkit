//! Linux multi-touch protocol B, straight from evdev, into liminalkit's
//! [`TouchEvent`]s.
//!
//! Protocol B (Documentation/input/multi-touch-protocol.rst) is what slot-
//! based controllers report — the Goodix GT9xx family among them: `ABS_MT_SLOT`
//! selects a slot, `ABS_MT_TRACKING_ID` starts (≥ 0) or ends (−1) the contact
//! in it, `ABS_MT_POSITION_X/Y` move it, and `SYN_REPORT` says "that was one
//! consistent frame". Nothing is emitted until the `SYN_REPORT`, so a contact
//! that starts and moves in the same report arrives as one `Started` at its
//! final position.
//!
//! `SYN_DROPPED` means the kernel's buffer overflowed and events were lost.
//! The state can no longer be trusted, so every contact is cancelled at the
//! next report (a cancellation activates nothing) and fingers still down are
//! ignored until they lift and touch again — the safe failure for a kiosk.
//!
//! Pure: fed `(type, code, value)` triples, so whole touch sessions are tests.

use std::time::Instant;

use liminalkit_shell::{TouchEvent, TouchId, TouchPhase, TouchTransform};

use crate::rotate::Rotation;

pub const EV_SYN: u16 = 0x00;
pub const EV_ABS: u16 = 0x03;
pub const SYN_REPORT: u16 = 0;
pub const SYN_DROPPED: u16 = 3;
pub const ABS_MT_SLOT: u16 = 0x2f;
pub const ABS_MT_POSITION_X: u16 = 0x35;
pub const ABS_MT_POSITION_Y: u16 = 0x36;
pub const ABS_MT_TRACKING_ID: u16 = 0x39;
pub const ABS_MT_PRESSURE: u16 = 0x3a;

/// A device axis's reported range, inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AxisRange {
    pub min: i32,
    pub max: i32,
}

impl AxisRange {
    /// `value` as a position across `pixels`, continuous: `min` is the left
    /// edge of the first pixel, `max` the inside of the last.
    fn to_pixels(self, value: i32, pixels: u32) -> f64 {
        let span = f64::from(self.max) - f64::from(self.min) + 1.0;
        (f64::from(value) - f64::from(self.min) + 0.5) * f64::from(pixels) / span.max(1.0)
    }
}

/// How a device's raw coordinates become logical buffer coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchMapping {
    pub x: AxisRange,
    pub y: AxisRange,
    pub pressure: Option<AxisRange>,
    /// The panel's scan-out size, which the touch surface covers.
    pub panel: (u32, u32),
    /// Applied in panel pixels, before rotation: for a controller mounted
    /// differently from the glass (swapped or inverted axes).
    pub calibration: TouchTransform,
    pub rotation: Rotation,
}

#[derive(Debug, Clone, Copy, Default)]
struct Slot {
    /// The contact in the slot, if one is down.
    tracking: Option<i32>,
    /// The contact the application has been told about, if any.
    reported: Option<i32>,
    x: i32,
    y: i32,
    pressure: Option<i32>,
    moved: bool,
}

/// Slots per device, at most. Ten-finger controllers use ten; the cap keeps
/// a hostile or broken device from growing this without bound.
const MAX_SLOTS: usize = 32;

pub struct MtDecoder {
    device: u64,
    mapping: TouchMapping,
    slots: Vec<Slot>,
    current: usize,
    dropped: bool,
}

impl MtDecoder {
    pub fn new(device: u64, mapping: TouchMapping) -> MtDecoder {
        MtDecoder {
            device,
            mapping,
            slots: vec![Slot::default(); 10],
            current: 0,
            dropped: false,
        }
    }

    /// Feed one evdev event. Touch events for a completed report are pushed
    /// to `out`.
    pub fn feed(
        &mut self,
        kind: u16,
        code: u16,
        value: i32,
        at: Instant,
        out: &mut Vec<TouchEvent>,
    ) {
        match (kind, code) {
            (EV_SYN, SYN_DROPPED) => self.dropped = true,
            (EV_SYN, SYN_REPORT) if self.dropped => {
                self.dropped = false;
                self.cancel_all(at, out);
            }
            (EV_SYN, SYN_REPORT) => self.report(at, out),
            _ if self.dropped => {}
            (EV_ABS, ABS_MT_SLOT) => {
                if let Ok(slot) = usize::try_from(value) {
                    if slot < MAX_SLOTS {
                        if slot >= self.slots.len() {
                            self.slots.resize(slot + 1, Slot::default());
                        }
                        self.current = slot;
                    }
                }
            }
            (EV_ABS, ABS_MT_TRACKING_ID) => {
                let slot = &mut self.slots[self.current];
                slot.tracking = (value >= 0).then_some(value);
                slot.moved = true;
            }
            (EV_ABS, ABS_MT_POSITION_X) => {
                let slot = &mut self.slots[self.current];
                slot.x = value;
                slot.moved = true;
            }
            (EV_ABS, ABS_MT_POSITION_Y) => {
                let slot = &mut self.slots[self.current];
                slot.y = value;
                slot.moved = true;
            }
            (EV_ABS, ABS_MT_PRESSURE) => self.slots[self.current].pressure = Some(value),
            _ => {}
        }
    }

    fn position(&self, slot: &Slot) -> Option<(f64, f64)> {
        let m = &self.mapping;
        let panel = (
            m.x.to_pixels(slot.x, m.panel.0),
            m.y.to_pixels(slot.y, m.panel.1),
        );
        let probe = TouchEvent {
            id: TouchId {
                device: 0,
                contact: 0,
            },
            phase: TouchPhase::Moved,
            position: panel,
            pressure: None,
            received_at: Instant::now(),
        };
        let calibrated = m.calibration.map(probe)?.position;
        Some(m.rotation.to_logical(m.panel, calibrated))
    }

    fn event(
        &self,
        slot: &Slot,
        contact: i32,
        phase: TouchPhase,
        position: (f64, f64),
        at: Instant,
    ) -> TouchEvent {
        TouchEvent {
            id: TouchId {
                device: self.device,
                contact: contact as u64,
            },
            phase,
            position,
            pressure: self.mapping.pressure.zip(slot.pressure).map(|(range, p)| {
                ((f64::from(p) - f64::from(range.min))
                    / (f64::from(range.max) - f64::from(range.min)).max(1.0))
                .clamp(0.0, 1.0)
            }),
            received_at: at,
        }
    }

    fn report(&mut self, at: Instant, out: &mut Vec<TouchEvent>) {
        for index in 0..self.slots.len() {
            let slot = self.slots[index];
            if !slot.moved {
                continue;
            }
            self.slots[index].moved = false;
            let position = self.position(&slot);
            // A contact that ended, or was replaced by a new one in the same
            // report (a lift and a press faster than one report).
            if let Some(old) = slot.reported.filter(|&old| slot.tracking != Some(old)) {
                let phase = if position.is_some() {
                    TouchPhase::Ended
                } else {
                    TouchPhase::Cancelled
                };
                let last = position.unwrap_or((0.0, 0.0));
                out.push(self.event(&slot, old, phase, last, at));
                self.slots[index].reported = None;
            }
            let Some(contact) = slot.tracking else {
                continue;
            };
            // Coordinates the calibration cannot map are not a position;
            // the contact waits for one that is.
            let Some(position) = position else {
                continue;
            };
            let phase = if self.slots[index].reported == Some(contact) {
                TouchPhase::Moved
            } else {
                self.slots[index].reported = Some(contact);
                TouchPhase::Started
            };
            out.push(self.event(&slot, contact, phase, position, at));
        }
    }

    /// Cancel everything the application was told about, and forget every
    /// slot: after a drop no slot's state is known.
    pub fn cancel_all(&mut self, at: Instant, out: &mut Vec<TouchEvent>) {
        for index in 0..self.slots.len() {
            let slot = self.slots[index];
            if let Some(contact) = slot.reported {
                let position = self.position(&slot).unwrap_or((0.0, 0.0));
                out.push(self.event(&slot, contact, TouchPhase::Cancelled, position, at));
            }
            self.slots[index] = Slot::default();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping(rotation: Rotation) -> TouchMapping {
        TouchMapping {
            x: AxisRange { min: 0, max: 1279 },
            y: AxisRange { min: 0, max: 799 },
            pressure: None,
            panel: (1280, 800),
            calibration: TouchTransform::IDENTITY,
            rotation,
        }
    }

    struct Feed {
        decoder: MtDecoder,
        at: Instant,
    }

    impl Feed {
        fn new(rotation: Rotation) -> Feed {
            Feed {
                decoder: MtDecoder::new(7, mapping(rotation)),
                at: Instant::now(),
            }
        }

        fn abs(&mut self, code: u16, value: i32) {
            let mut out = Vec::new();
            self.decoder.feed(EV_ABS, code, value, self.at, &mut out);
            assert!(out.is_empty(), "nothing before the report");
        }

        fn syn(&mut self, code: u16) -> Vec<(u64, TouchPhase, (f64, f64))> {
            let mut out = Vec::new();
            self.decoder.feed(EV_SYN, code, 0, self.at, &mut out);
            out.iter()
                .map(|e| {
                    assert_eq!(e.id.device, 7);
                    (e.id.contact, e.phase, e.position)
                })
                .collect()
        }

        fn down(&mut self, slot: i32, id: i32, x: i32, y: i32) {
            self.abs(ABS_MT_SLOT, slot);
            self.abs(ABS_MT_TRACKING_ID, id);
            self.abs(ABS_MT_POSITION_X, x);
            self.abs(ABS_MT_POSITION_Y, y);
        }
    }

    #[test]
    fn a_tap_is_started_then_ended_at_pixel_centres() {
        let mut f = Feed::new(Rotation::R0);
        f.down(0, 41, 100, 200);
        assert_eq!(
            f.syn(SYN_REPORT),
            [(41, TouchPhase::Started, (100.5, 200.5))]
        );
        f.abs(ABS_MT_TRACKING_ID, -1);
        assert_eq!(f.syn(SYN_REPORT), [(41, TouchPhase::Ended, (100.5, 200.5))]);
        assert!(f.syn(SYN_REPORT).is_empty(), "an empty report says nothing");
    }

    #[test]
    fn two_fingers_move_independently_and_lift_in_any_order() {
        let mut f = Feed::new(Rotation::R0);
        f.down(0, 1, 10, 10);
        f.down(1, 2, 500, 500);
        assert_eq!(f.syn(SYN_REPORT).len(), 2);
        f.abs(ABS_MT_SLOT, 0);
        f.abs(ABS_MT_POSITION_X, 20);
        assert_eq!(f.syn(SYN_REPORT), [(1, TouchPhase::Moved, (20.5, 10.5))]);
        f.abs(ABS_MT_SLOT, 1);
        f.abs(ABS_MT_TRACKING_ID, -1);
        assert_eq!(f.syn(SYN_REPORT), [(2, TouchPhase::Ended, (500.5, 500.5))]);
        f.abs(ABS_MT_SLOT, 0);
        f.abs(ABS_MT_TRACKING_ID, -1);
        assert_eq!(f.syn(SYN_REPORT)[0].1, TouchPhase::Ended);
    }

    #[test]
    fn a_new_contact_in_the_same_report_ends_the_old_one_first() {
        let mut f = Feed::new(Rotation::R0);
        f.down(0, 1, 10, 10);
        f.syn(SYN_REPORT);
        f.abs(ABS_MT_TRACKING_ID, 2);
        f.abs(ABS_MT_POSITION_X, 30);
        let events = f.syn(SYN_REPORT);
        assert_eq!(events[0].0, 1);
        assert_eq!(events[0].1, TouchPhase::Ended);
        assert_eq!((events[1].0, events[1].1), (2, TouchPhase::Started));
    }

    #[test]
    fn a_dropped_buffer_cancels_everything_and_ignores_until_a_new_touch() {
        let mut f = Feed::new(Rotation::R0);
        f.down(0, 1, 10, 10);
        f.syn(SYN_REPORT);
        f.syn(SYN_DROPPED);
        f.abs(ABS_MT_POSITION_X, 900);
        assert_eq!(
            f.syn(SYN_REPORT),
            [(1, TouchPhase::Cancelled, (10.5, 10.5))]
        );
        // The finger still down moves, without a tracking id: ignored.
        f.abs(ABS_MT_POSITION_X, 950);
        assert!(f.syn(SYN_REPORT).is_empty());
        f.abs(ABS_MT_TRACKING_ID, -1);
        assert!(f.syn(SYN_REPORT).is_empty());
        f.down(0, 3, 40, 40);
        assert_eq!(f.syn(SYN_REPORT)[0].1, TouchPhase::Started);
    }

    #[test]
    fn coordinates_follow_the_display_rotation() {
        let mut f = Feed::new(Rotation::R90);
        // The panel's top-right corner is the upright image's top-left.
        f.down(0, 1, 1279, 0);
        let (_, _, (x, y)) = f.syn(SYN_REPORT)[0];
        assert!(x < 1.0 && y < 1.0, "{x},{y}");
    }

    #[test]
    fn device_ranges_scale_to_the_panel_and_bad_slots_are_ignored() {
        let mut m = mapping(Rotation::R0);
        m.x = AxisRange { min: 0, max: 4095 };
        m.y = AxisRange { min: 0, max: 4095 };
        let mut decoder = MtDecoder::new(1, m);
        let mut out = Vec::new();
        let at = Instant::now();
        for (code, value) in [
            (ABS_MT_SLOT, 999),
            (ABS_MT_SLOT, -3),
            (ABS_MT_TRACKING_ID, 5),
            (ABS_MT_POSITION_X, 4095),
            (ABS_MT_POSITION_Y, 2048),
        ] {
            decoder.feed(EV_ABS, code, value, at, &mut out);
        }
        decoder.feed(EV_SYN, SYN_REPORT, 0, at, &mut out);
        let (x, y) = out[0].position;
        assert!(
            (1279.0..1280.0).contains(&x) && (400.0..401.0).contains(&y),
            "{x},{y}"
        );
    }
}
