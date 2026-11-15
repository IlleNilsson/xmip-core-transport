//! The sessions a transport keeps, measured (`xmip_core_transport::pool`),
//! against the far end its tests echo with (`xmip_core_transport::echo`).

use xcore::latency::Rounds;
use xmip_core_transport::echo::{Line, echo, echoing, opener};
use xmip_core_transport::pool::Pool;

/// What an exchange wakes where a plain wake wakes one: the far end at the
/// byte, and this end at its echo (`xcore::latency::Rounds`).
const WAKES: u32 = 2;

#[test]
fn a_thousand_exchanges_on_one_key_open_one_session() {
    const EXCHANGES: usize = 1000;
    let (address, far_end) = echoing(1, EXCHANGES);
    let pool: Pool<Line> = Pool::new();
    let shared = pool.clone();
    // A millisecond an exchange beyond its wakes, round by round beside a
    // plain wake; until 2026-11-15 the thousand were timed once together.
    let mut rounds = Rounds::crossing(WAKES);
    for n in 0..EXCHANGES {
        let by = if n % 2 == 0 { &pool } else { &shared };
        let byte = u8::try_from(n % 256).expect("a byte");
        let back = rounds
            .round(|| by.exchange(&address, opener(&address), |line| echo(line, byte)))
            .expect("echoed");
        assert_eq!(back, byte);
    }
    rounds.hold("an exchange on a kept session");
    assert_eq!(pool.opened(), 1);
    drop((pool, shared));
    assert_eq!(far_end.join().expect("far end"), 1);
}
