//! liminalkit-ui: immediate-mode widgets over liminalkit-raster/-text/-shell.
//!
//! No widget tree and no identity map: a screen owns its widget state and
//! passes `&mut` to widget functions. Provenance: `pos-client-ui::widget`
//! (restaurant-pos), see `ATTRIBUTION.md`.

pub mod chrome;
pub mod dropdown;
pub mod focus;
pub mod form;
pub mod layout;
pub mod list;
pub mod palette;
#[cfg(feature = "accessibility")]
pub mod semantics;
pub mod toast;
pub mod tooltip;
pub mod widget;

pub use chrome::ButtonStyle;
pub use dropdown::Dropdown;
pub use focus::Focus;
pub use form::{CHECKBOX_SIZE, RADIO_SIZE, TOGGLE_SIZE};
pub use layout::{columns_per_row, grid, grid_content_height, grid_item, grid_visible_range};
pub use list::{ScrollList, ScrollState};
pub use palette::{Appearance, OperationalPalette};
pub use toast::{
    toast_layout, ToastAction, ToastCard, ToastLayout, ToastOutcome, TOAST_MAX_ACTIONS,
    TOAST_MAX_BODY_LINES, TOAST_WIDTH,
};
pub use tooltip::Tooltip;
pub use tooltip::TooltipStyle;
pub use widget::{
    column_widths, navigate, Cell, CellAppearance, Column, Row, TableOutcome, TableState,
    TextFieldState, Theme, Ui, Width,
};
