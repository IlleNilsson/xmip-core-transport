//! The sessions a loopback has stood up and not yet taken, by address
//! (ADR-0051).
//!
//! A protocol whose two ends meet in this process — a tester and an ECU on
//! a simulated bus — cannot be found by a socket address, so its far end
//! stands a fresh session up, names it, and the near end looks the name up.
//! A fresh session per round, so rounds driven at once from several threads
//! never read each other's frames; the far end forgets its session once the
//! exchange is taken. ISO-TP, UDS, OBD-II and J1939 each wrote this map, its
//! counter and its forgetting far end until 2026-09-24.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use crate::arrived::Arrived;
use crate::error::{Result, protocol_error};
use crate::held::Held;
use crate::loopback::FarEnd;

/// Numbers the sessions, so each address names one.
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

/// The sessions stood up and not yet taken. Cloned, it is the same map: a
/// transport and the far ends it stands up share it.
pub struct Standing<S> {
    sessions: Arc<Mutex<HashMap<String, S>>>,
}

impl<S> Default for Standing<S> {
    fn default() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl<S> Clone for Standing<S> {
    fn clone(&self) -> Self {
        Self {
            sessions: Arc::clone(&self.sessions),
        }
    }
}

impl<S> fmt::Debug for Standing<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Standing")
            .field("sessions", &self.lock().len())
            .finish()
    }
}

impl<S> Standing<S> {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, S>> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Stand `session` up under a fresh address, `<scheme>://loopback/<n>`.
    pub fn stand(&self, scheme: &str, session: S) -> String {
        let address = format!(
            "{scheme}://loopback/{}",
            NEXT_SESSION.fetch_add(1, Ordering::Relaxed)
        );
        self.lock().insert(address.clone(), session);
        address
    }

    /// Forget the session at `address`, if it still stands.
    pub fn forget(&self, address: &str) {
        self.lock().remove(address);
    }

    /// Whether every session stood up has been taken.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// The far end at `address` whose one exchange is `take`, forgetting the
    /// session once it is taken, whatever the taking came to.
    #[must_use]
    pub fn far_end<F>(&self, address: String, take: F) -> Box<dyn FarEnd>
    where
        S: Send + 'static,
        F: FnOnce() -> Result<Arrived> + Send + 'static,
    {
        let standing = self.clone();
        let forgotten = address.clone();
        Box::new(Held::new(address, move || {
            let taken = take();
            standing.forget(&forgotten);
            taken
        }))
    }
}

impl<S: Clone> Standing<S> {
    /// The session standing at `address`.
    ///
    /// # Errors
    /// Where nothing stands there: an address this loopback never gave out,
    /// or one already taken.
    pub fn session(&self, address: &str) -> Result<S> {
        self.lock()
            .get(address)
            .cloned()
            .ok_or_else(|| protocol_error(format!("{address} is not a session stood up here")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_stands_under_a_fresh_address_until_its_far_end_is_taken() {
        let standing = Standing::default();
        let first = standing.stand("bus", 1u8);
        let second = standing.stand("bus", 2u8);
        assert_ne!(first, second);
        assert!(first.starts_with("bus://loopback/"));
        assert_eq!(standing.session(&second).expect("stands"), 2);
        let far = standing.far_end(first.clone(), || Ok(Arrived::new("bus://one", vec![1])));
        assert_eq!(far.address(), first);
        assert_eq!(far.take_one().expect("take").bytes, [1]);
        let error = standing.session(&first).expect_err("forgotten");
        assert!(
            error.message.contains("not a session stood up here"),
            "{error}"
        );
        standing.forget(&second);
        assert!(standing.is_empty());
    }
}
