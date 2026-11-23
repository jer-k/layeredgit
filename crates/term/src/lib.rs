//! A Ghostty terminal emulator and an asynchronous local PTY transport.

mod emulator;
mod pty;
mod screen;

pub use emulator::Emulator;
pub use ghostty_vt::{Error as VtError, key, render::CursorVisualStyle, style::RgbColor};
pub use pty::{LocalPty, Source, SourceEvent};
pub use screen::{Cell, Cursor, Row, Screen};

/// Terminal dimensions in cells and the physical size of one cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    /// Number of columns.
    pub cols: u16,
    /// Number of rows.
    pub rows: u16,
    /// Width of one cell in pixels.
    pub cell_width: u16,
    /// Height of one cell in pixels.
    pub cell_height: u16,
}
