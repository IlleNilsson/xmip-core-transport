//! One receive's wait over a kept listener and the connections kept open on
//! it ([`super::Serving::next`]): one readiness wait over all of them, the
//! listener's new peer accepted and kept, and the first kept connection
//! that spoke handed back for its turn.

use std::net::{SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustix::fd::AsFd;
use xcore::Clock;
use xcore::lock::Locked;

use super::{Connection, MOST_KEPT, Open, owing};
use crate::answer::Freed;
use crate::deadline::Within;
use crate::error::{Result, TransportError, classify};
use crate::socket;
use crate::turned::Turned;

/// One receive's wait: the listener, what a freed connection tells, where
/// a caller turned away is kept, the Clock a new connection's turns are
/// counted on, and how long the receive may wait.
pub(super) struct Wait<'a> {
    listener: &'a TcpListener,
    freed: &'a Freed,
    turned: &'a Mutex<Turned>,
    clock: &'a Arc<dyn Clock>,
    deadline: Option<Instant>,
    timeout: Option<Duration>,
}

impl<'a> Wait<'a> {
    /// A wait on `listener` and the connections kept on it, ending within
    /// `timeout` from now (`None` waits as long as it takes), a new
    /// connection's turns counted on `clock`.
    pub(super) fn new(
        listener: &'a TcpListener,
        freed: &'a Freed,
        turned: &'a Mutex<Turned>,
        clock: &'a Arc<dyn Clock>,
        timeout: Option<Duration>,
    ) -> Self {
        Self {
            listener,
            freed,
            turned,
            clock,
            deadline: timeout.map(|timeout| Instant::now() + timeout),
            timeout,
        }
    }

    /// Wait until the deadline for the listener or an open connection: the
    /// first connection that spoke, if one did. A new peer is accepted and
    /// kept whenever the listener is ready, so busy connections never keep
    /// one waiting; the one quiet longest is let go where too many are
    /// kept. A connection freed meanwhile ends the wait with none, so the
    /// next waits on it too. A new peer is handed to `opened` as the
    /// [`Within`] its reads go through, its deadline kept beside it for
    /// each turn; one whose connection could not be readied or opened is
    /// turned away.
    pub(super) fn spoken<C: Open>(
        &self,
        open: &mut Vec<Connection<C>>,
        opened: &mut impl FnMut(Within, SocketAddr) -> Result<C>,
    ) -> Result<Option<usize>> {
        let left = self
            .deadline
            .map(|at| at.saturating_duration_since(Instant::now()));
        if left.is_some_and(|left| left.is_zero()) {
            return Err(nothing_within(self.timeout));
        }
        let ready = self.ready(open, left)?;
        if ready.freed {
            self.freed.heard();
        }
        let mut spoke = ready.open.iter().position(|spoke| *spoke);
        if ready.listener {
            let nobody = socket::NOTHING_CONNECTED;
            let (stream, peer) = socket::accept_within(self.listener, self.timeout, nobody)?;
            let made = socket::accepted(&stream, self.timeout).and_then(|()| {
                let within = Within::timed_by(stream, self.clock);
                let deadline = within.deadline().clone();
                opened(within, peer).map(|made| (made, deadline))
            });
            match made {
                Ok((made, deadline)) => {
                    if open.len() == MOST_KEPT {
                        open.remove(0);
                        spoke = spoke.and_then(|at| at.checked_sub(1));
                    }
                    open.push(Connection {
                        open: made,
                        peer,
                        deadline,
                    });
                }
                Err(why) => self.turned.locked().away(peer, why),
            }
        }
        Ok(spoke)
    }

    /// Which of the listener, the freed connections' word and the open
    /// connections is ready.
    fn ready<C: Open>(&self, open: &[Connection<C>], within: Option<Duration>) -> Result<Ready> {
        // A busy connection is not waited on: it answers nothing until its
        // exchange is, so it is reported quiet.
        let idle: Vec<usize> = (0..open.len())
            .filter(|&at| !owing(&open[at].open))
            .collect();
        let mut sources = Vec::with_capacity(idle.len() + 2);
        sources.push(self.listener.as_fd());
        sources.push(self.freed.socket().as_fd());
        sources.extend(idle.iter().map(|&at| open[at].open.socket().as_fd()));
        loop {
            match socket::ready(&sources, within) {
                Ok(said) => {
                    let mut ready = Ready {
                        listener: said[0],
                        freed: said[1],
                        open: vec![false; open.len()],
                    };
                    for (&at, &spoke) in idle.iter().zip(&said[2..]) {
                        ready.open[at] = spoke;
                    }
                    return Ok(ready);
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => return Err(classify("waiting for a peer", &error)),
            }
        }
    }
}

/// What one wait found ready.
struct Ready {
    listener: bool,
    freed: bool,
    open: Vec<bool>,
}

fn nothing_within(timeout: Option<Duration>) -> TransportError {
    let within = timeout.unwrap_or_default().as_millis();
    TransportError::retryable(format!("nothing arrived within {within} ms"))
        .at("waiting for a peer")
}
