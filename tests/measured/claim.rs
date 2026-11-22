//! A starting node's recovery of what its sends left, measured
//! (`xmip_core_transport::claim`), over a location of the file family held
//! in memory that counts its own time (`claim::Memory`).

use std::time::{Duration, Instant};

use xcore::fastest::FIVE;
use xcore::latency::spread;
use xmip_core_transport::claim::{Claimant, Memory};
use xmip_core_transport::node::{NodeLocation, NodeStart};

/// A start of the node at `place`, after every start before it.
fn claimant(place: usize) -> Claimant {
    let node = NodeLocation::new(format!("process-{}/place-{place}", std::process::id()));
    Claimant::of(&NodeStart::next(node))
}

/// A file `claimant`'s send to `target` left behind: its name, no longer
/// in flight.
fn left(claimant: &Claimant, target: &str) -> String {
    claimant.sending(target).name().to_string()
}

/// A location of a hundred names: thirty an earlier start of this node's
/// sends left, thirty another node's, forty dropped files.
fn crowded() -> Memory {
    let site = Memory::default();
    let (before, them) = (claimant(0), claimant(1));
    for file in 0..30 {
        site.drop_file(&left(&before, &format!("mine-{file}.edi")), b"ISA");
        site.drop_file(&left(&them, &format!("theirs-{file}.edi")), b"ISA");
    }
    for file in 0..40 {
        site.drop_file(&format!("order-{file}.edi"), b"ISA");
    }
    site
}

/// What a recovery costs beyond the far end's own work — its listings and
/// each removal it is asked for — over a hundred names: the fastest of five
/// rounds (`xcore::latency::spread`), each over a location made anew. It
/// waits on no wake, so no plain wake is measured beside it.
#[test]
fn the_removal_costs_no_more_than_a_millisecond_beyond_the_far_end() {
    let me = claimant(0);
    let beyond = (0..FIVE)
        .map(|_| {
            let site = crowded();
            let before = site.spent();
            let started = Instant::now();
            me.recover(&site).expect("recovered");
            let all = started.elapsed();
            let far_end = site.spent().saturating_sub(before);
            assert_eq!(site.names().len(), 70, "thirty removed");
            all.saturating_sub(far_end)
        })
        .collect();
    let beyond = spread("a recovery beyond the far end's own work", beyond);
    assert!(
        beyond.fastest_of_five <= Duration::from_millis(1),
        "{beyond:?} beyond the far end"
    );
}
