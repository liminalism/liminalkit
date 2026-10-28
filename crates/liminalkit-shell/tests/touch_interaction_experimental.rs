//! EXPERIMENTAL / UNTESTED: regression cases supplied for later execution.
//! These tests were compile-checked only when the feature was introduced.
use liminalkit_shell::gesture::{Gesture, GestureConfig, GestureRecognizer};
use liminalkit_shell::scroll_physics::{Boundary, ScrollAxis, ScrollConfig, ScrollMotion};
use liminalkit_shell::touch_replay::{ReplayCursor, TouchRecording};
use liminalkit_shell::{TouchEvent, TouchId, TouchPhase};
use std::time::{Duration, Instant};

fn touch(epoch: Instant, ms: u64, phase: TouchPhase, y: f64) -> TouchEvent {
    TouchEvent {
        id: TouchId {
            device: 1,
            contact: 7,
        },
        phase,
        position: (10.0, y),
        pressure: None,
        received_at: epoch + Duration::from_millis(ms),
    }
}
fn recognizer() -> GestureRecognizer {
    GestureRecognizer::new(GestureConfig::default()).unwrap()
}
fn axis(config: ScrollConfig) -> ScrollAxis {
    ScrollAxis::new(0.0, 1000.0, config).unwrap()
}
#[test]
fn release_activates_but_down_and_drag_do_not() {
    let epoch = Instant::now();
    let mut g = recognizer();
    assert_eq!(
        g.update(touch(epoch, 0, TouchPhase::Started, 10.0)),
        vec![Gesture::Pressed {
            position: (10.0, 10.0)
        }]
    );
    assert!(g
        .update(touch(epoch, 20, TouchPhase::Moved, 12.0))
        .is_empty());
    assert!(g
        .update(touch(epoch, 30, TouchPhase::Ended, 12.0))
        .iter()
        .any(|e| matches!(e, Gesture::Tap { .. })));
    g.update(touch(epoch, 40, TouchPhase::Started, 10.0));
    assert!(g
        .update(touch(epoch, 50, TouchPhase::Moved, 30.0))
        .contains(&Gesture::DragStarted));
    assert!(!g
        .update(touch(epoch, 60, TouchPhase::Ended, 40.0))
        .iter()
        .any(|e| matches!(e, Gesture::Tap { .. })));
}
#[test]
fn drag_pause_release_has_no_fling() {
    let epoch = Instant::now();
    let mut g = recognizer();
    g.update(touch(epoch, 0, TouchPhase::Started, 0.0));
    g.update(touch(epoch, 10, TouchPhase::Moved, 20.0));
    g.update(touch(epoch, 20, TouchPhase::Moved, 40.0));
    assert!(g
        .update(touch(epoch, 200, TouchPhase::Ended, 40.0))
        .contains(&Gesture::DragEnded {
            velocity: (0.0, 0.0)
        }));
}
#[test]
fn catching_fling_suppresses_tap_and_preserves_position() {
    let mut s = axis(ScrollConfig::default());
    s.jump_to(300.0);
    s.catch();
    s.release(900.0);
    s.advance(Duration::from_millis(30));
    let position = s.position();
    assert!(s.catch());
    assert_eq!(s.position(), position);
    assert_eq!(s.velocity(), 0.0);
    let epoch = Instant::now();
    let mut g = recognizer();
    g.update(touch(epoch, 0, TouchPhase::Started, 10.0));
    g.suppress_tap();
    assert!(!g
        .update(touch(epoch, 20, TouchPhase::Ended, 10.0))
        .iter()
        .any(|e| matches!(e, Gesture::Tap { .. })));
}
#[test]
fn multi_finger_transition_rebases_and_never_taps() {
    let epoch = Instant::now();
    let mut g = recognizer();
    g.update(touch(epoch, 0, TouchPhase::Started, 10.0));
    let mut second = touch(epoch, 10, TouchPhase::Started, 100.0);
    second.id.contact = 8;
    assert!(g.update(second).contains(&Gesture::Cancelled));
    second.phase = TouchPhase::Moved;
    second.position.1 = 120.0;
    second.received_at += Duration::from_millis(10);
    assert!(g
        .update(second)
        .iter()
        .any(|e| matches!(e, Gesture::Transform(_))));
    second.phase = TouchPhase::Ended;
    second.received_at += Duration::from_millis(10);
    g.update(second);
    assert!(g
        .update(touch(epoch, 40, TouchPhase::Moved, 12.0))
        .is_empty());
    assert_eq!(
        g.update(touch(epoch, 50, TouchPhase::Ended, 12.0)),
        vec![Gesture::Released]
    );
}
#[test]
fn cancelled_or_stale_terminal_events_never_activate_or_fling() {
    let epoch = Instant::now();
    let mut g = recognizer();
    g.update(touch(epoch, 20, TouchPhase::Started, 10.0));
    let out = g.update(touch(epoch, 10, TouchPhase::Ended, 10.0));
    assert_eq!(out, vec![Gesture::Cancelled, Gesture::Released]);
    assert!(!g.active());
    g.update(touch(epoch, 30, TouchPhase::Started, 10.0));
    assert_eq!(
        g.update(touch(epoch, 40, TouchPhase::Cancelled, 10.0)),
        vec![Gesture::Cancelled, Gesture::Released]
    );
}
#[test]
fn hard_boundaries_return_only_unconsumed_movement_for_parent() {
    let mut child = axis(ScrollConfig {
        boundary: Boundary::Clamp,
        ..ScrollConfig::default()
    });
    child.jump_to(990.0);
    child.catch();
    let remainder = child.drag_by(25.0);
    assert_eq!(child.position(), 1000.0);
    assert_eq!(remainder, 15.0);
    let mut parent = axis(ScrollConfig::default());
    parent.catch();
    parent.drag_by(remainder);
    assert_eq!(parent.position(), 15.0);
}
#[test]
fn elastic_drag_and_spring_settle_and_can_be_caught_without_jump() {
    let mut s = axis(ScrollConfig::default());
    s.catch();
    s.drag_by(-300.0);
    assert!(s.position() < 0.0 && s.position() > -100.0);
    s.release(0.0);
    s.advance(Duration::from_millis(30));
    let position = s.position();
    assert!(s.catch());
    s.drag_by(0.0);
    assert!((s.position() - position).abs() < 1e-8);
    s.release(0.0);
    s.advance(Duration::from_secs(5));
    assert_eq!(s.position(), 0.0);
    assert_eq!(s.motion(), ScrollMotion::Idle);
}
#[test]
fn partitioning_time_preserves_fling_and_boundary_spring() {
    for damping in [0.5, 1.0, 1.5] {
        let config = ScrollConfig {
            damping_ratio: damping,
            ..ScrollConfig::default()
        };
        let mut whole = axis(config);
        whole.jump_to(990.0);
        whole.catch();
        whole.release(2000.0);
        let mut pieces = whole.clone();
        whole.advance(Duration::from_millis(240));
        for _ in 0..24 {
            pieces.advance(Duration::from_millis(10));
        }
        assert!((whole.position() - pieces.position()).abs() < 1e-7);
        assert!((whole.velocity() - pieces.velocity()).abs() < 1e-7);
    }
}
#[test]
fn snapping_uses_projected_release_target() {
    let mut s = axis(ScrollConfig {
        snap_interval: Some(100.0),
        ..ScrollConfig::default()
    });
    s.jump_to(340.0);
    s.catch();
    s.release(600.0);
    s.advance(Duration::from_secs(5));
    assert_eq!(s.position(), 400.0);
    assert_eq!(s.motion(), ScrollMotion::Idle);
}
#[test]
fn invalid_configuration_is_rejected_without_changing_state() {
    assert!(GestureRecognizer::new(GestureConfig {
        slop: f64::NAN,
        ..GestureConfig::default()
    })
    .is_err());
    let mut s = axis(ScrollConfig::default());
    s.jump_to(500.0);
    assert!(s
        .configure(ScrollConfig {
            friction: 0.0,
            ..ScrollConfig::default()
        })
        .is_err());
    assert_eq!(s.position(), 500.0);
    assert!(s.set_bounds(5.0, 1.0).is_err());
}
#[test]
fn replay_preserves_order_timing_and_reports_truncation() {
    let epoch = Instant::now();
    let mut recording = TouchRecording::new(3);
    for (ms, phase, y) in [
        (0, TouchPhase::Started, 0.0),
        (10, TouchPhase::Moved, 20.0),
        (20, TouchPhase::Ended, 40.0),
    ] {
        assert!(recording.record(touch(epoch, ms, phase, y)));
    }
    let mut cursor = ReplayCursor::default();
    assert_eq!(cursor.due(&recording, Duration::from_millis(9)).len(), 1);
    assert!(cursor.due(&recording, Duration::from_millis(5)).is_empty());
    let due = cursor.due(&recording, Duration::from_millis(30));
    assert_eq!(due.len(), 2);
    assert_eq!(
        due[0].event(epoch).unwrap().received_at,
        epoch + Duration::from_millis(10)
    );
    assert!(cursor.finished(&recording));
    assert!(!recording.record(touch(epoch, 40, TouchPhase::Started, 0.0)));
    assert!(recording.truncated());
}
#[test]
fn replay_input_is_independent_of_display_poll_frequency() {
    let epoch = Instant::now();
    let mut recording = TouchRecording::new(10);
    for (ms, phase, y) in [
        (0, TouchPhase::Started, 0.0),
        (10, TouchPhase::Moved, 20.0),
        (20, TouchPhase::Moved, 40.0),
        (30, TouchPhase::Ended, 40.0),
    ] {
        recording.record(touch(epoch, ms, phase, y));
    }
    let run = |polls: &[u64]| {
        let mut g = recognizer();
        let mut cursor = ReplayCursor::default();
        let mut output = Vec::new();
        for ms in polls {
            for event in cursor.due(&recording, Duration::from_millis(*ms)) {
                output.extend(g.update(event.event(epoch).unwrap()));
            }
        }
        output
    };
    assert_eq!(run(&[0, 10, 20, 30]), run(&[30]));
}
