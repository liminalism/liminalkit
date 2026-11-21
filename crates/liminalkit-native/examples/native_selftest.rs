//! Drives real native controls from inside a running window and checks what
//! comes back. Exits 0 when every check passes, 1 with the failures printed
//! otherwise.
//!
//! ```sh
//! cargo run -p liminalkit-native --example native_selftest
//! ```
//!
//! On Windows the window opens briefly: the checks send the same messages a
//! mouse and keyboard would cause to the Win32 controls. Elsewhere there are
//! no native controls, and the test checks that this is reported honestly.

#![allow(unsafe_code)]

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use liminalkit_native::{ControlId, NativeControls, NativeEvent};
use liminalkit_raster::{Rect, WindowBuffer};
use liminalkit_shell::Scale;
use liminalkit_windowing::{run_app, PixelApp, WindowConfig};

struct SelfTest {
    native: NativeControls,
    button: ControlId,
    toggle: ControlId,
    combo: ControlId,
    edit: ControlId,
    label: ControlId,
    frame: u32,
    failures: Rc<RefCell<Vec<String>>>,
    checks: Rc<RefCell<u32>>,
    done: bool,
}

impl SelfTest {
    fn check(&mut self, ok: bool, what: &str) {
        *self.checks.borrow_mut() += 1;
        if !ok {
            self.failures
                .borrow_mut()
                .push(format!("frame {}: {what}", self.frame));
        }
    }

    fn place_all(&self, scale: Scale) {
        let rects = [
            (self.button, Rect::new(8, 8, 88, 26)),
            (self.toggle, Rect::new(104, 8, 32, 26)),
            (self.combo, Rect::new(144, 8, 120, 26)),
            (self.edit, Rect::new(8, 44, 256, 24)),
            (self.label, Rect::new(8, 76, 256, 20)),
        ];
        for (id, rect) in rects {
            self.native.place(id, scale.rect(rect));
        }
    }
}

#[cfg(target_os = "windows")]
mod os {
    use liminalkit_native::winsafe::{self as w, co, HWND};
    use liminalkit_native::{ControlId, NativeControls};

    fn send(hwnd: &HWND, message: u32, wparam: usize, lparam: isize) -> isize {
        // SAFETY: integer-only control messages to a live control.
        unsafe {
            hwnd.SendMessage(w::msg::WndMsg::new(
                co::WM::from_raw(message),
                wparam,
                lparam,
            ))
        }
    }

    pub fn exists(native: &NativeControls, id: ControlId) -> bool {
        native.hwnd(id).is_some()
    }
    pub fn visible(native: &NativeControls, id: ControlId) -> bool {
        native.hwnd(id).is_some_and(|hwnd| hwnd.IsWindowVisible())
    }
    pub fn enabled(native: &NativeControls, id: ControlId) -> bool {
        native.hwnd(id).is_some_and(|hwnd| hwnd.IsWindowEnabled())
    }
    pub fn text(native: &NativeControls, id: ControlId) -> String {
        native
            .hwnd(id)
            .and_then(|hwnd| hwnd.GetWindowText().ok())
            .unwrap_or_default()
    }
    pub fn rect(native: &NativeControls, id: ControlId) -> (i32, i32, i32) {
        let Some(hwnd) = native.hwnd(id) else {
            return (0, 0, 0);
        };
        let (Ok(parent), Ok(screen)) = (hwnd.GetParent(), hwnd.GetWindowRect()) else {
            return (0, 0, 0);
        };
        let origin = parent
            .ScreenToClient(w::POINT::with(screen.left, screen.top))
            .unwrap_or_default();
        (origin.x, origin.y, screen.right - screen.left)
    }
    /// What a mouse click does to a button.
    pub fn click(native: &NativeControls, id: ControlId) {
        if let Some(hwnd) = native.hwnd(id) {
            send(&hwnd, 0x00F5, 0, 0); // BM_CLICK
        }
    }
    pub fn is_checked(native: &NativeControls, id: ControlId) -> bool {
        native
            .hwnd(id)
            .is_some_and(|hwnd| send(&hwnd, 0x00F0, 0, 0) == 1) // BM_GETCHECK
    }
    pub fn combo_index(native: &NativeControls, id: ControlId) -> isize {
        native.hwnd(id).map_or(-2, |hwnd| send(&hwnd, 0x0147, 0, 0)) // CB_GETCURSEL
    }
    pub fn combo_count(native: &NativeControls, id: ControlId) -> isize {
        native.hwnd(id).map_or(-2, |hwnd| send(&hwnd, 0x0146, 0, 0)) // CB_GETCOUNT
    }
    /// What picking an item with the mouse does: the selection moves, then
    /// the combo tells its parent.
    pub fn pick(native: &NativeControls, id: ControlId, index: usize) {
        let Some(hwnd) = native.hwnd(id) else { return };
        send(&hwnd, 0x014E, index, 0); // CB_SETCURSEL
        let (Ok(parent), Ok(control)) = (hwnd.GetParent(), hwnd.GetDlgCtrlID()) else {
            return;
        };
        let wparam = (1usize << 16) | control as usize; // CBN_SELCHANGE
        send(&parent, 0x0111, wparam, hwnd.ptr() as isize); // WM_COMMAND
    }
    /// What typing does: the field's text changes and it notifies.
    pub fn type_text(native: &NativeControls, id: ControlId, text: &str) {
        if let Some(hwnd) = native.hwnd(id) {
            let _ = hwnd.SetWindowText(text);
        }
    }
    pub fn key_down(native: &NativeControls, id: ControlId, vk: usize) {
        if let Some(hwnd) = native.hwnd(id) {
            send(&hwnd, 0x0100, vk, 0); // WM_KEYDOWN
        }
    }
    pub fn focus(native: &NativeControls, id: ControlId) {
        if let Some(hwnd) = native.hwnd(id) {
            hwnd.SetFocus();
        }
    }
    pub fn has_focus(native: &NativeControls, id: ControlId) -> bool {
        native.hwnd(id).is_some() && HWND::GetFocus() == native.hwnd(id)
    }
    pub fn parent_has_focus(native: &NativeControls, id: ControlId) -> bool {
        let parent = native.hwnd(id).and_then(|hwnd| hwnd.GetParent().ok());
        parent.is_some() && HWND::GetFocus() == parent
    }
    pub fn parent_clips_children(native: &NativeControls, id: ControlId) -> bool {
        native
            .hwnd(id)
            .and_then(|hwnd| hwnd.GetParent().ok())
            .is_some_and(|parent| parent.style().has(co::WS::CLIPCHILDREN))
    }
}

impl PixelApp for SelfTest {
    fn render(&mut self, buffer: &mut WindowBuffer, scale: Scale) {
        buffer.pixels.fill(0x00f4_f4f4);
        self.frame += 1;
        if !self.native.is_native() {
            self.check(
                self.native.poll_event().is_none(),
                "no events without native controls",
            );
            self.done = true;
            return;
        }
        #[cfg(target_os = "windows")]
        self.windows_frame(scale);
        #[cfg(not(target_os = "windows"))]
        {
            let _ = scale;
            self.check(false, "is_native() is true on a platform with no backend");
            self.done = true;
        }
    }

    fn animation_interval(&self) -> Option<Duration> {
        Some(Duration::from_millis(15))
    }

    fn should_exit(&self) -> bool {
        self.done
    }

    fn on_host_error(&mut self, error: &str) {
        self.failures
            .borrow_mut()
            .push(format!("host error: {error}"));
        self.done = true;
    }
}

#[cfg(target_os = "windows")]
impl SelfTest {
    fn windows_frame(&mut self, scale: Scale) {
        let n = self.native.clone();
        let (button, toggle, combo, edit, label) =
            (self.button, self.toggle, self.combo, self.edit, self.label);
        // Frame 9 leaves the toolbar unplaced, as a prompt replacing it would.
        if self.frame != 9 {
            self.place_all(scale);
        }
        match self.frame {
            1 => self.check(
                !os::exists(&n, button),
                "nothing exists before the first frame ends",
            ),
            2 => {
                for (id, name) in [
                    (button, "button"),
                    (toggle, "toggle"),
                    (combo, "combo"),
                    (edit, "edit"),
                    (label, "label"),
                ] {
                    self.check(
                        os::visible(&n, id),
                        &format!("{name} is created and visible"),
                    );
                }
                self.check(
                    os::parent_clips_children(&n, button),
                    "the frame is not blitted over controls",
                );
                self.check(
                    os::text(&n, button) == "Save",
                    "the button carries its label",
                );
                self.check(os::text(&n, label) == "Ready", "the label carries its text");
                self.check(
                    os::combo_count(&n, combo) == 3,
                    "the combo has its three items",
                );
                self.check(
                    os::combo_index(&n, combo) == 0,
                    "the combo starts on its declared item",
                );
                let (x, y, width) = os::rect(&n, button);
                let want = scale.rect(Rect::new(8, 8, 88, 26));
                self.check(
                    (x, y, width) == (want.x, want.y, want.w),
                    "the button is where the layout put it",
                );

                os::focus(&n, button);
                os::click(&n, button);
                self.check(n.clicked(button), "a click on the button is reported");
                self.check(!n.clicked(button), "and reported once");
                self.check(
                    os::parent_has_focus(&n, button),
                    "a click gives the keyboard back to the canvas",
                );

                os::click(&n, toggle);
                self.check(
                    n.changed(toggle) && n.checked(toggle),
                    "a click on the toggle turns it on",
                );
            }
            3 => {
                // Caret-driven sync: the application turns the toggle off
                // and moves the combo. Neither may come back as an event.
                n.set_checked(toggle, false);
                n.set_selected(combo, Some(2));
                n.set_text(label, "Chapter 2");
                n.set_text(edit, "set by the app");
            }
            4 => {
                self.check(
                    !os::is_checked(&n, toggle),
                    "set_checked reaches the control",
                );
                self.check(
                    os::combo_index(&n, combo) == 2,
                    "set_selected reaches the control",
                );
                self.check(
                    os::text(&n, label) == "Chapter 2",
                    "set_text reaches the label",
                );
                self.check(
                    os::text(&n, edit) == "set by the app",
                    "set_text reaches the field",
                );
                self.check(
                    n.poll_event().is_none(),
                    "none of it is reported as the user's doing",
                );

                os::pick(&n, combo, 1);
                self.check(
                    n.poll_event() == Some(NativeEvent::Selected(combo, Some(1))),
                    "picking a combo item is reported",
                );
                self.check(n.selected(combo) == Some(1), "and readable");
            }
            5 => {
                os::focus(&n, edit);
                os::type_text(&n, edit, "typed");
                self.check(
                    n.poll_event() == Some(NativeEvent::TextChanged(edit, "typed".to_owned())),
                    "typing in the field is reported",
                );
                self.check(n.text(edit) == "typed", "and readable");
                self.check(
                    os::has_focus(&n, edit),
                    "the field keeps the keyboard while typing",
                );
                os::key_down(&n, edit, 0x0D);
                self.check(n.submitted(edit), "Enter submits the field");
                self.check(
                    os::parent_has_focus(&n, edit),
                    "and gives the keyboard back",
                );
            }
            6 => {
                os::focus(&n, edit);
                os::key_down(&n, edit, 0x1B);
                self.check(
                    n.poll_event() == Some(NativeEvent::Cancelled(edit)),
                    "Escape cancels the field",
                );
                self.check(
                    os::parent_has_focus(&n, edit),
                    "and gives the keyboard back",
                );
                n.set_enabled(button, false);
                n.focus(edit);
            }
            7 => {
                self.check(
                    !os::enabled(&n, button),
                    "set_enabled(false) disables the control",
                );
                self.check(
                    os::has_focus(&n, edit),
                    "focus(id) moves the keyboard to the field",
                );
                n.set_enabled(button, true);
                n.focus_canvas();
            }
            8 => {
                self.check(
                    os::parent_has_focus(&n, edit),
                    "focus_canvas() takes it back",
                );
                self.check(
                    os::enabled(&n, button),
                    "set_enabled(true) enables it again",
                );
                os::focus(&n, edit);
            }
            // Frame 9 placed nothing.
            10 => {
                // Checked on the frame after: frame 9's commands ran after
                // its render returned.
                self.check(os::exists(&n, button), "an unplaced control still exists");
            }
            11 => {
                self.check(
                    os::visible(&n, button),
                    "a control placed again is shown again",
                );
                self.done = true;
            }
            _ => {}
        }
        if self.frame == 10 {
            // Still unplaced state from frame 9 was applied before this
            // frame's render, and this frame's placement is not applied yet.
            self.check(
                !os::visible(&n, button),
                "a control that is not placed is hidden",
            );
            self.check(!os::visible(&n, edit), "including the field that had focus");
            self.check(
                os::parent_has_focus(&n, edit),
                "whose focus went back to the canvas",
            );
        }
    }
}

fn main() {
    let drawn = std::env::args().any(|arg| arg == "--drawn");
    let native = if drawn {
        NativeControls::drawn()
    } else {
        NativeControls::new()
    };
    let failures = Rc::new(RefCell::new(Vec::new()));
    let checks = Rc::new(RefCell::new(0));
    let app = SelfTest {
        button: native.button("Save"),
        toggle: native.toggle("B", false),
        combo: native.combo(&["Body", "Subhead", "Quote"], Some(0)),
        edit: native.edit(""),
        label: native.label("Ready"),
        native: native.clone(),
        frame: 0,
        failures: failures.clone(),
        checks: checks.clone(),
        done: false,
    };
    let config = WindowConfig::new("liminalkit native self-test", 320.0, 140.0);
    if let Err(error) = run_app(native.host(app), config) {
        failures.borrow_mut().push(format!("run_app: {error}"));
    }
    let failures = failures.borrow();
    let mode = if native.is_native() {
        "native"
    } else {
        "drawn"
    };
    if failures.is_empty() {
        println!(
            "native_selftest ({mode}): {} checks passed",
            checks.borrow()
        );
    } else {
        for failure in failures.iter() {
            eprintln!("FAIL {failure}");
        }
        eprintln!(
            "native_selftest ({mode}): {} of {} checks failed",
            failures.len(),
            checks.borrow()
        );
        std::process::exit(1);
    }
}
