//! A kept listener and its peers' connections, measured
//! (`xmip_core_transport::serving`), over the line protocol its tests
//! serve (`xmip_core_transport::lines`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use xmip_core_transport::latency::TcpControl;
use xmip_core_transport::lines::{Peer, next, owed, serving, waited};

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
    assert!(
        took.fastest_of_five < Duration::from_millis(1) + machine.fastest_of_five,
        "taken {:?} after the line was said, the fastest of five, the machine's own {:?}",
        took.fastest_of_five,
        machine.fastest_of_five
    );
}

#[test]
fn exchanges_on_a_kept_connection_take_under_a_millisecond_each() {
    // The owner's rule, held: around a payload, Xmip's own work is
    // under a millisecond. A thousand lines on one kept connection, each
    // answered before the next is said.
    const ROUNDS: u32 = 1000;
    let (serving, address) = serving();
    let peer = std::thread::spawn(move || {
        let mut peer = Peer::to(&address);
        for _ in 0..ROUNDS {
            peer.say("x");
            assert_eq!(peer.heard(), "ok\n");
        }
    });
    let began = Instant::now();
    for _ in 0..ROUNDS {
        next(&serving).expect("taken");
    }
    let took = began.elapsed();
    peer.join().expect("peer");
    assert!(
        took < Duration::from_millis(u64::from(ROUNDS)),
        "{ROUNDS} exchanges took {took:?}"
    );
}
