//! The sessions a transport keeps, measured (`xmip_core_transport::pool`),
//! against the far end its tests echo with (`xmip_core_transport::echo`).

use std::time::{Duration, Instant};

use xmip_core_transport::echo::{Line, echo, echoing, opener};
use xmip_core_transport::pool::Pool;

#[test]
fn a_thousand_exchanges_on_one_key_open_one_session() {
    const EXCHANGES: usize = 1000;
    let (address, far_end) = echoing(1, EXCHANGES);
    let pool: Pool<Line> = Pool::new();
    let shared = pool.clone();
    let began = Instant::now();
    for n in 0..EXCHANGES {
        let by = if n % 2 == 0 { &pool } else { &shared };
        let byte = u8::try_from(n % 256).expect("a byte");
        let back = by
            .exchange(&address, opener(&address), |line| echo(line, byte))
            .expect("echoed");
        assert_eq!(back, byte);
    }
    // Generous for a debug build under load: a millisecond an exchange.
    let took = began.elapsed();
    assert!(took < Duration::from_millis(EXCHANGES as u64), "{took:?}");
    assert_eq!(pool.opened(), 1);
    drop((pool, shared));
    assert_eq!(far_end.join().expect("far end"), 1);
}
