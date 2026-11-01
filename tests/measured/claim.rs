//! A starting node's recovery of what its sends left, measured
//! (`xmip_core_transport::claim`), over a location of the file family held
//! in memory that counts its own time (`claim::Memory`).

use std::time::{Duration, Instant};

use xmip_core_transport::claim::{Claimant, Memory};
use xmip_core_transport::node::NodeLocation;

fn claimant(place: usize) -> Claimant {
    let node = NodeLocation::new(format!("process-{}/place-{place}", std::process::id()));
    Claimant::of(&node)
}

/// A file `claimant`'s send to `target` left behind: its name, no longer
/// in flight.
fn left(claimant: &Claimant, target: &str) -> String {
    claimant.sending(target).name().to_string()
}

/// A location of a hundred names: thirty this node's sends left, thirty
/// another's, forty dropped files.
fn crowded() -> Memory {
    let site = Memory::default();
    let (me, them) = (claimant(0), claimant(1));
    for file in 0..30 {
        site.drop_file(&left(&me, &format!("mine-{file}.edi")), b"ISA");
        site.drop_file(&left(&them, &format!("theirs-{file}.edi")), b"ISA");
    }
    for file in 0..40 {
        site.drop_file(&format!("order-{file}.edi"), b"ISA");
    }
    site
}

/// What a recovery costs beyond the far end's own work — its listings and
/// each removal it is asked for — over a hundred names: the fastest of five
/// rounds, each over a location made anew.
#[test]
fn the_removal_costs_no_more_than_a_millisecond_beyond_the_far_end() {
    let me = claimant(0);
    let rounds = (0..5).map(|_| {
        let site = crowded();
        let before = site.spent();
        let started = Instant::now();
        me.recover(&site).expect("recovered");
        let all = started.elapsed();
        let far_end = site.spent().saturating_sub(before);
        assert_eq!(site.names().len(), 70, "thirty removed");
        (all.saturating_sub(far_end), all, far_end)
    });
    let (beyond, all, far_end) = rounds.min().expect("five rounds");
    assert!(
        beyond <= Duration::from_millis(1),
        "{beyond:?} beyond the far end ({all:?} in all, {far_end:?} at the far end)"
    );
}
