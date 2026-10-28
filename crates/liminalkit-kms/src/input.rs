//! Touchscreens, found and read straight from evdev.
//!
//! A touchscreen here is a device that reports protocol-B multi-touch
//! (`ABS_MT_SLOT`, `ABS_MT_POSITION_X/Y`) and says it is *direct* — the
//! finger is on the image (`INPUT_PROP_DIRECT`), which is what tells a
//! touchscreen from a touchpad. The Goodix driver and USB `hid-multitouch`
//! panels both qualify. Each device's axis ranges come from the kernel, so a
//! controller reporting 0–4095 and one reporting the panel's own pixels both
//! land on the panel's pixels.
//!
//! No libinput: a kiosk has one touchscreen and no pointer acceleration,
//! palm rejection or gesture arbitration to want from it, and reading evdev
//! directly keeps the host free of C libraries.

use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::path::{Path, PathBuf};
use std::time::Instant;

use evdev::raw_stream::RawDevice;
use evdev::{AbsoluteAxisCode, PropType};
use liminalkit_shell::TouchEvent;

use crate::mt::{AxisRange, MtDecoder, TouchMapping};

pub struct TouchDevice {
    pub path: PathBuf,
    pub name: String,
    device: RawDevice,
    decoder: MtDecoder,
}

impl TouchDevice {
    fn open(
        path: &Path,
        device: RawDevice,
        id: u64,
        base: TouchMapping,
        require_direct: bool,
    ) -> Option<TouchDevice> {
        let axes = device.supported_absolute_axes()?;
        if !axes.contains(AbsoluteAxisCode::ABS_MT_SLOT)
            || !axes.contains(AbsoluteAxisCode::ABS_MT_POSITION_X)
            || !axes.contains(AbsoluteAxisCode::ABS_MT_POSITION_Y)
        {
            return None;
        }
        if require_direct && !device.properties().contains(PropType::DIRECT) {
            return None;
        }
        let (mut x, mut y, mut pressure) = (None, None, None);
        for (axis, info) in device.get_absinfo().ok()? {
            let range = AxisRange {
                min: info.minimum(),
                max: info.maximum(),
            };
            match axis {
                AbsoluteAxisCode::ABS_MT_POSITION_X => x = Some(range),
                AbsoluteAxisCode::ABS_MT_POSITION_Y => y = Some(range),
                AbsoluteAxisCode::ABS_MT_PRESSURE => pressure = Some(range),
                _ => {}
            }
        }
        let mapping = TouchMapping {
            x: x?,
            y: y?,
            pressure,
            ..base
        };
        Some(TouchDevice {
            path: path.to_owned(),
            name: device.name().unwrap_or("touchscreen").to_owned(),
            device,
            decoder: MtDecoder::new(id, mapping),
        })
    }

    /// Read what is waiting (call when `poll` says readable) and decode it.
    pub fn read(&mut self, out: &mut Vec<TouchEvent>) -> io::Result<()> {
        // Host receipt time, the same meaning the winit host gives it.
        let now = Instant::now();
        for event in self.device.fetch_events()? {
            self.decoder
                .feed(event.event_type().0, event.code(), event.value(), now, out);
        }
        Ok(())
    }

    /// The device went away: tell the application its contacts are gone.
    pub fn cancel(&mut self, out: &mut Vec<TouchEvent>) {
        self.decoder.cancel_all(Instant::now(), out);
    }
}

impl AsFd for TouchDevice {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.device.as_fd()
    }
}

/// Open `explicit`, or every direct multi-touch device under `/dev/input`.
/// `next_id` numbers devices so their contacts never share an identity.
pub fn discover(
    explicit: Option<&Path>,
    base: TouchMapping,
    next_id: &mut u64,
) -> Vec<TouchDevice> {
    let mut found = Vec::new();
    let mut take = |path: &Path, device: RawDevice, require_direct: bool| {
        if let Some(touch) = TouchDevice::open(path, device, *next_id, base, require_direct) {
            *next_id += 1;
            found.push(touch);
        }
    };
    match explicit {
        // Named on purpose: trust it even without the direct property, which
        // a few USB bridges leave off.
        Some(path) => {
            if let Ok(device) = RawDevice::open(path) {
                take(path, device, false);
            }
        }
        None => {
            for (path, device) in evdev::raw_stream::enumerate() {
                take(&path, device, true);
            }
        }
    }
    found
}
