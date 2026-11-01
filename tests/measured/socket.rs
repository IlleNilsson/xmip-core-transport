//! A bounded accept, measured (`xmip_core_transport::socket`).

use std::io::Write;
use std::time::{Duration, Instant};

use xmip_core_transport::latency::TcpControl;
use xmip_core_transport::socket::{accept_tcp, bind_tcp, connect_tcp};

#[test]
fn a_connection_is_accepted_as_it_lands_not_after_a_nap() {
    // Each round the accept is waiting before the peer connects, so a
    // polled accept napped once a round; a readiness wait does not.
    // Until 2026-09-27 the nap was two milliseconds, and a hundred
    // rounds took two hundred.
    const ROUNDS: u32 = 100;
    let (listener, address) = bind_tcp("127.0.0.1:0").expect("bind");
    let (go, went) = std::sync::mpsc::channel::<()>();
    let (landed, lands) = std::sync::mpsc::channel::<Instant>();
    let peer = std::thread::spawn(move || {
        for _ in 0..ROUNDS {
            went.recv().expect("go");
            let mut stream = connect_tcp(&address, Some(Duration::from_secs(5))).expect("c");
            landed.send(Instant::now()).expect("landed");
            let mut back = [0u8; 1];
            std::io::Read::read_exact(&mut stream, &mut back).expect("answered");
        }
    });
    // What the accept adds once the connection has landed: the peer's
    // wake-up and its handshake are the peer's, not the accept's. Held
    // as the fastest of each five rounds against a plain loopback wake
    // beside each (xcore::latency): a wake that waited its turn for a
    // processor, under a busy machine, is in some rounds only.
    let mut control = TcpControl::start();
    let mut waited = Vec::new();
    for _ in 0..ROUNDS {
        go.send(()).expect("go");
        let (mut stream, _) =
            accept_tcp(&listener, Some(Duration::from_secs(5))).expect("accepted");
        let accepted = Instant::now();
        let connected = lands.recv().expect("landed");
        waited.push(accepted.saturating_duration_since(connected));
        stream.write_all(b"x").expect("answer");
        control.poke();
    }
    peer.join().expect("peer");
    let waited = xcore::latency::spread("an accept after the connection landed", waited);
    let machine = control.load();
    assert!(
        waited.fastest_of_five < Duration::from_millis(1) + machine.fastest_of_five,
        "an accept waited {:?}, the fastest of five, the machine's own {:?}",
        waited.fastest_of_five,
        machine.fastest_of_five
    );
}
