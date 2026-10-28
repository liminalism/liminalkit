//! The loop: one thread, one `poll`, three kinds of thing to wait for — the
//! display finishing a flip, a touchscreen with something to say, and a
//! wake from another thread.
//!
//! **The flip is the clock.** While the application animates, the next frame
//! is drawn the moment the previous one reaches the glass, so frames land one
//! per vertical blank with no timer beating against the panel's refresh.
//! When nothing moves, nothing is drawn and the loop sleeps in `poll` until a
//! finger, a wake, or the application's next deadline.
//!
//! **Input is never queued behind a frame.** Touch events are delivered to
//! the application as they are read; at most one frame is in flight, and
//! the next one is drawn from the newest state as soon as the flip completes.

use std::io::{self, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use liminalkit_raster::WindowBuffer;
use liminalkit_shell::{FrameTimer, Scale};
use liminalkit_windowing::{PixelApp, Waker};
use rustix::event::{poll, PollFd, PollFlags, Timespec};

use crate::display::Display;
use crate::input::{discover, TouchDevice};
use crate::mt::{AxisRange, TouchMapping};
use crate::pacing::Pacer;
use crate::{KmsConfig, KmsError};

/// How often to look for a touchscreen while there is none (a USB panel
/// plugged in late, or unplugged and back).
const RESCAN: Duration = Duration::from_secs(2);

pub fn run<A: PixelApp>(mut app: A, config: KmsConfig) -> Result<(), KmsError> {
    let mut display = Display::open(config.card.as_deref(), config.mode)?;
    let panel = display.size();
    let output = display.output().clone();
    let (width, height) = config.rotation.logical_size(panel);
    eprintln!(
        "liminalkit-kms: {} {} {}x{}@{}Hz, rotation {:?}, drawing {width}x{height}",
        output.card.display(),
        output.connector,
        output.width,
        output.height,
        output.refresh,
        config.rotation
    );

    let base = TouchMapping {
        // Replaced per device from its own reported ranges.
        x: AxisRange { min: 0, max: 0 },
        y: AxisRange { min: 0, max: 0 },
        pressure: None,
        panel,
        calibration: config.calibration,
        rotation: config.rotation,
    };
    let mut next_device = 0;
    let mut touches = discover(config.touch.as_deref(), base, &mut next_device);
    report_touch(&touches);
    let mut rescan_at = touches.is_empty().then(|| Instant::now() + RESCAN);

    let (wake_rx, wake_tx) = UnixStream::pair().map_err(io_error("wake socket"))?;
    wake_rx
        .set_nonblocking(true)
        .map_err(io_error("wake socket"))?;
    wake_tx
        .set_nonblocking(true)
        .map_err(io_error("wake socket"))?;
    app.attach(Waker::from_fn(move || {
        // A full socket already means "wake up"; dropping this byte is fine.
        let _ = (&wake_tx).write(&[1]);
    }));

    let scale = Scale(config.scale);
    app.on_scale(scale);
    let mut buffer = WindowBuffer::new(width, height);
    let mut pacer = Pacer::new(output.refresh);
    let mut timer = FrameTimer::from_env();
    let mut input_at: Option<Instant> = None;
    // Timings of the frame in flight, recorded when it reaches the glass.
    let mut in_flight: Option<(Duration, Duration, Duration, Option<Instant>)> = None;
    let mut touch_events = Vec::new();

    loop {
        let tick_started = Instant::now();
        app.tick();
        let tick = tick_started.elapsed();
        if app.should_exit() {
            break;
        }
        let now = Instant::now();
        pacer.schedule(now, app.animation_interval());

        if pacer.should_render() {
            let started = Instant::now();
            app.render(&mut buffer, scale);
            // The buffer is the panel's shape and the copy relies on it; an
            // application that resized it gets it back rather than taking
            // the host down.
            if (buffer.width, buffer.height) != (width, height) {
                buffer.resize(width, height);
            }
            let painted = Instant::now();
            display.present(&buffer, config.rotation)?;
            in_flight = Some((tick, painted - started, painted.elapsed(), input_at.take()));
            pacer.rendered();
        }

        let mut timeout = pacer.timeout(Instant::now());
        if let Some(at) = rescan_at {
            let left = at.saturating_duration_since(Instant::now());
            timeout = Some(timeout.map_or(left, |t| t.min(left)));
        }
        let (display_ready, wake_ready, ready_touches) = {
            let mut fds = Vec::with_capacity(2 + touches.len());
            fds.push(PollFd::from_borrowed_fd(display.fd(), PollFlags::IN));
            fds.push(PollFd::new(&wake_rx, PollFlags::IN));
            for touch in &touches {
                fds.push(PollFd::from_borrowed_fd(touch.as_fd(), PollFlags::IN));
            }
            let timespec = timeout.map(|t| Timespec {
                tv_sec: t.as_secs() as i64,
                tv_nsec: i64::from(t.subsec_nanos()),
            });
            match poll(&mut fds, timespec.as_ref()) {
                Ok(_) => {}
                Err(rustix::io::Errno::INTR) => continue,
                Err(e) => return Err(KmsError::Io(format!("poll: {e}"))),
            }
            let ready = |fd: &PollFd<'_>| !fd.revents().is_empty();
            let touch_ready: Vec<bool> = fds[2..].iter().map(ready).collect();
            (ready(&fds[0]), ready(&fds[1]), touch_ready)
        };

        if display_ready && display.dispatch()? {
            let on_glass = Instant::now();
            if let Some((tick, paint, copy, input)) = in_flight.take() {
                let latency = input.map(|at| on_glass.saturating_duration_since(at));
                timer.record_full(tick, paint, copy, latency);
            }
            pacer.flipped();
        }
        if wake_ready {
            let mut drain = [0u8; 64];
            while matches!((&wake_rx).read(&mut drain), Ok(n) if n > 0) {}
            pacer.invalidate();
        }
        let mut lost = Vec::new();
        for (index, touch) in touches.iter_mut().enumerate() {
            if !ready_touches.get(index).copied().unwrap_or(false) {
                continue;
            }
            touch_events.clear();
            if let Err(error) = touch.read(&mut touch_events) {
                if error.kind() != io::ErrorKind::WouldBlock {
                    eprintln!("liminalkit-kms: {} went away ({error})", touch.path.display());
                    touch.cancel(&mut touch_events);
                    lost.push(index);
                }
            }
            if !touch_events.is_empty() {
                input_at.get_or_insert_with(Instant::now);
                for event in &touch_events {
                    app.on_touch(event);
                }
                pacer.invalidate();
            }
        }
        for index in lost.into_iter().rev() {
            touches.remove(index);
            rescan_at.get_or_insert_with(|| Instant::now() + RESCAN);
        }
        if rescan_at.is_some_and(|at| Instant::now() >= at) {
            let found = discover(config.touch.as_deref(), base, &mut next_device);
            if found.is_empty() {
                rescan_at = Some(Instant::now() + RESCAN);
            } else {
                report_touch(&found);
                touches.extend(found);
                rescan_at = None;
            }
        }
    }
    timer.flush();
    app.on_exit();
    Ok(())
}

fn report_touch(touches: &[TouchDevice]) {
    if touches.is_empty() {
        eprintln!(
            "liminalkit-kms: no touchscreen yet; looking again every {}s",
            RESCAN.as_secs()
        );
    }
    for touch in touches {
        eprintln!(
            "liminalkit-kms: touch {} ({})",
            touch.path.display(),
            touch.name
        );
    }
}

fn io_error(what: &'static str) -> impl Fn(io::Error) -> KmsError {
    move |e| KmsError::Io(format!("{what}: {e}"))
}
