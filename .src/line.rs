//! A line: where a protocol's frames go and come from, whatever carries them.
//!
//! A serial port is one, the in-process multi-drop bus in the serial
//! technology is one, and so is the air wireless M-Bus speaks over. M-Bus and
//! HART each declared this trait for themselves, identically but for the type
//! of the name, and wireless M-Bus borrowed M-Bus's; it lives here since
//! 2026-09-24, where every technology that needs it already depends
//! (ADR-0044; open problem 24).

use std::sync::Arc;
use std::time::Duration;

use crate::error::Result;
use crate::sharing::Sharing;

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
    /// Where the line could not be read, or carried what opens no frame,
    /// or a frame began and `timeout` passed before it ended: something
    /// arrived, so a cut frame is an error, never `None`.
    fn receive(&self, timeout: Duration) -> Result<Option<Vec<u8>>>;

    /// `frame` put on the line and the next frame taken off it, nothing
    /// else written to or read off the line between the two: `None` when
    /// nothing arrived within `timeout`. By default a [`Line::transmit`]
    /// and then a [`Line::receive`]; a line several Locations share holds
    /// itself across both, so an answer reaches only the Location that
    /// asked (a serial port; ADR-0064, amendment 2026-11-15).
    ///
    /// # Errors
    /// As [`Line::transmit`], then as [`Line::receive`].
    fn exchange(&self, frame: &[u8], timeout: Duration) -> Result<Option<Vec<u8>>> {
        self.transmit(frame)?;
        self.receive(timeout)
    }

    /// This line on what the Locations of its node share
    /// ([`crate::Configured::shared_by`]): a serial port takes the one port
    /// its device has on the node. `None`, as by default, for a line
    /// nothing is shared on, which stays as it is.
    fn shared_on(&self, _sharing: &Arc<dyn Sharing>) -> Option<Arc<dyn Line>> {
        None
    }
}
