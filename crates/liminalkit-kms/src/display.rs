//! The panel, driven directly through DRM/KMS: two dumb buffers, one on the
//! glass and one being drawn, swapped by a page flip at vertical blank.
//!
//! Dumb buffers are the kernel's CPU-mappable scan-out buffers: what the
//! software renderer produces is copied (and, for a quarter-turn mount,
//! turned) straight into the buffer the display controller reads. There is
//! no compositor's texture upload or composition pass in between.
//!
//! Both buffers are mapped once for the life of the process. Remapping per
//! frame, as a general-purpose library has to, would page-fault every page of
//! a 4–9 MB buffer back in on every frame.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::path::{Path, PathBuf};

use drm::buffer::{Buffer, DrmFourcc};
use drm::control::dumbbuffer::{DumbBuffer, DumbMapping};
use drm::control::{
    connector, crtc, framebuffer, Device as ControlDevice, Event, Mode, ModeTypeFlags,
    PageFlipFlags,
};
use drm::Device;
use liminalkit_raster::WindowBuffer;

use crate::mode::{choose_mode, ModeSummary};
use crate::rotate::Rotation;
use crate::KmsError;

struct Card(File);

impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl Device for Card {}
impl ControlDevice for Card {}

struct Scanout {
    fb: framebuffer::Handle,
    map: DumbMapping<'static>,
    pitch: usize,
}

/// What was found and is being driven.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub card: PathBuf,
    pub connector: String,
    pub width: u32,
    pub height: u32,
    pub refresh: u32,
}

pub struct Display {
    card: Card,
    crtc: crtc::Handle,
    connector: connector::Handle,
    /// What the CRTC showed before, restored on exit (the console, usually).
    saved: Option<crtc::Info>,
    buffers: Vec<Scanout>,
    front: usize,
    flip_pending: bool,
    output: Output,
}

impl Display {
    /// Open `card`, or the first `/dev/dri/card*` that has a connected display.
    /// Puts a black frame on the glass before returning, so a missing
    /// permission is an error now rather than at the first frame.
    pub fn open(card: Option<&Path>, want: Option<(u32, u32)>) -> Result<Display, KmsError> {
        let candidates: Vec<PathBuf> = match card {
            Some(path) => vec![path.to_owned()],
            None => {
                let mut cards: Vec<PathBuf> = std::fs::read_dir("/dev/dri")
                    .map_err(|e| KmsError::Display(format!("/dev/dri: {e}")))?
                    .filter_map(|entry| entry.ok().map(|e| e.path()))
                    .filter(|path| {
                        path.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.starts_with("card"))
                    })
                    .collect();
                cards.sort();
                cards
            }
        };
        let mut reasons = Vec::new();
        for path in candidates {
            match Display::try_open(&path, want) {
                Ok(display) => return Ok(display),
                Err(reason) => reasons.push(format!("{}: {reason}", path.display())),
            }
        }
        Err(KmsError::Display(if reasons.is_empty() {
            "no DRM card found under /dev/dri".into()
        } else {
            reasons.join("; ")
        }))
    }

    fn try_open(path: &Path, want: Option<(u32, u32)>) -> Result<Display, String> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| e.to_string())?;
        let card = Card(file);
        // Render-only nodes (a Pi's v3d) have no display resources.
        let resources = card
            .resource_handles()
            .map_err(|e| format!("not a display controller ({e})"))?;
        let connector = resources
            .connectors()
            .iter()
            .filter_map(|&handle| card.get_connector(handle, true).ok())
            .find(|info| info.state() == connector::State::Connected && !info.modes().is_empty())
            .ok_or("no connected display")?;
        let summaries: Vec<ModeSummary> = connector
            .modes()
            .iter()
            .map(|m| ModeSummary {
                width: u32::from(m.size().0),
                height: u32::from(m.size().1),
                refresh: m.vrefresh(),
                preferred: m.mode_type().contains(ModeTypeFlags::PREFERRED),
            })
            .collect();
        let index = choose_mode(&summaries, want).ok_or_else(|| match want {
            Some((w, h)) => format!("the display has no {w}x{h} mode"),
            None => "the display reports no modes".into(),
        })?;
        let mode: Mode = connector.modes()[index];
        let crtc = connector
            .current_encoder()
            .and_then(|e| card.get_encoder(e).ok())
            .and_then(|e| e.crtc())
            .or_else(|| {
                connector
                    .encoders()
                    .iter()
                    .filter_map(|&e| card.get_encoder(e).ok())
                    .flat_map(|e| resources.filter_crtcs(e.possible_crtcs()))
                    .next()
            })
            .ok_or("no CRTC can drive the display")?;
        let saved = card.get_crtc(crtc).ok();
        let (width, height) = (summaries[index].width, summaries[index].height);

        let mut buffers = Vec::with_capacity(2);
        for _ in 0..2 {
            let dumb = card
                .create_dumb_buffer((width, height), DrmFourcc::Xrgb8888, 32)
                .map_err(|e| format!("cannot allocate a scan-out buffer: {e}"))?;
            let fb = card
                .add_framebuffer(&dumb, 24, 32)
                .map_err(|e| format!("cannot register a framebuffer: {e}"))?;
            let pitch = dumb.pitch() as usize;
            // Kept for the life of the process so the mapping can be too;
            // the kernel frees both when the card is closed at exit.
            let dumb: &'static mut DumbBuffer = Box::leak(Box::new(dumb));
            let mut map = card
                .map_dumb_buffer(dumb)
                .map_err(|e| format!("cannot map a scan-out buffer: {e}"))?;
            map.fill(0);
            buffers.push(Scanout { fb, map, pitch });
        }
        card.set_crtc(
            crtc,
            Some(buffers[0].fb),
            (0, 0),
            &[connector.handle()],
            Some(mode),
        )
        .map_err(|e| {
            if matches!(e.kind(), io::ErrorKind::PermissionDenied) || e.raw_os_error() == Some(13) {
                "cannot take the display: another compositor or X server owns it, \
                 or this user cannot be DRM master (run from a VT with no desktop, \
                 as a member of the video group or with a logind seat)"
                    .to_string()
            } else {
                format!("cannot set the mode: {e}")
            }
        })?;
        Ok(Display {
            card,
            crtc,
            connector: connector.handle(),
            saved,
            buffers,
            front: 0,
            flip_pending: false,
            output: Output {
                card: path.to_owned(),
                connector: format!("{:?}-{}", connector.interface(), connector.interface_id()),
                width,
                height,
                refresh: mode.vrefresh(),
            },
        })
    }

    pub fn output(&self) -> &Output {
        &self.output
    }

    pub fn size(&self) -> (u32, u32) {
        (self.output.width, self.output.height)
    }

    pub fn fd(&self) -> BorrowedFd<'_> {
        self.card.as_fd()
    }

    /// Copy `buffer` into the back buffer and queue it for the next vertical
    /// blank. Only one flip may be in flight; wait for [`Display::dispatch`]
    /// to report it before presenting again.
    pub fn present(&mut self, buffer: &WindowBuffer, rotation: Rotation) -> Result<(), KmsError> {
        if self.flip_pending {
            return Err(KmsError::Display("a flip is already pending".into()));
        }
        let size = self.size();
        let back = 1 - self.front;
        let scanout = &mut self.buffers[back];
        rotation.copy(buffer, size, &mut scanout.map[..], scanout.pitch);
        self.card
            .page_flip(self.crtc, scanout.fb, PageFlipFlags::EVENT, None)
            .map_err(|e| KmsError::Display(format!("page flip failed: {e}")))?;
        self.flip_pending = true;
        Ok(())
    }

    /// Read the card's events after `poll` says it is readable. True when the
    /// pending flip completed: the frame is now on the glass.
    pub fn dispatch(&mut self) -> Result<bool, KmsError> {
        let mut flipped = false;
        let events = self
            .card
            .receive_events()
            .map_err(|e| KmsError::Display(format!("reading display events: {e}")))?;
        for event in events {
            if let Event::PageFlip(_) = event {
                flipped = true;
            }
        }
        if flipped && self.flip_pending {
            self.front = 1 - self.front;
            self.flip_pending = false;
        }
        Ok(flipped)
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        // Give the console (or whatever was there) its screen back. Best
        // effort: on the way out there is nobody to report a failure to.
        if let Some(saved) = &self.saved {
            let _ = self.card.set_crtc(
                self.crtc,
                saved.framebuffer(),
                saved.position(),
                &[self.connector],
                saved.mode(),
            );
        }
    }
}

/// Every DRM card the process can open, with what each has connected —
/// read-only, no mode is set. For `--list` style diagnostics, and safe to run
/// while a desktop owns the display.
pub fn probe() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir("/dev/dri") else {
        return vec!["/dev/dri: not present".into()];
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    let mut lines = Vec::new();
    for path in paths {
        let name = path.display().to_string();
        if !name.contains("card") {
            continue;
        }
        let card = match OpenOptions::new().read(true).write(true).open(&path) {
            Ok(file) => Card(file),
            Err(e) => {
                lines.push(format!("{name}: {e}"));
                continue;
            }
        };
        let Ok(resources) = card.resource_handles() else {
            lines.push(format!("{name}: render-only (no display)"));
            continue;
        };
        for &handle in resources.connectors() {
            let Ok(info) = card.get_connector(handle, false) else {
                continue;
            };
            let modes: Vec<String> = info
                .modes()
                .iter()
                .take(4)
                .map(|m| {
                    let star = if m.mode_type().contains(ModeTypeFlags::PREFERRED) {
                        "*"
                    } else {
                        ""
                    };
                    format!("{}x{}@{}{star}", m.size().0, m.size().1, m.vrefresh())
                })
                .collect();
            lines.push(format!(
                "{name}: {:?}-{} {:?} {}",
                info.interface(),
                info.interface_id(),
                info.state(),
                modes.join(" ")
            ));
        }
    }
    lines
}
