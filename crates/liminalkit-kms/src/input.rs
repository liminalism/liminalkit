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

use std::collections::BTreeSet;
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
    known: &BTreeSet<PathBuf>,
) -> Vec<TouchDevice> {
    let candidates: Vec<_> = match explicit {
        // A deliberately named path need not advertise INPUT_PROP_DIRECT.
        Some(path) if !known.contains(path) => RawDevice::open(path)
            .ok()
            .map(|device| vec![(path.to_owned(), device)])
            .unwrap_or_default(),
        Some(_) => Vec::new(),
        None => evdev::raw_stream::enumerate().collect(),
    };
    open_new_devices(candidates, known, |path, device| {
        let touch = TouchDevice::open(path, device, *next_id, base, explicit.is_none())?;
        *next_id += 1;
        Some(touch)
    })
}

/// Reconcile candidate identities before assigning logical IDs or retaining
/// device handles. Existing readers are never replaced by a rescan.
fn open_new_devices<T, D>(
    candidates: impl IntoIterator<Item = (PathBuf, T)>,
    known: &BTreeSet<PathBuf>,
    mut open: impl FnMut(&Path, T) -> Option<D>,
) -> Vec<D> {
    let mut seen = known.clone();
    candidates
        .into_iter()
        .filter_map(|(path, candidate)| {
            if !seen.insert(path.clone()) {
                return None;
            }
            open(&path, candidate)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rescanning_opens_only_new_devices_and_reconnects_removed_ones() {
        let a = PathBuf::from("/dev/input/event1");
        let b = PathBuf::from("/dev/input/event2");
        let mut known = BTreeSet::from([a.clone()]);
        let mut next_id = 7;
        let mut scan = |known: &BTreeSet<PathBuf>| {
            open_new_devices(
                [(a.clone(), ()), (b.clone(), ()), (b.clone(), ())],
                known,
                |path, ()| {
                    let id = next_id;
                    next_id += 1;
                    Some((path.to_owned(), id))
                },
            )
        };
        let added = scan(&known);
        assert_eq!(added, [(b.clone(), 7)]);
        known.insert(b.clone());
        assert!(scan(&known).is_empty());
        known.remove(&a);
        assert_eq!(scan(&known), [(a, 8)]);
    }
}
