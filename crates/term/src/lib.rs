//! A Ghostty terminal emulator and asynchronous local PTY and Rex transports.

mod emulator;
mod pty;
mod rex;
mod screen;
mod source;

pub use emulator::Emulator;
pub use ghostty_vt::{Error as VtError, key, render::CursorVisualStyle, style::RgbColor};
pub use pty::LocalPty;
pub use rex::RexPty;
pub use screen::{Cell, Cursor, Row, Screen};
pub use source::{Source, SourceEvent};

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
