//! Plain text — and, since image viewers need it, whole images — with the
//! system clipboard.
//!
//! macOS uses NSPasteboard. Linux uses real X11 selections or Wayland
//! data-control via arboard, retaining the owner for the handle's lifetime.
//! A compositor without data-control reports unavailable; this does not claim
//! to implement seat/serial-based wl_data_device access on such compositors.
//! There is no successful process-local production fallback.
//!
//! Images travel as PNG on Linux (arboard's `image-data` flavour) and as
//! uncompressed TIFF on macOS (hand-encoded below: the one raster pasteboard
//! flavour every macOS release reads).

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardError(pub String);
impl std::fmt::Display for ClipboardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ClipboardError {}

/// Test-fixture image recording: width, height, straight-alpha RGBA bytes.
#[cfg(test)]
type FakeImage = std::sync::Arc<std::sync::Mutex<Option<(u32, u32, Vec<u8>)>>>;

#[derive(Clone)]
pub struct Clipboard {
    #[cfg(target_os = "linux")]
    native: std::sync::Arc<std::sync::Mutex<Option<arboard::Clipboard>>>,
    #[cfg(test)]
    fake: Option<std::sync::Arc<std::sync::Mutex<Option<String>>>>,
    #[cfg(test)]
    fake_image: Option<FakeImage>,
}

impl std::fmt::Debug for Clipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Clipboard { system contents redacted }")
    }
}
impl Default for Clipboard {
    fn default() -> Self {
        Self {
            #[cfg(target_os = "linux")]
            native: Default::default(),
            #[cfg(test)]
            fake: None,
            #[cfg(test)]
            fake_image: None,
        }
    }
}

impl Clipboard {
    /// A lazy system handle. Linux ownership is retained until the last clone
    /// is dropped; clipboard operations belong off the graphical event loop.
    pub fn new() -> Clipboard {
        Clipboard::default()
    }

    /// Copy `text` onto the clipboard, replacing whatever was there.
    /// Returns whether the text reached the clipboard.
    pub fn set_text(&mut self, text: impl Into<String>) -> bool {
        self.try_set_text(text.into()).is_ok()
    }

    pub fn try_set_text(&mut self, text: String) -> Result<(), ClipboardError> {
        #[cfg(test)]
        if let Some(fake) = &self.fake {
            *fake
                .lock()
                .map_err(|_| ClipboardError("test clipboard failed".into()))? = Some(text);
            return Ok(());
        }
        #[cfg(target_os = "linux")]
        return self.with_native(|clipboard| clipboard.set_text(text));
        #[cfg(target_os = "macos")]
        return if macos::set(&text) {
            Ok(())
        } else {
            Err(ClipboardError("pasteboard unavailable".into()))
        };
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = text;
            Err(ClipboardError(
                "system clipboard unavailable on this platform".into(),
            ))
        }
    }

    /// Compatibility read. Use `try_text` to distinguish empty from unavailable.
    pub fn text(&self) -> Option<String> {
        self.try_text().ok().flatten()
    }

    pub fn try_text(&self) -> Result<Option<String>, ClipboardError> {
        #[cfg(test)]
        if let Some(fake) = &self.fake {
            return Ok(fake
                .lock()
                .map_err(|_| ClipboardError("test clipboard failed".into()))?
                .clone());
        }
        #[cfg(target_os = "linux")]
        {
            return self.with_native(|clipboard| match clipboard.get_text() {
                Ok(text) => Ok(Some(text)),
                Err(arboard::Error::ContentNotAvailable) => Ok(None),
                Err(error) => Err(error),
            });
        }
        #[cfg(target_os = "macos")]
        return Ok(macos::get());
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        Err(ClipboardError(
            "system clipboard unavailable on this platform".into(),
        ))
    }

    /// Copy an image onto the clipboard: straight-alpha RGBA bytes, row by
    /// row, top-down. Returns whether the image reached the clipboard.
    /// Mismatched lengths (`rgba.len() != width*height*4`) fail closed.
    pub fn set_image_rgba(&mut self, width: u32, height: u32, rgba: Vec<u8>) -> bool {
        self.try_set_image_rgba(width, height, rgba).is_ok()
    }

    pub fn try_set_image_rgba(
        &mut self,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> Result<(), ClipboardError> {
        check_rgba_len(width, height, &rgba)?;
        #[cfg(test)]
        if let Some(fake) = &self.fake_image {
            *fake
                .lock()
                .map_err(|_| ClipboardError("test clipboard failed".into()))? =
                Some((width, height, rgba));
            return Ok(());
        }
        #[cfg(target_os = "linux")]
        return self.with_native(|clipboard| {
            clipboard.set_image(arboard::ImageData {
                width: width as usize,
                height: height as usize,
                bytes: std::borrow::Cow::Owned(rgba),
            })
        });
        #[cfg(target_os = "macos")]
        return if macos::set_image(&tiff_rgba(width, height, &rgba)) {
            Ok(())
        } else {
            Err(ClipboardError("pasteboard unavailable".into()))
        };
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = (width, height, rgba);
            Err(ClipboardError(
                "system clipboard unavailable on this platform".into(),
            ))
        }
    }

    /// What the last test-fixture image write recorded (`Clipboard::fake()`
    /// only; `None` for a real handle or before any write).
    #[cfg(test)]
    pub fn fake_image_contents(&self) -> Option<(u32, u32, Vec<u8>)> {
        self.fake_image.as_ref()?.lock().ok()?.clone()
    }

    pub fn clear(&mut self) {
        let _ = self.try_clear();
    }

    pub fn try_clear(&mut self) -> Result<(), ClipboardError> {
        #[cfg(test)]
        if let Some(fake) = &self.fake {
            *fake
                .lock()
                .map_err(|_| ClipboardError("test clipboard failed".into()))? = None;
            return Ok(());
        }
        #[cfg(target_os = "linux")]
        return self.with_native(|clipboard| clipboard.clear());
        #[cfg(target_os = "macos")]
        {
            macos::clear();
            Ok(())
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        Err(ClipboardError(
            "system clipboard unavailable on this platform".into(),
        ))
    }

    #[cfg(target_os = "linux")]
    fn with_native<T>(
        &self,
        operation: impl FnOnce(&mut arboard::Clipboard) -> Result<T, arboard::Error>,
    ) -> Result<T, ClipboardError> {
        let mut native = self
            .native
            .lock()
            .map_err(|_| ClipboardError("clipboard worker failed".into()))?;
        if native.is_none() {
            *native =
                Some(arboard::Clipboard::new().map_err(|error| ClipboardError(error.to_string()))?);
        }
        operation(native.as_mut().expect("native clipboard initialized"))
            .map_err(|error| ClipboardError(error.to_string()))
    }

    /// Explicit local test fixture: never selected from the runtime environment.
    #[cfg(test)]
    fn fake() -> Self {
        Self {
            fake: Some(Default::default()),
            fake_image: Some(Default::default()),
            ..Self::default()
        }
    }
}

/// A bounded, single-worker clipboard owner. Completion wakes the caller's
/// existing event loop; there is no timer and no synchronous read on a frame.
#[derive(Clone, Debug)]
pub struct ClipboardWorker {
    sender: std::sync::mpsc::SyncSender<ClipboardCommand>,
}

#[derive(Debug)]
enum ClipboardOperation {
    Copy(String),
    Paste,
}
#[derive(Debug)]
struct ClipboardCommand {
    operation: ClipboardOperation,
    answer: std::sync::mpsc::Sender<Result<Option<zeroize::Zeroizing<String>>, ClipboardError>>,
}

#[derive(Debug)]
pub struct ClipboardTask {
    receiver: std::sync::mpsc::Receiver<Result<Option<zeroize::Zeroizing<String>>, ClipboardError>>,
}
impl ClipboardTask {
    pub fn try_result(&self) -> Option<Result<Option<String>, ClipboardError>> {
        match self.receiver.try_recv() {
            Ok(result) => {
                Some(result.map(|value| value.map(|mut text| std::mem::take(&mut *text))))
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Some(Err(ClipboardError("clipboard worker stopped".into())))
            }
        }
    }
}
impl ClipboardWorker {
    pub fn new(notify: impl Fn() + Send + Sync + 'static) -> Self {
        let (sender, receiver) = std::sync::mpsc::sync_channel::<ClipboardCommand>(16);
        std::thread::spawn(move || {
            let mut clipboard = Clipboard::new();
            while let Ok(command) = receiver.recv() {
                let result = match command.operation {
                    ClipboardOperation::Copy(text) => clipboard.try_set_text(text).map(|()| None),
                    ClipboardOperation::Paste => clipboard.try_text(),
                }
                .map(|value| value.map(zeroize::Zeroizing::new));
                // A canceled receiver and an unread queued completion both zeroize automatically.
                let _ = command.answer.send(result);
                notify();
            }
        });
        Self { sender }
    }

    pub fn copy(&self, text: String) -> Result<ClipboardTask, ClipboardError> {
        self.submit(ClipboardOperation::Copy(text))
    }
    pub fn paste(&self) -> Result<ClipboardTask, ClipboardError> {
        self.submit(ClipboardOperation::Paste)
    }
    fn submit(&self, operation: ClipboardOperation) -> Result<ClipboardTask, ClipboardError> {
        let (answer, receiver) = std::sync::mpsc::channel();
        self.sender
            .try_send(ClipboardCommand { operation, answer })
            .map_err(|error| ClipboardError(format!("clipboard request unavailable: {error}")))?;
        Ok(ClipboardTask { receiver })
    }
}

fn check_rgba_len(width: u32, height: u32, rgba: &[u8]) -> Result<(), ClipboardError> {
    let expected = u64::from(width) * u64::from(height) * 4;
    if expected > 0 && rgba.len() as u64 == expected {
        Ok(())
    } else {
        Err(ClipboardError(
            "image bytes do not match width*height*4".into(),
        ))
    }
}

/// Uncompressed little-endian RGBA TIFF, single strip, top-down rows: the
/// pasteboard raster macOS has read since NeXT. Only the macOS call site
/// ships it, but the encoder stays testable everywhere.
#[cfg(any(test, target_os = "macos"))]
fn tiff_rgba(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    const TYPE_SHORT: u16 = 3;
    const TYPE_LONG: u16 = 4;
    // (tag, type, count, value-or-offset)
    let bits_offset: u32 = 8 + 2 + 10 * 12 + 4;
    let pixels_offset = bits_offset + 8;
    let entries: [(u16, u16, u32, u32); 10] = [
        (256, TYPE_LONG, 1, width),
        (257, TYPE_LONG, 1, height),
        (258, TYPE_SHORT, 4, bits_offset),
        (259, TYPE_SHORT, 1, 1), // no compression
        (262, TYPE_SHORT, 1, 2), // RGB
        (273, TYPE_LONG, 1, pixels_offset),
        (277, TYPE_SHORT, 1, 4),     // RGBA
        (278, TYPE_LONG, 1, height), // one strip
        (279, TYPE_LONG, 1, rgba.len() as u32),
        (284, TYPE_SHORT, 1, 1), // chunky
    ];
    let mut out = Vec::with_capacity(pixels_offset as usize + rgba.len());
    out.extend_from_slice(b"II");
    out.extend_from_slice(&42u16.to_le_bytes());
    out.extend_from_slice(&8u32.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (tag, kind, count, value) in entries {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&0u32.to_le_bytes());
    debug_assert_eq!(out.len(), bits_offset as usize);
    out.extend_from_slice(&[8u8, 0, 8, 0, 8, 0, 8, 0]);
    debug_assert_eq!(out.len(), pixels_offset as usize);
    out.extend_from_slice(rgba);
    out
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2_app_kit::{NSPasteboard, NSPasteboardType};
    use objc2_foundation::{NSData, NSString};

    /// Run `f` against the general pasteboard. `generalPasteboard` returns
    /// nil — which objc2 reports as a panic, not an error — in processes
    /// without window-server access (a sandbox, an ssh session, parental
    /// controls), so the nil case is caught and reads as `default`. The
    /// clipboard keeps its contract: reads are `None`, never a crash.
    fn with_board<T: Default>(f: impl FnOnce(&NSPasteboard) -> T) -> T {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let board = NSPasteboard::generalPasteboard();
            f(&board)
        }))
        .unwrap_or_default()
    }

    /// The value of `NSPasteboardTypeString`, spelled out: that symbol is an
    /// extern static, and reading one takes `unsafe`, which this workspace
    /// forbids. The string has been `NSStringPboardType` since NeXT, and the
    /// round-trip test below proves the board answers to it.
    const STRING_TYPE: &str = "NSStringPboardType";
    /// Same treatment for `NSPasteboardTypeTIFF`.
    const TIFF_TYPE: &str = "NSTIFFPboardType";

    pub fn get() -> Option<String> {
        with_board(|board| {
            board
                .stringForType(&NSString::from_str(STRING_TYPE))
                .map(|value| value.to_string())
        })
    }

    pub fn set(text: &str) -> bool {
        with_board(|board| {
            board.clearContents();
            let value = NSString::from_str(text);
            board.setString_forType(&value, &NSString::from_str(STRING_TYPE))
        })
    }

    pub fn set_image(tiff: &[u8]) -> bool {
        with_board(|board| {
            board.clearContents();
            let data = NSData::with_bytes(tiff);
            board.setData_forType(Some(&data), &NSPasteboardType::from_str(TIFF_TYPE))
        })
    }

    pub fn clear() {
        with_board(|board| {
            board.clearContents();
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Tests share one system pasteboard, so they take turns.
    static LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn a_new_clipboard_reads_empty_or_foreign_text_without_failing() {
        let _guard = LOCK.lock().unwrap();
        let mut clipboard = Clipboard::fake();
        clipboard.clear();
        assert_eq!(clipboard.text(), None);
    }

    #[test]
    fn copied_text_reads_back() {
        let _guard = LOCK.lock().unwrap();
        let mut clipboard = Clipboard::fake();
        assert!(clipboard.set_text("ข้าวผัด 2"));
        assert_eq!(clipboard.text(), Some("ข้าวผัด 2".to_owned()));
    }

    #[test]
    fn copying_replaces_what_was_there() {
        let _guard = LOCK.lock().unwrap();
        let mut clipboard = Clipboard::fake();
        assert!(clipboard.set_text("first"));
        assert!(clipboard.set_text("second"));
        assert_eq!(clipboard.text(), Some("second".to_owned()));
    }

    #[test]
    fn clearing_reads_empty_again() {
        let _guard = LOCK.lock().unwrap();
        let mut clipboard = Clipboard::fake();
        assert!(clipboard.set_text("something"));
        clipboard.clear();
        assert_eq!(clipboard.text(), None);
    }

    #[test]
    fn copied_image_records_bytes_and_dimensions() {
        let _guard = LOCK.lock().unwrap();
        let mut clipboard = Clipboard::fake();
        assert_eq!(clipboard.fake_image_contents(), None);
        let rgba = vec![200, 40, 30, 255, 0, 0, 0, 0];
        assert!(clipboard.set_image_rgba(2, 1, rgba.clone()));
        assert_eq!(clipboard.fake_image_contents(), Some((2, 1, rgba)));
    }

    #[test]
    fn mismatched_image_bytes_fail_closed() {
        let _guard = LOCK.lock().unwrap();
        let mut clipboard = Clipboard::fake();
        assert!(!clipboard.set_image_rgba(2, 2, vec![0; 12]));
        assert!(!clipboard.set_image_rgba(0, 0, Vec::new()));
        assert_eq!(clipboard.fake_image_contents(), None);
    }

    #[test]
    fn tiff_encoder_writes_a_parseable_file() {
        let rgba = vec![200, 40, 30, 255, 0, 255, 0, 0];
        let tiff = tiff_rgba(2, 1, &rgba);
        assert_eq!(&tiff[0..4], b"II*\0", "little-endian magic");
        let ifd = u32::from_le_bytes(tiff[4..8].try_into().unwrap()) as usize;
        assert_eq!(ifd, 8);
        let count = u16::from_le_bytes(tiff[ifd..ifd + 2].try_into().unwrap()) as usize;
        assert_eq!(count, 10);
        let mut width = None;
        let mut height = None;
        let mut strip_offset = None;
        let mut strip_bytes = None;
        for i in 0..count {
            let entry = ifd + 2 + i * 12;
            let tag = u16::from_le_bytes(tiff[entry..entry + 2].try_into().unwrap());
            let value = u32::from_le_bytes(tiff[entry + 8..entry + 12].try_into().unwrap());
            match tag {
                256 => width = Some(value),
                257 => height = Some(value),
                259 => assert_eq!(value, 1, "uncompressed"),
                262 => assert_eq!(value, 2, "RGB photometric"),
                273 => strip_offset = Some(value as usize),
                279 => strip_bytes = Some(value as usize),
                _ => {}
            }
        }
        assert_eq!((width, height), (Some(2), Some(1)));
        let (offset, len) = (strip_offset.unwrap(), strip_bytes.unwrap());
        assert_eq!(len, rgba.len());
        assert_eq!(
            &tiff[offset..offset + len],
            rgba.as_slice(),
            "pixel strip round-trips"
        );
    }

    /// The macOS image path compiles against the real pasteboard bindings:
    /// `NSData::with_bytes` is the bytes constructor objc2-foundation
    /// exposes without the `block2` feature, and this test is what breaks
    /// a Mac `cargo test` first if it drifts again. No assertion on the
    /// outcome: without window-server access the board fails closed
    /// (`false`), which is the documented contract, not a failure.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_image_write_compiles_against_the_system_board() {
        let _guard = LOCK.lock().unwrap();
        let rgba = vec![200, 40, 30, 255, 0, 255, 0, 0];
        let mut clipboard = Clipboard::new();
        let _ = clipboard.set_image_rgba(2, 1, rgba);
    }
}
