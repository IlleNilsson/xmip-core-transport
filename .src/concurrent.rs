//! A far end that serves every client a listener accepts, each on a thread
//! of its own, until it is let go.
//!
//! A loopback's far end serves its one client for one round (`listening.rs`);
//! a far end several near ends reach at once — the receiving nodes of a
//! cluster racing for the same files at one server (ADR-0024, amendment
//! 2026-10-13; the Playground's drop box, ADR-0028, amendment 2026-10-13) —
//! serves them all, together. FTP and SFTP both stand one up; the accepting
//! is written here once.

use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::error::{Result, classify};

/// A listener serving its clients; dropped, it stops accepting, and each
/// client's session ends as the client leaves or its reads time out.
pub struct Concurrent {
    address: String,
    stopped: Arc<AtomicBool>,
    accepting: Option<JoinHandle<()>>,
}

impl Concurrent {
    /// Serve every connection `listener` accepts with `serve`, on a thread
    /// named `name` each.
    ///
    /// # Errors
    /// Where the listener's address could not be read, or no thread
    /// started.
    pub fn start(
        listener: TcpListener,
        name: &str,
        serve: impl Fn(TcpStream) + Send + Sync + 'static,
    ) -> Result<Self> {
        let address = listener
            .local_addr()
            .map_err(|e| classify("reading the listener's address", &e))?
            .to_string();
        let stopped = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stopped);
        let serve = Arc::new(serve);
        let named = name.to_string();
        let accepting = std::thread::Builder::new()
            .name(format!("{name}-accepting"))
            .spawn(move || {
                for accepted in listener.incoming() {
                    if stopping.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = accepted else {
                        continue;
                    };
                    let serve = Arc::clone(&serve);
                    let _ = std::thread::Builder::new()
                        .name(named.clone())
                        .spawn(move || serve(stream));
                }
            })
            .map_err(|e| classify("starting a far end", &e))?;
        Ok(Self {
            address,
            stopped,
            accepting: Some(accepting),
        })
    }

    /// Where clients connect.
    #[must_use]
    pub fn address(&self) -> &str {
        &self.address
    }
}

impl Drop for Concurrent {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        // The accept waits for a client: one more lets it see the stop,
        // through the bounded connect, so a stop never waits on the
        // operating system's own schedule.
        let _ = crate::socket::connect_tcp(&self.address, Some(Duration::from_secs(1)));
        if let Some(accepting) = self.accepting.take() {
            let _ = accepting.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};

    use super::*;

    #[test]
    fn every_client_is_served_at_once_and_a_drop_stops_the_accepting() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bound");
        let far = Concurrent::start(listener, "echo", |mut stream| {
            let mut byte = [0];
            if stream.read_exact(&mut byte).is_ok() {
                let _ = stream.write_all(&byte);
            }
        })
        .expect("serving");
        // Both connected before either speaks: served together.
        let mut clients: Vec<TcpStream> = (0..2)
            .map(|_| TcpStream::connect(far.address()).expect("connected"))
            .collect();
        for (client, at) in clients.iter_mut().zip([0u8, 1]).rev() {
            client.write_all(&[at]).expect("asked");
            let mut back = [9];
            client.read_exact(&mut back).expect("answered");
            assert_eq!(back[0], at);
        }
        drop(far);
    }
}
