# liminalkit-native

Operating-system controls inside a [liminalkit](../../README.md) window.

A liminalkit GUI draws every pixel itself. With this crate the same GUI can
hand chosen controls to the platform: on Windows a declared button is a real
Win32 `BUTTON`, created through [winsafe](https://docs.rs/winsafe) and placed
at the rectangle the liminalkit layout gave it, on top of the software-drawn
frame. Everywhere else nothing native exists and the application draws the
control itself from the same state.

The window stays the ordinary liminalkit one. winit owns it and its event
loop and softbuffer presents the frame, so keyboard, IME, pointer shapes,
touch, file drop, accessibility and multiple windows are unchanged.

```sh
cargo run -p liminalkit-native --example native_controls            # native toolbar
cargo run -p liminalkit-native --example native_controls -- --drawn # same screen, drawn
cargo run -p liminalkit-native --example native_selftest            # drives the real controls
```

## Using it

Declare controls once, place them every frame they should be visible, read
what the user did:

```rust
let controls = NativeControls::new();
let save = controls.button("Save");
let bold = controls.toggle("B", false);
let app = MyApp { controls: controls.clone(), save, bold /* … */ };
run_app(controls.host(app), WindowConfig::new("editor", 800.0, 600.0))?;

// in MyApp::render, with rectangles in device pixels:
if self.controls.is_native() {
    self.controls.place(self.save, save_rect);
    self.controls.place(self.bold, bold_rect);
} else {
    // draw liminalkit buttons at the same rectangles
}
if self.controls.clicked(self.save) { /* … */ }
self.controls.set_checked(self.bold, caret_is_bold); // not an event
```

`controls.host(app)` is the one required step: it wraps the `PixelApp` so the
controls learn about the window and are reconciled after every frame.

Through the `liminalkit` facade (feature `native`), `liminalkit::native::button`,
`toggle` and `label` take a `Ui` and do the branch for you: the platform's
control where there is one, liminalkit's drawn one where there is not.

| Control | Declared with | Reports |
|---|---|---|
| Push button | `button(label)` | `clicked(id)` |
| Toggle (stays pressed) | `toggle(label, on)` | `changed(id)`, `checked(id)` |
| Check box | `checkbox(label, checked)` | `changed(id)`, `checked(id)` |
| Drop-down list | `combo(items, selected)` | `changed(id)`, `selected(id)` |
| Text field | `edit(text)` | `changed(id)`, `text(id)`, `submitted(id)` |
| Static text | `label(text)` | — |

`poll_event()` returns the same things as a queue of `NativeEvent`s.
`NativeControls::hwnd(id)` gives the Win32 handle for anything not wrapped
here, and the crate re-exports the `winsafe` it was built against.

## How it behaves

- **Placed means visible.** A control not placed during a frame is hidden.
  A prompt that takes over a toolbar strip hides the toolbar's controls by
  not placing them.
- **Setting is not an event.** `set_checked`, `set_selected` and `set_text`
  change the control without reporting back, so mirroring a toggle or a combo
  to the caret every frame cannot loop.
- **Focus comes back.** A clicked Win32 control keeps keyboard focus, which
  would send the application's typing to a button. Buttons, toggles and
  combos return focus to the canvas when the interaction ends; a text field
  keeps it until Enter (`Submitted`), Escape (`Cancelled`) or a click on the
  canvas. `focus(id)` and `focus_canvas()` move it deliberately.
- **Native is always on top.** A control is a child window above the drawn
  frame. A liminalkit dropdown, tooltip or overlay cannot cover one; do not
  place a native control on a frame where something drawn must.
- **Tab does not move between native controls.** The winit event loop does
  not run the dialog manager. Each control is reachable by mouse, and by
  `focus(id)` from an application shortcut.
- **Colours.** Buttons, combos and text fields use the system theme.
  `set_backdrop(Some((background, text)))` colours labels and check boxes so
  they sit on the drawn frame rather than on a grey box.

## Themed controls

Windows draws themed (Common Controls 6) controls only for an executable
whose manifest asks for them; without it they work but look like Windows 95.
With the MSVC toolchain an application's `build.rs` can embed it without
other tools:

```rust
println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
println!("cargo:rustc-link-arg-bins=/MANIFESTDEPENDENCY:type='win32' \
    name='Microsoft.Windows.Common-Controls' version='6.0.0.0' \
    processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'");
```

This crate's own `build.rs` does that for its examples.

## Platforms

| Platform | Native controls |
|---|---|
| Windows | Yes. |
| macOS | Not yet; `is_native()` is `false` and the application draws. Planned through AppKit. |
| Linux, others | No; `is_native()` is `false`. |

## Unsafe

The rest of the workspace forbids `unsafe`. This crate denies it everywhere
except `src/win.rs`, which is compiled on Windows only. Putting child
controls into a window another library created means subclassing its window
procedure and creating raw controls (`winsafe::gui` accepts only its own
windows as parents), and Win32 offers neither safely. The invariants that
code relies on are written at the top of that file.
