//! The Windows backend: Win32 child controls inside the winit window.
//!
//! winit owns the top-level window and its window procedure, and softbuffer
//! paints the whole client area. Three things make real controls live there:
//!
//! - `WS_CLIPCHILDREN` on the window, so the frame is never blitted over a
//!   control.
//! - A window subclass on the winit window, because a control reports to its
//!   parent (`WM_COMMAND`) and winit ignores those reports.
//! - Giving keyboard focus back to the window when an interaction ends,
//!   because a clicked Win32 control keeps focus and the application's keys
//!   would go to a button.
//!
//! `winsafe::gui` cannot be used for this — its controls accept only its own
//! windows as parents — so the controls are created with the raw
//! `CreateWindowEx` and driven by messages. That and the subclass are the
//! unsafe in this crate; all of it is in this file.
//!
//! # Invariants the unsafe code relies on
//!
//! - Everything here runs on the thread that owns the window. The subclass
//!   procedures are only ever called by that thread's message dispatch.
//! - The subclass reference data is a leaked `Rc<Shared>`. It is released
//!   exactly once, by whichever comes first of [`detach`] and the window's
//!   `WM_NCDESTROY`, and both take the [`Backend`] out of its cell to decide.
//! - Child controls are destroyed before that release (explicitly in
//!   [`detach`]; by Windows, before the parent's `WM_NCDESTROY`), so the edit
//!   subclass never outlives the data it points at.
//! - No `RefCell` borrow of the model is held across a call that can send a
//!   message, and the subclass procedures never borrow the backend, which
//!   [`apply`] holds while it changes controls.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use liminalkit_raster::Rect;
use liminalkit_shell::scroll::{ScrollSettings, SourceHint, WheelLines, WheelSample, WheelUnit};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;
use winsafe::guard::DeleteObjectGuard;
use winsafe::{self as w, co, HWND, POINT, SIZE};

use crate::model::{Command, ControlId, ControlKind, NativeEvent, Snapshot};
use crate::Shared;

/// Control ids start here; Windows reserves the low ids for dialogs.
const BASE_ID: u16 = 0x4000;
/// More controls than this in one window is a mistake, and ids must stay
/// below 57,344.
const MAX_CONTROLS: u32 = 8192;
const PARENT_SUBCLASS: usize = 0x4C4B_0001;

const WM_SETFONT: u32 = 0x0030;
const BM_GETCHECK: u32 = 0x00F0;
const BM_SETCHECK: u32 = 0x00F1;
const CB_ADDSTRING: u32 = 0x0143;
const CB_GETCURSEL: u32 = 0x0147;
const CB_SETCURSEL: u32 = 0x014E;
const CB_GETDROPPEDSTATE: u32 = 0x0157;

const BN_CLICKED: u16 = 0;
const CBN_SELCHANGE: u16 = 1;
const CBN_SELENDCANCEL: u16 = 10;
const EN_CHANGE: u16 = 0x0300;

const VK_TAB: usize = 0x09;
const VK_RETURN: usize = 0x0D;
const VK_ESCAPE: usize = 0x1B;

const WS_CHILD: u32 = 0x4000_0000;
const WS_DISABLED: u32 = 0x0800_0000;
const WS_CLIPSIBLINGS: u32 = 0x0400_0000;
const WS_VSCROLL: u32 = 0x0020_0000;
const WS_TABSTOP: u32 = 0x0001_0000;
const WS_EX_CLIENTEDGE: u32 = 0x0000_0200;
const BS_AUTOCHECKBOX: u32 = 0x0003;
const BS_PUSHLIKE: u32 = 0x1000;
const CBS_DROPDOWNLIST: u32 = 0x0003;
const CBS_HASSTRINGS: u32 = 0x0200;
const ES_AUTOHSCROLL: u32 = 0x0080;
const SS_LEFTNOWORDWRAP: u32 = 0x000C;
const SS_NOPREFIX: u32 = 0x0080;

#[derive(Default)]
pub(crate) struct State {
    backend: RefCell<Option<Backend>>,
    backdrop: RefCell<Option<Backdrop>>,
    /// The system's wheel settings, freshly read and not yet handed to the
    /// application.
    pub(crate) wheel_settings: Cell<Option<ScrollSettings>>,
    /// Wheel messages seen on their way to winit: the raw delta, and whether
    /// the system said a touchpad sent it. winit's event for the same
    /// message arrives while it is still being dispatched, so the newest
    /// entry is the one a sample belongs to.
    wheel_sources: RefCell<VecDeque<(i16, bool)>>,
}

const WM_SETTINGCHANGE: u32 = 0x001A;
const WM_MOUSEWHEEL: u32 = 0x020A;
const WM_MOUSEHWHEEL: u32 = 0x020E;
const SPI_GETWHEELSCROLLLINES: u32 = 0x0068;
const SPI_GETWHEELSCROLLCHARS: u32 = 0x006C;
/// `INPUT_MESSAGE_DEVICE_TYPE`: the message came from a touchpad.
const IMDT_TOUCHPAD: u32 = 0x10;

#[repr(C)]
struct InputMessageSource {
    device_type: u32,
    origin_id: u32,
}

#[link(name = "user32")]
extern "system" {
    fn SystemParametersInfoW(
        action: u32,
        param: u32,
        value: *mut std::ffi::c_void,
        update: u32,
    ) -> i32;
    fn GetCurrentInputMessageSource(source: *mut InputMessageSource) -> i32;
}

/// What the system's lines-per-notch value means. Zero is "the wheel does
/// not scroll"; the largest value is "a page at a time".
pub(crate) fn wheel_lines(raw: u32) -> WheelLines {
    match raw {
        0 => WheelLines::Off,
        u32::MAX => WheelLines::Page,
        lines => WheelLines::Lines(f64::from(lines)),
    }
}

/// The wheel settings as the system has them now, with the defaults for
/// anything it would not say.
fn read_scroll_settings() -> ScrollSettings {
    let mut settings = ScrollSettings::default();
    let read = |action: u32| {
        let mut value: u32 = 0;
        // SAFETY: both actions write one UINT through the pointer, which is
        // to a live `u32` for the length of the call.
        let ok =
            unsafe { SystemParametersInfoW(action, 0, std::ptr::from_mut(&mut value).cast(), 0) };
        (ok != 0).then_some(value)
    };
    if let Some(lines) = read(SPI_GETWHEELSCROLLLINES) {
        settings.wheel_lines = wheel_lines(lines);
    }
    if let Some(chars) = read(SPI_GETWHEELSCROLLCHARS) {
        settings.wheel_chars = f64::from(chars);
    }
    settings
}

/// Remember a wheel message for the sample winit is about to make of it.
///
/// EXPERIMENTAL. Whether the system reports a touchpad here at all for the
/// wheel messages it synthesises from one is not documented, and reportedly
/// depends on the thread declaring itself touchpad-aware, which changes
/// which pointer messages a window receives and is deliberately not done.
/// So this only ever *adds* a hint when the system volunteers one; nothing
/// is concluded from its absence. On the one machine it has been tried on,
/// a recording of 89 touchpad messages carried no hint at all.
fn note_wheel(shared: &Shared, wparam: usize) {
    let mut source = InputMessageSource {
        device_type: 0,
        origin_id: 0,
    };
    // SAFETY: writes one INPUT_MESSAGE_SOURCE through a pointer to a live
    // one; called while the wheel message is being dispatched, which is when
    // the function is defined to answer.
    let known = unsafe { GetCurrentInputMessageSource(&mut source) } != 0;
    let touchpad = known && source.device_type & IMDT_TOUCHPAD != 0;
    if let Ok(mut seen) = shared.win.wheel_sources.try_borrow_mut() {
        if seen.len() >= 64 {
            seen.pop_front();
        }
        seen.push_back((high_word(wparam) as i16, touchpad));
    }
}

/// Add what the system said about the device to a sample made from a wheel
/// message, if it said anything.
pub(crate) fn hinted(shared: &Shared, mut sample: WheelSample) -> WheelSample {
    if sample.unit != WheelUnit::Windows120 {
        return sample;
    }
    let raw = if sample.y != 0.0 { sample.y } else { sample.x };
    let Ok(mut seen) = shared.win.wheel_sources.try_borrow_mut() else {
        return sample;
    };
    // The horizontal wheel's sign is turned round on its way through winit.
    while let Some((delta, touchpad)) = seen.pop_back() {
        if f64::from(delta).abs() == raw.abs() {
            if touchpad {
                sample.source = SourceHint::Touchpad;
            }
            break;
        }
    }
    seen.clear();
    sample
}

/// The wheel settings, if they have been read since this was last asked.
pub(crate) fn take_scroll_settings(shared: &Shared) -> Option<ScrollSettings> {
    shared.win.wheel_settings.take()
}

struct Backdrop {
    background: w::COLORREF,
    text: w::COLORREF,
    brush: DeleteObjectGuard<w::HBRUSH>,
}

struct Backend {
    parent: HWND,
    /// By control index. `None` where creation failed.
    children: Vec<Option<HWND>>,
    font: Option<DeleteObjectGuard<w::HFONT>>,
    font_dpi: u32,
    /// The leaked `Rc<Shared>` the subclass procedures read.
    shared: *const Shared,
}

fn copy(hwnd: &HWND) -> HWND {
    // SAFETY: an HWND is a plain handle value; the copy does not own it.
    unsafe { hwnd.raw_copy() }
}

fn send(hwnd: &HWND, message: u32, wparam: usize, lparam: isize) -> isize {
    // SAFETY: every message sent through here is a documented control
    // message whose parameters are integers, or pointers that outlive the
    // synchronous call (see the call sites).
    unsafe {
        hwnd.SendMessage(w::msg::WndMsg::new(
            co::WM::from_raw(message),
            wparam,
            lparam,
        ))
    }
}

fn colour(rgb: u32) -> w::COLORREF {
    w::COLORREF::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// Start putting controls in `window`.
pub(crate) fn attach(shared: &Rc<Shared>, window: &Window) -> Result<(), String> {
    detach(shared);
    let handle = window.window_handle().map_err(|error| error.to_string())?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return Err("the window is not a Win32 window".to_owned());
    };
    // SAFETY: winit reports the HWND of a window it owns and that is alive
    // for this call; the handle is only used on this, its own, thread.
    let parent = unsafe { HWND::from_ptr(handle.hwnd.get() as *mut std::ffi::c_void) };

    w::InitCommonControls();
    parent.set_style(parent.style() | co::WS::CLIPCHILDREN);

    let data = Rc::into_raw(shared.clone());
    // SAFETY: `parent_proc` has the SUBCLASSPROC signature, and `data` stays
    // valid until the subclass is removed (module invariants).
    if let Err(error) =
        unsafe { parent.SetWindowSubclass(parent_proc, PARENT_SUBCLASS, data as usize) }
    {
        // SAFETY: the subclass was not installed, so this is the only owner.
        drop(unsafe { Rc::from_raw(data) });
        return Err(format!("SetWindowSubclass: {error}"));
    }
    shared.win.wheel_settings.set(Some(read_scroll_settings()));
    *shared.win.backend.borrow_mut() = Some(Backend {
        parent,
        children: Vec::new(),
        font: None,
        font_dpi: 0,
        shared: data,
    });
    Ok(())
}

/// Remove every control and stop listening to the window. Safe to call when
/// nothing is attached, and after the window is already gone.
pub(crate) fn detach(shared: &Shared) {
    let Ok(mut slot) = shared.win.backend.try_borrow_mut() else {
        return;
    };
    let Some(backend) = slot.take() else {
        return;
    };
    drop(slot);
    for child in backend.children.iter().flatten() {
        let _ = child.DestroyWindow();
    }
    release(backend);
}

/// Remove the parent subclass and give up its reference to `Shared`.
fn release(backend: Backend) {
    let _ = backend
        .parent
        .RemoveWindowSubclass(parent_proc, PARENT_SUBCLASS);
    // SAFETY: `backend.shared` came from `Rc::into_raw` in `attach`, and a
    // `Backend` is consumed here exactly once.
    drop(unsafe { Rc::from_raw(backend.shared) });
}

/// Bring the platform controls in line with the frame just drawn.
pub(crate) fn apply(shared: &Shared, commands: Vec<Command>, events_read: bool) {
    let Ok(mut slot) = shared.win.backend.try_borrow_mut() else {
        return;
    };
    let Some(backend) = slot.as_mut() else {
        return;
    };
    backend.refresh_font();
    let count = shared.model.borrow().count();
    while backend.children.len() < count {
        let id = ControlId(backend.children.len() as u32);
        let snapshot = shared.model.borrow().snapshot(id);
        let child = snapshot.and_then(|snapshot| backend.create(id, &snapshot).ok());
        backend.children.push(child);
    }
    for command in commands {
        backend.run(command);
    }
    if events_read {
        // The application reacted to an event while drawing; show the result.
        let _ = backend.parent.InvalidateRect(None, false);
    }
}

pub(crate) fn set_backdrop(shared: &Shared, colours: Option<(u32, u32)>) {
    let backdrop = colours.and_then(|(background, text)| {
        let background = colour(background);
        Some(Backdrop {
            background,
            text: colour(text),
            brush: w::HBRUSH::CreateSolidBrush(background).ok()?,
        })
    });
    if let Ok(mut slot) = shared.win.backdrop.try_borrow_mut() {
        *slot = backdrop;
    }
    if let Ok(slot) = shared.win.backend.try_borrow() {
        for child in slot
            .iter()
            .flat_map(|backend| backend.children.iter().flatten())
        {
            let _ = child.InvalidateRect(None, true);
        }
    }
}

pub(crate) fn hwnd(shared: &Shared, id: ControlId) -> Option<HWND> {
    let slot = shared.win.backend.try_borrow().ok()?;
    let child = slot.as_ref()?.children.get(id.0 as usize)?.as_ref()?;
    Some(copy(child))
}

impl Backend {
    fn child(&self, id: ControlId) -> Option<&HWND> {
        self.children.get(id.0 as usize)?.as_ref()
    }

    /// The system UI face at the window's DPI, remade when the window moves
    /// to a display with a different one.
    fn refresh_font(&mut self) {
        let dpi = self.parent.GetDpiForWindow();
        if dpi == self.font_dpi || dpi == 0 {
            return;
        }
        self.font_dpi = dpi;
        let height = -((9 * dpi as i32 + 36) / 72);
        let font = w::HFONT::CreateFont(
            SIZE::with(0, height),
            0,
            0,
            co::FW::NORMAL,
            false,
            false,
            false,
            co::CHARSET::DEFAULT,
            co::OUT_PRECIS::DEFAULT,
            co::CLIP::DEFAULT_PRECIS,
            co::QUALITY::CLEARTYPE,
            co::PITCH::DEFAULT,
            "Segoe UI",
        )
        .ok();
        if let Some(font) = &font {
            for child in self.children.iter().flatten() {
                send(child, WM_SETFONT, font.ptr() as usize, 1);
            }
        }
        // The old font is deleted only now, after no control uses it.
        self.font = font;
    }

    fn create(&self, id: ControlId, snapshot: &Snapshot) -> Result<HWND, String> {
        if id.0 >= MAX_CONTROLS {
            return Err("too many native controls".to_owned());
        }
        let (class, kind_style, ex_style) = match snapshot.kind {
            ControlKind::Button => ("BUTTON", WS_TABSTOP, 0),
            ControlKind::Toggle => ("BUTTON", WS_TABSTOP | BS_AUTOCHECKBOX | BS_PUSHLIKE, 0),
            ControlKind::Checkbox => ("BUTTON", WS_TABSTOP | BS_AUTOCHECKBOX, 0),
            ControlKind::Combo => (
                "COMBOBOX",
                WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST | CBS_HASSTRINGS,
                0,
            ),
            ControlKind::Edit => ("EDIT", WS_TABSTOP | ES_AUTOHSCROLL, WS_EX_CLIENTEDGE),
            ControlKind::Label => ("STATIC", SS_LEFTNOWORDWRAP | SS_NOPREFIX, 0),
        };
        let mut style = WS_CHILD | WS_CLIPSIBLINGS | kind_style;
        if !snapshot.enabled {
            style |= WS_DISABLED;
        }
        // SAFETY: the classes are system control classes, which bring their
        // own window procedures; the parent is the live window `attach` was
        // given; the style bits are valid for each class. Created hidden.
        let child = unsafe {
            HWND::CreateWindowEx(
                co::WS_EX::from_raw(ex_style),
                w::AtomStr::Str(w::WString::from_str(class)),
                Some(&snapshot.text),
                co::WS::from_raw(style),
                POINT::new(),
                SIZE::new(),
                Some(&self.parent),
                w::IdMenu::Id(BASE_ID + id.0 as u16),
                &self.parent.hinstance(),
                None,
            )
        }
        .map_err(|error| format!("CreateWindowEx({class}): {error}"))?;

        if let Some(font) = &self.font {
            send(&child, WM_SETFONT, font.ptr() as usize, 1);
        }
        match snapshot.kind {
            ControlKind::Toggle | ControlKind::Checkbox => {
                send(&child, BM_SETCHECK, snapshot.checked as usize, 0);
            }
            ControlKind::Combo => {
                for item in &snapshot.items {
                    let text = w::WString::from_str(item);
                    // The string is copied by the control during the call.
                    send(&child, CB_ADDSTRING, 0, text.as_ptr() as isize);
                }
                select(&child, snapshot.selected);
            }
            ControlKind::Edit => {
                // SAFETY: `edit_proc` has the SUBCLASSPROC signature; its
                // reference data outlives the control (module invariants).
                let _ = unsafe {
                    child.SetWindowSubclass(edit_proc, id.0 as usize, self.shared as usize)
                };
            }
            ControlKind::Button | ControlKind::Label => {}
        }
        if let Some(rect) = snapshot.rect {
            place(&child, snapshot.kind, rect);
        }
        Ok(child)
    }

    fn run(&self, command: Command) {
        match command {
            Command::Place(id, rect) => {
                if let Some(child) = self.child(id) {
                    let kind = class_kind(child);
                    place(child, kind, rect);
                }
            }
            Command::Hide(id) => {
                if let Some(child) = self.child(id) {
                    // A hidden window that keeps focus swallows the keyboard.
                    if HWND::GetFocus().as_ref() == Some(child) {
                        self.parent.SetFocus();
                    }
                    child.ShowWindow(co::SW::HIDE);
                }
            }
            Command::Enable(id, enabled) => {
                if let Some(child) = self.child(id) {
                    if !enabled && HWND::GetFocus().as_ref() == Some(child) {
                        self.parent.SetFocus();
                    }
                    child.EnableWindow(enabled);
                }
            }
            Command::SetText(id, text) => {
                if let Some(child) = self.child(id) {
                    let _ = child.SetWindowText(&text);
                }
            }
            Command::SetChecked(id, checked) => {
                if let Some(child) = self.child(id) {
                    send(child, BM_SETCHECK, checked as usize, 0);
                }
            }
            Command::Select(id, selected) => {
                if let Some(child) = self.child(id) {
                    select(child, selected);
                }
            }
            Command::Focus(id) => {
                if let Some(child) = self.child(id) {
                    child.SetFocus();
                }
            }
            Command::FocusCanvas => {
                self.parent.SetFocus();
            }
        }
    }
}

fn select(combo: &HWND, selected: Option<usize>) {
    // CB_SETCURSEL takes -1 for "no selection".
    send(combo, CB_SETCURSEL, selected.unwrap_or(usize::MAX), 0);
}

/// Whether a control is a combo, which is sized differently. Asked of the
/// window rather than the model, so `run` needs no model borrow.
fn class_kind(child: &HWND) -> ControlKind {
    match child.GetClassName() {
        Ok(name) if name.eq_ignore_ascii_case("COMBOBOX") => ControlKind::Combo,
        _ => ControlKind::Button,
    }
}

fn place(child: &HWND, kind: ControlKind, rect: Rect) {
    let width = rect.w.max(0);
    let mut height = rect.h.max(0);
    if kind == ControlKind::Combo {
        // A combo's height is its dropped height on unthemed Windows, where
        // a list as short as the closed box could not be opened. Themed
        // Windows ignores it and sizes the list itself.
        height = height.saturating_mul(12);
    }
    let _ = child.MoveWindow(POINT::with(rect.x, rect.y), SIZE::with(width, height), true);
    child.ShowWindow(co::SW::SHOWNA);
}

fn low_word(value: usize) -> u16 {
    (value & 0xFFFF) as u16
}

fn high_word(value: usize) -> u16 {
    ((value >> 16) & 0xFFFF) as u16
}

/// The subclass on the winit window: hears what the controls report.
extern "system" fn parent_proc(
    hwnd: HWND,
    message: co::WM,
    wparam: usize,
    lparam: isize,
    _subclass: usize,
    data: usize,
) -> isize {
    // SAFETY: `data` is the `Rc<Shared>` leaked by `attach`, alive until the
    // subclass is removed (module invariants).
    let shared = unsafe { &*(data as *const Shared) };
    let pass = || {
        // SAFETY: forwards this exact message to the next procedure in the
        // subclass chain, from inside a subclass procedure.
        unsafe { hwnd.DefSubclassProc(w::msg::WndMsg::new(message, wparam, lparam)) }
    };

    if message == co::WM::COMMAND && lparam != 0 {
        on_command(shared, &hwnd, wparam, lparam);
    } else if message == co::WM::CTLCOLORSTATIC || message == co::WM::CTLCOLORBTN {
        if let Some(brush) = on_colour(shared, wparam, lparam) {
            return brush;
        }
    } else if message == co::WM::LBUTTONDOWN
        || message == co::WM::RBUTTONDOWN
        || message == co::WM::MBUTTONDOWN
    {
        // A click on the drawn frame takes the keyboard back from a text
        // field, as a click outside a field does anywhere else.
        if HWND::GetFocus().as_ref() != Some(&hwnd) {
            hwnd.SetFocus();
        }
    } else if message.raw() == WM_MOUSEWHEEL || message.raw() == WM_MOUSEHWHEEL {
        note_wheel(shared, wparam);
    } else if message.raw() == WM_SETTINGCHANGE {
        // Sent for any system setting; reading two numbers again is cheap.
        shared.win.wheel_settings.set(Some(read_scroll_settings()));
    } else if message == co::WM::NCDESTROY {
        let result = pass();
        // The window is gone and took its children with it. `shared` is not
        // used after this: releasing may free it.
        let backend = shared
            .win
            .backend
            .try_borrow_mut()
            .ok()
            .and_then(|mut slot| slot.take());
        if let Some(backend) = backend {
            release(backend);
        }
        return result;
    }
    pass()
}

fn on_command(shared: &Shared, parent: &HWND, wparam: usize, lparam: isize) {
    let (control, code) = (low_word(wparam), high_word(wparam));
    if control < BASE_ID {
        return;
    }
    let id = ControlId((control - BASE_ID) as u32);
    let Some(kind) = shared
        .model
        .try_borrow()
        .ok()
        .and_then(|model| model.kind(id))
    else {
        return;
    };
    // SAFETY: for a control notification, lParam is the control's HWND; it
    // is alive for the duration of the message it is sending.
    let child = unsafe { HWND::from_ptr(lparam as *mut std::ffi::c_void) };

    // Read the control first, then borrow the model: reading sends messages.
    let mut refocus = false;
    let report: Box<dyn FnOnce(&mut crate::model::Model)> = match (kind, code) {
        (ControlKind::Button, BN_CLICKED) => {
            refocus = true;
            Box::new(move |model| model.user_event(NativeEvent::Clicked(id)))
        }
        (ControlKind::Toggle | ControlKind::Checkbox, BN_CLICKED) => {
            refocus = true;
            let checked = send(&child, BM_GETCHECK, 0, 0) == 1;
            Box::new(move |model| model.user_toggled(id, checked))
        }
        (ControlKind::Combo, CBN_SELCHANGE) => {
            // While the list is open the user is still choosing with the
            // arrow keys; taking focus would close it under them.
            refocus = send(&child, CB_GETDROPPEDSTATE, 0, 0) == 0;
            let selected = usize::try_from(send(&child, CB_GETCURSEL, 0, 0)).ok();
            Box::new(move |model| model.user_selected(id, selected))
        }
        (ControlKind::Combo, CBN_SELENDCANCEL) => {
            refocus = true;
            Box::new(|_| {})
        }
        (ControlKind::Edit, EN_CHANGE) => {
            let text = child.GetWindowText().unwrap_or_default();
            Box::new(move |model| model.user_text(id, text))
        }
        _ => return,
    };
    if let Ok(mut model) = shared.model.try_borrow_mut() {
        report(&mut model);
    }
    if refocus {
        parent.SetFocus();
    }
    let _ = parent.InvalidateRect(None, false);
}

/// Colours for a label or check box, if the application set a backdrop.
fn on_colour(shared: &Shared, wparam: usize, lparam: isize) -> Option<isize> {
    let slot = shared.win.backdrop.try_borrow().ok()?;
    let backdrop = slot.as_ref()?;
    // SAFETY: for WM_CTLCOLOR*, lParam is the control's HWND and wParam its
    // device context, both valid for the duration of the message.
    let (child, hdc) = unsafe {
        (
            HWND::from_ptr(lparam as *mut std::ffi::c_void),
            w::HDC::from_ptr(wparam as *mut std::ffi::c_void),
        )
    };
    // Only this crate's controls; another library's children keep theirs.
    if child.GetDlgCtrlID().ok()? < BASE_ID {
        return None;
    }
    let _ = hdc.SetBkColor(backdrop.background);
    let _ = hdc.SetTextColor(backdrop.text);
    Some(backdrop.brush.ptr() as isize)
}

/// The subclass on each text field: Enter submits, Escape cancels, and both
/// hand the keyboard back, which a bare EDIT control never does.
extern "system" fn edit_proc(
    hwnd: HWND,
    message: co::WM,
    wparam: usize,
    lparam: isize,
    subclass: usize,
    data: usize,
) -> isize {
    if message == co::WM::KEYDOWN && (wparam == VK_RETURN || wparam == VK_ESCAPE) {
        // SAFETY: as in `parent_proc`; the control dies before the data.
        let shared = unsafe { &*(data as *const Shared) };
        let id = ControlId(subclass as u32);
        if let Ok(mut model) = shared.model.try_borrow_mut() {
            model.user_event(if wparam == VK_RETURN {
                NativeEvent::Submitted(id)
            } else {
                NativeEvent::Cancelled(id)
            });
        }
        if let Ok(parent) = hwnd.GetParent() {
            parent.SetFocus();
            let _ = parent.InvalidateRect(None, false);
        }
        return 0;
    }
    if message == co::WM::CHAR && (wparam == VK_RETURN || wparam == VK_ESCAPE || wparam == VK_TAB) {
        // The characters those keys also produce; a single-line field beeps
        // at them.
        return 0;
    }
    if message == co::WM::NCDESTROY {
        let _ = hwnd.RemoveWindowSubclass(edit_proc, subclass);
    }
    // SAFETY: forwards this exact message down the subclass chain.
    unsafe { hwnd.DefSubclassProc(w::msg::WndMsg::new(message, wparam, lparam)) }
}

#[cfg(test)]
mod wheel_tests {
    use super::*;
    use liminalkit_shell::scroll::GesturePhase;
    use std::time::Duration;

    #[test]
    fn the_systems_lines_per_notch_value_has_two_special_meanings() {
        assert_eq!(wheel_lines(3), WheelLines::Lines(3.0));
        assert_eq!(wheel_lines(0), WheelLines::Off);
        assert_eq!(wheel_lines(u32::MAX), WheelLines::Page);
    }

    #[test]
    fn the_system_answers_when_asked_for_its_wheel_settings() {
        // Whatever this machine is set to, it is one of the three kinds and
        // the sideways count is a count.
        let settings = read_scroll_settings();
        assert!(settings.wheel_chars >= 0.0);
        if let WheelLines::Lines(lines) = settings.wheel_lines {
            assert!((1.0..=100.0).contains(&lines), "{lines} lines a notch");
        }
    }

    #[test]
    fn a_sample_takes_the_hint_of_the_message_it_was_made_from() {
        let shared = Shared {
            model: RefCell::new(crate::model::Model::default()),
            win: State::default(),
        };
        let sample = |raw: f32| {
            WheelSample::from_lines(0.0, raw / 120.0, true, GesturePhase::Moved, Duration::ZERO)
        };
        // The system said touchpad for a delta of 7.
        shared.win.wheel_sources.borrow_mut().push_back((7, true));
        assert_eq!(hinted(&shared, sample(7.0)).source, SourceHint::Touchpad);
        // It said nothing in particular for a notch: no hint is invented.
        shared
            .win
            .wheel_sources
            .borrow_mut()
            .push_back((-120, false));
        assert_eq!(hinted(&shared, sample(-120.0)).source, SourceHint::Unknown);
        // A sample with no message on record is left as it came.
        assert_eq!(hinted(&shared, sample(30.0)).source, SourceHint::Unknown);
        // A stale entry for some other message is not applied to this one.
        shared.win.wheel_sources.borrow_mut().push_back((9, true));
        assert_eq!(hinted(&shared, sample(30.0)).source, SourceHint::Unknown);
    }
}
