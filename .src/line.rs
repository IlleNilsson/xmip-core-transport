//! A line: where a protocol's frames go and come from, whatever carries them.
//!
//! A serial port is one, the in-process multi-drop bus in the serial
//! technology is one, and so is the air wireless M-Bus speaks over. M-Bus and
//! HART each declared this trait for themselves, identically but for the type
//! of the name, and wireless M-Bus borrowed M-Bus's; it lives here since
//! 2026-09-24, where every technology that needs it already depends
//! (ADR-0044; open problem 24).

use std::time::Duration;

use crate::error::Result;

/// Where frames go and come from.
pub trait Line: Send + Sync {
    /// The line's name, for the origin URI.
    fn name(&self) -> String;

    /// Put a frame on the line.
    ///
    /// # Errors
    /// Where the line refused it.
    fn transmit(&self, frame: &[u8]) -> Result<()>;

    /// The next frame, or `None` when nothing arrived within `timeout`.
    ///
    /// # Errors
    /// Where the line could not be read, or carried what opens no frame.
    fn receive(&self, timeout: Duration) -> Result<Option<Vec<u8>>>;
}
