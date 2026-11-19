//! Pointer shapes.
//!
//! Here, below the window host, so that a control can ask for one: the
//! scrollbar wants a closed hand while its thumb is held, and a control has
//! no window to ask.

/// Pointer shapes an application can ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorShape {
    /// The platform arrow.
    #[default]
    Default,
    /// An I-beam over editable text.
    Text,
    /// A hand over something clickable.
    Pointer,
    /// A left-right arrow over a draggable vertical edge.
    ResizeHorizontal,
    /// An up-down arrow over a draggable horizontal edge.
    ResizeVertical,
    /// A top-left to bottom-right arrow over a `\` corner handle.
    ResizeDiagonalTlBr,
    /// A top-right to bottom-left arrow over a `/` corner handle.
    ResizeDiagonalTrBl,
    /// An open hand over something that can be picked up and dragged: a file
    /// tile, a card, a canvas to pan. Drawn by the toolkit on Windows, which
    /// has no hand of its own; the cursor theme's elsewhere.
    Grab,
    /// A closed hand while it is being dragged.
    Grabbing,
}

impl CursorShape {
    /// Whether the toolkit draws this shape itself on this platform rather
    /// than asking for a system cursor.
    pub fn is_drawn(self) -> bool {
        cfg!(target_os = "windows") && matches!(self, CursorShape::Grab | CursorShape::Grabbing)
    }
}
