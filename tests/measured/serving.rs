//! A kept listener and its peers' connections, measured
//! (`xmip_core_transport::serving`), over the line protocol its tests
//! serve (`xmip_core_transport::lines`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use xcore::latency::{Rounds, woke_beside_a_plain_wake};
use xmip_core_transport::latency::TcpControl;
use xmip_core_transport::lines::{Lines, Peer, next, owed, serving, waited};
use xmip_core_transport::serving::Serving;

#[test]
fn a_connection_freed_while_a_receive_waits_is_waited_on_at_once() {
    // The receive waits as long as it takes, as a listening Location
    // does: before 2026-10-21 a connection freed meanwhile was never
    // waited on again, and the receive waited for good.
    // Held as the fastest of each five rounds beside a plain loopback
    // wake (`TcpControl`): a wake that waited its turn for a processor is
    // in some rounds only.
    const ROUNDS: usize = 25;
    let (serving, address) = serving();
    let serving = Arc::new(serving);
    let mut peer = Peer::to(&address);
    let mut control = TcpControl::start();
    let mut took = Vec::with_capacity(ROUNDS);
    for _ in 0..ROUNDS {
        peer.say("wait");
        assert_eq!(next(&serving).expect("taken").0, "wait");
        let owed = owed();
        let waiting = Arc::clone(&serving);
        let receive = std::thread::spawn(move || {
            let taken = waited(&waiting, None);
            (taken, Instant::now())
        });
        // Answered on another thread while that receive waits, the
        // connection's next line is taken at once.
        owed.write(b"ok\n").expect("answered");
        assert_eq!(peer.heard(), "ok\n");
        let said = Instant::now();
        peer.say("next");
        let (taken, at) = receive.join().expect("the receive");
        assert_eq!(taken.expect("the next line").0, "next");
        took.push(at.saturating_duration_since(said));
        assert_eq!(peer.heard(), "ok\n");
        control.poke();
    }
    let took = xcore::latency::spread("a line on a connection freed meanwhile", took);
    let machine = control.load();
    woke_beside_a_plain_wake(
        "a line taken after it was said",
        took.fastest_of_five,
        machine.fastest_of_five,
        1,
    );
}

#[test]
fn exchanges_on_a_kept_connection_take_under_a_millisecond_each() {
    // The owner's rule, held: around a payload, Xmip's own work is
    // under a millisecond. The Location's wait on a read ends it before
    // the turn would, so no read sets its socket's wait.
    let (serving, address) = serving();
    a_thousand_exchanges_each_under_a_millisecond(&serving, address);
}

#[test]
fn exchanges_whose_every_read_its_turn_bounds_take_under_a_millisecond_each() {
    // A turn shorter than the Location's wait on a read: every read is
    // given what is left of its turn (`deadline::Within`), one
    // `setsockopt` each, and an exchange still takes under a millisecond.
    let (serving, address) = serving();
    let serving = serving.turning_within(Duration::from_secs(1));
    a_thousand_exchanges_each_under_a_millisecond(&serving, address);
}

/// What one exchange wakes where a plain wake wakes one: the peer at the
/// answer, and the serving side at the peer's next line
/// (`xcore::latency::Rounds`).
const EXCHANGE_WAKES: u32 = 2;

/// A thousand lines on one kept connection of `serving` at `address`,
/// each answered before the next is said: each the millisecond beyond its
/// wakes, round by round beside a plain wake. Until 2026-11-15 the
/// thousand were held within a second together.
fn a_thousand_exchanges_each_under_a_millisecond(serving: &Serving<Lines>, address: String) {
    const ROUNDS: u32 = 1000;
    let peer = std::thread::spawn(move || {
        let mut peer = Peer::to(&address);
        for _ in 0..ROUNDS {
            peer.say("x");
            assert_eq!(peer.heard(), "ok\n");
        }
    });
    let mut rounds = Rounds::crossing(EXCHANGE_WAKES);
    for _ in 0..ROUNDS {
        rounds.round(|| next(serving)).expect("taken");
    }
    peer.join().expect("peer");
    rounds.hold("a line taken and answered on a kept connection");
}
