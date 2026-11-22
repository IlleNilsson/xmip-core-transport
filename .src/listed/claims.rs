//! One Receive Location's claims over an object store: what its listing
//! wins, what a starting node lets go of, and what waits to be let go.
//!
//! **An object is won by one node** (ADR-0024, amendment 2026-11-20). A
//! listing claims each object it hands over by writing its claim,
//! `<name>.xmip-claim` beside it, only where none lies — one request the
//! store decides alone ([`Store::create`]) — holding the claimed name the
//! file family's claim records (`claim::Claimant::claimed`): the object's
//! name, the node's location and the time. The node whose write the store
//! takes has the object; one that finds a claim listed beside an object,
//! or whose write the store refuses, passes it over and asks nothing more
//! of it. Nothing is renamed: an object store has no rename, and the
//! object is read where it lies.
//!
//! **A claim is let go however the cycle ends** (`hold.rs`): deleted as it
//! was written, after the object on `Accepted`, alone on `Refused` and
//! `Failed`, and when an arrival is let go without a verdict. One the
//! store does not answer is kept and let go at the next listing, so a
//! claim of a node that runs never outlives its cycle for long.
//!
//! **A node that died leaves its claims, and lets go of them as it
//! starts.** At each Location's first listing that reaches the store it
//! reads each claim listed and deletes every one an earlier start of the
//! node holds: it started again, so whatever an earlier start held,
//! nothing holds now. Only its claimant can know that. A claim of this
//! start is left, whichever of its Locations made it: a Location built
//! while another of the same start receives over the same prefix never
//! lets go of what that one holds (ADR-0024, amendment 2026-11-22; the
//! owner, *Do them all*). A claim another node's name holds is left
//! whatever its age: a claim ended by a time limit is a lease inside Xmip,
//! the thing ADR-0024 retired, and one a slow cycle outlived would let a
//! second node take an object the first still holds — the twice this
//! claim closes. A
//! node gone for good leaves its objects claimed and in place, never lost,
//! each claim saying who holds it and since when, as a file family's
//! claimed file does (ADR-0024, amendment 2026-09-26: the cluster's half
//! of the recovery is open).

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use xcore::lock::Locked;

use crate::arrived::Arrived;
use crate::body::fetched;
use crate::claim::{Artifact, Claimant, Claimed, claim_of, is_turn, may_arrive, turn_of};
use crate::error::{Result, TransportError};
use crate::refused::Refused;

use super::hold::Hold;
use super::{AsListed, CHANGED, Object, Store};

/// One Location's claims over an object store. Cloned, it is the same
/// memory: a transport and the acknowledgements it hands out share it.
pub struct Claims {
    inner: Arc<Inner>,
}

struct Inner {
    claimant: Claimant,
    /// Whether what the claimant's node's earlier starts still hold is yet
    /// to be let go, by the first listing that reaches the store.
    recovering: AtomicBool,
    refused: Refused<String, String>,
    /// The claims whose deletes the store did not answer: each claim's
    /// name, and the stamp it was written with.
    unreleased: Mutex<BTreeMap<String, String>>,
}

impl Clone for Claims {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl Claims {
    /// Claims made as `claimant`, nothing remembered yet.
    #[must_use]
    pub fn new(claimant: Claimant) -> Self {
        Self::built(claimant, false)
    }

    /// Claims made as `claimant`, a node starting: what its node's earlier
    /// starts still hold is let go by the first listing that reaches the
    /// store, so a store out of reach as the node starts does not stop it.
    #[must_use]
    pub fn recovering(claimant: Claimant) -> Self {
        Self::built(claimant, true)
    }

    fn built(claimant: Claimant, recovering: bool) -> Self {
        Self {
            inner: Arc::new(Inner {
                claimant,
                recovering: AtomicBool::new(recovering),
                refused: Refused::default(),
                unreleased: Mutex::default(),
            }),
        }
    }

    /// Who claims.
    #[must_use]
    pub fn claimant(&self) -> &Claimant {
        &self.inner.claimant
    }

    /// What lies refused, as this Location remembers it.
    #[must_use]
    pub fn refused(&self) -> &Refused<String, String> {
        &self.inner.refused
    }

    /// At most `most` of the objects `store` lists that no claim holds,
    /// that this Location does not hold refused as they lie, and that this
    /// node's claim won: each an arrival whose body gets it as listed on
    /// its first read and whose acknowledgement tells its verdict
    /// (`hold.rs`). The rest stay unclaimed in the store for a later
    /// listing: one claimed by nobody costs no request.
    ///
    /// # Errors
    /// Where the store could not be listed, or failed the first claim; a
    /// claim failing after one was won hands back what was won.
    pub fn listed(&self, store: &Arc<dyn Store>, most: usize) -> Result<Vec<Arrived>> {
        let objects = store.list()?;
        let mut freed = self.settle(&**store);
        if self.inner.recovering.swap(false, Ordering::SeqCst) {
            match self.recover(&**store, &objects) {
                Ok(returned) => freed.extend(returned),
                Err(_) => self.inner.recovering.store(true, Ordering::SeqCst),
            }
        }
        let held: HashSet<&str> = objects
            .iter()
            .map(|(name, _)| name.as_str())
            .filter(|name| is_turn(name) && !freed.contains(*name))
            .collect();
        let found: Vec<Object> = objects
            .iter()
            .filter(|(name, _)| may_arrive(name) && !held.contains(turn_of(name).as_str()))
            .cloned()
            .collect();
        let found =
            self.inner
                .refused
                .sift(found, |(name, _)| name, |(_, stamp)| Some(stamp.clone()));
        let mut won = Vec::new();
        for (name, stamp) in found {
            if won.len() >= most {
                break;
            }
            let claim = turn_of(&name);
            match store.create(&claim, self.inner.claimant.claimed(&name).as_bytes()) {
                Ok(Some(written)) => won.push(self.arrival(store, (name, stamp), (claim, written))),
                Ok(None) => {}
                Err(failed) if won.is_empty() => return Err(failed),
                Err(_) => break,
            }
        }
        Ok(won)
    }

    /// `object`, won at `store` by `claim`, as an arrival.
    fn arrival(&self, store: &Arc<dyn Store>, object: Object, claim: Object) -> Arrived {
        let origin = store.origin(&object.0);
        let (fetching, (named, as_listed)) = (Arc::clone(store), object.clone());
        let body = fetched(move || match fetching.get(&named, &as_listed)? {
            AsListed::Held(bytes) => Ok(bytes),
            AsListed::Changed => Err(TransportError::retryable(CHANGED)),
        });
        let (name, stamp) = object.clone();
        let told = Hold::acknowledgement(self.clone(), Arc::clone(store), object, claim);
        Arrived::new(
            origin,
            body,
            self.inner.refused.remembering(name, stamp, told),
        )
        .detected()
    }

    /// The claim `claim`, written with `stamp`, deleted as it was written;
    /// one gone or written over since is let go already. One the store did
    /// not answer is kept, and let go at the next listing.
    ///
    /// # Errors
    /// Where the store did not answer: why, and that it is kept.
    pub(super) fn let_go(&self, store: &dyn Store, claim: &str, stamp: &str) -> Result<()> {
        match store.delete(claim, stamp) {
            Ok(AsListed::Held(()) | AsListed::Changed) => Ok(()),
            Err(failed) => {
                let kept = (claim.to_string(), stamp.to_string());
                self.inner.unreleased.locked().insert(kept.0, kept.1);
                Err(TransportError::retryable(format!(
                    "{}; the claim {claim} is let go at the next listing",
                    failed.message
                )))
            }
        }
    }

    /// Every claim not let go yet, let go now: the names let go. Nothing
    /// kept costs one lock, and nothing is locked while the store is asked.
    fn settle(&self, store: &dyn Store) -> HashSet<String> {
        let kept = std::mem::take(&mut *self.inner.unreleased.locked());
        let mut freed = HashSet::new();
        for (claim, stamp) in kept {
            match store.delete(&claim, &stamp) {
                Ok(_) => {
                    freed.insert(claim);
                }
                Err(_) => {
                    self.inner.unreleased.locked().insert(claim, stamp);
                }
            }
        }
        freed
    }

    /// Every claim among `objects` that an earlier start of this
    /// claimant's node holds, read and deleted as listed: the node started
    /// again, so whatever an earlier start held, nothing holds now. A claim
    /// of this start, whichever of its Locations made it, and another
    /// node's are read and left ([`Claimant::is_earlier_start`]). The names
    /// let go.
    ///
    /// # Errors
    /// Where the store could not be asked.
    fn recover(&self, store: &dyn Store, objects: &[Object]) -> Result<HashSet<String>> {
        let me = &self.inner.claimant;
        let mut returned = HashSet::new();
        for (claim, stamp) in objects.iter().filter(|(name, _)| is_turn(name)) {
            let AsListed::Held(held) = store.get(claim, stamp)? else {
                continue;
            };
            let holder = std::str::from_utf8(&held).ok().and_then(claim_of);
            if holder.is_some_and(|holder| me.is_earlier_start(holder.claimant))
                && matches!(store.delete(claim, stamp)?, AsListed::Held(()))
            {
                returned.insert(claim.clone());
            }
        }
        Ok(returned)
    }

    /// Whether the object `artifact` names lies in `store` and no claim
    /// holds it.
    ///
    /// # Errors
    /// Where `artifact` names no object of `store`, or the store could not
    /// be listed.
    pub fn is_available(&self, store: &dyn Store, artifact: &Artifact) -> Result<bool> {
        let name = name_in(store, artifact)?;
        if !may_arrive(name) {
            return Ok(false);
        }
        let claim = turn_of(name);
        let listed = store.list()?;
        let lying = |wanted: &str| listed.iter().any(|(listed, _)| listed == wanted);
        Ok(lying(name) && !lying(&claim))
    }

    /// The object `artifact` names, claimed in `store` as a listing claims
    /// it; the token is the claim's stamp.
    ///
    /// # Errors
    /// Where another's claim holds it, `artifact` names no object of
    /// `store`, or the store could not be reached.
    pub fn claim(&self, store: &dyn Store, artifact: &Artifact) -> Result<Claimed> {
        let name = name_in(store, artifact)?;
        let claimed = self.inner.claimant.claimed(name);
        store
            .create(&turn_of(name), claimed.as_bytes())?
            .map(|stamp| Claimed::new(artifact.clone(), stamp))
            .ok_or_else(|| TransportError::retryable(format!("{artifact} is held")))
    }

    /// The claim `claimed` let go.
    ///
    /// # Errors
    /// Where it names no object of `store`, or the store did not answer.
    pub fn release(&self, store: &dyn Store, claimed: &Claimed) -> Result<()> {
        let name = name_in(store, &claimed.artifact)?;
        self.let_go(store, &turn_of(name), &claimed.token)
    }
}

/// The name `artifact` lies under in `store`: its address past the
/// store's own origin.
fn name_in<'a>(store: &dyn Store, artifact: &'a Artifact) -> Result<&'a str> {
    let at = store.origin("");
    artifact
        .address()
        .strip_prefix(at.as_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| TransportError::permanent(format!("{artifact} is no object of {at}")))
}

#[cfg(test)]
mod tests {
    use super::super::memory::Memory;
    use super::*;
    use crate::acknowledgement::{Acknowledgement, Refusal, Verdict};
    use crate::claim::MARK;
    use crate::node::{NodeLocation, NodeStart};
    use std::io::Read;

    const CLAIM: &str = "a.edi.xmip-claim";

    /// A start of the node in `place`, after every start before it.
    fn claimant(place: usize) -> Claimant {
        let node = NodeLocation::new(format!("process-{}/place-{place}", std::process::id()));
        Claimant::of(&NodeStart::next(node))
    }

    /// The claims of a Location on a start of the node in `place`, after
    /// every start before it.
    fn node(place: usize) -> Claims {
        Claims::new(claimant(place))
    }

    fn receive(store: &Memory, claims: &Claims) -> Result<Vec<Arrived>> {
        receive_up_to(store, usize::MAX, claims)
    }

    fn receive_up_to(store: &Memory, most: usize, claims: &Claims) -> Result<Vec<Arrived>> {
        let store: Arc<dyn Store> = Arc::new(store.clone());
        claims.listed(&store, most)
    }

    /// What `arrived`'s body reads, with the acknowledgement it is told by.
    fn read(arrived: Arrived) -> (Vec<u8>, Acknowledgement) {
        let (_, mut body, told) = arrived.into_parts();
        let mut bytes = Vec::new();
        body.read_to_end(&mut bytes).expect("read as listed");
        (bytes, told)
    }

    fn names(arrived: &[Arrived]) -> Vec<String> {
        arrived.iter().map(|a| a.origin_uri.clone()).collect()
    }

    #[test]
    fn only_what_is_read_is_got_and_only_what_is_accepted_is_deleted() {
        let store = Memory::holding(&[("a.edi", b"one"), ("b.edi", b""), ("c.edi", b"3")]);
        let claims = node(0);
        let mut arrived = receive(&store, &claims).expect("listed").into_iter();
        assert_eq!(
            store.asked(),
            [
                "list",
                "create a.edi.xmip-claim",
                "create b.edi.xmip-claim",
                "create c.edi.xmip-claim"
            ],
            "each claimed, nothing got yet"
        );
        let one = arrived.next().expect("a.edi");
        assert!(one.defers());
        assert_eq!(one.origin_uri, "memory://inbox/a.edi");
        assert_eq!(one.taken().expect("taken").bytes, b"one");
        arrived.next().expect("b.edi").failed().expect("left");
        let refused = arrived.next().expect("c.edi");
        refused.refused(Refusal::Unacceptable).expect("left");
        assert_eq!(
            store.asked(),
            [
                "get a.edi",
                "delete a.edi",
                "delete a.edi.xmip-claim",
                "delete b.edi.xmip-claim",
                "delete c.edi.xmip-claim"
            ],
            "the refused one is not deleted, and every claim is let go"
        );
        let again = receive(&store, &claims).expect("listed again");
        assert_eq!(
            names(&again),
            ["memory://inbox/b.edi"],
            "the failed one, not the refused one"
        );
        assert!(store.bytes("c.edi").is_some(), "still there");
    }

    #[test]
    fn a_listing_claims_no_more_than_asked_and_leaves_the_rest_unclaimed() {
        let store = Memory::holding(&[("a.edi", b"1"), ("b.edi", b"2"), ("c.edi", b"3")]);
        let claims = node(0);
        for arrived in receive_up_to(&store, 2, &claims).expect("listed") {
            arrived.taken().expect("accepted");
        }
        let creates = store
            .asked()
            .iter()
            .filter(|a| a.starts_with("create"))
            .count();
        assert_eq!(creates, 2, "only what is handed over is claimed");
        assert_eq!(store.names(), ["c.edi"], "one left, and unclaimed");
        let rest = receive_up_to(&store, 2, &claims).expect("listed again");
        assert_eq!(names(&rest), ["memory://inbox/c.edi"]);
    }

    #[test]
    fn a_refused_object_written_again_is_a_new_arrival() {
        let store = Memory::holding(&[("a.edi", b"one")]);
        let claims = node(0);
        let arrived = receive(&store, &claims).expect("listed").remove(0);
        arrived.refused(Refusal::Forbidden).expect("left");
        assert!(receive(&store, &claims).expect("listed").is_empty());
        store.write("a.edi", b"two");
        let again = receive(&store, &claims).expect("listed");
        assert_eq!(names(&again), ["memory://inbox/a.edi"], "another stamp");
    }

    #[test]
    fn a_replacement_between_read_and_acceptance_stays_for_the_next_listing() {
        let store = Memory::holding(&[("a.edi", b"one")]);
        let claims = node(0);
        let (bytes, told) = read(receive(&store, &claims).expect("listed").remove(0));
        assert_eq!(bytes, b"one");
        store.write("a.edi", b"two");
        told.acknowledge(Verdict::Accepted)
            .expect("the one read is done with");
        let again = receive(&store, &claims).expect("listed again").remove(0);
        assert_eq!(again.taken().expect("a new arrival").bytes, b"two");
    }

    #[test]
    fn a_replacement_between_listing_and_first_read_is_not_read_under_the_old_stamp() {
        let store = Memory::holding(&[("a.edi", b"one")]);
        let claims = node(0);
        let arrived = receive(&store, &claims).expect("listed").remove(0);
        store.write("a.edi", b"two");
        store.asked();
        let error = arrived.taken().expect_err("not the object listed");
        assert!(error.message.contains(CHANGED), "{error}");
        assert!(error.retryable, "the next listing takes it: {error}");
        assert_eq!(
            store.asked(),
            ["get a.edi", "delete a.edi.xmip-claim"],
            "nothing deleted but its claim"
        );
        let again = receive(&store, &claims).expect("listed again").remove(0);
        assert_eq!(again.taken().expect("a new arrival").bytes, b"two");
    }

    #[test]
    fn an_object_gone_before_acceptance_is_told_done() {
        let store = Memory::holding(&[("a.edi", b"one")]);
        let (_, told) = read(receive(&store, &node(0)).expect("listed").remove(0));
        store.remove("a.edi");
        told.acknowledge(Verdict::Accepted)
            .expect("nothing left to delete is done");
        assert!(store.names().is_empty(), "its claim let go too");
    }

    #[test]
    fn a_failed_list_is_the_receives_failure_and_a_failed_get_its_bodys() {
        let store = Memory::holding(&[("a.edi", b"one")]);
        let claims = node(0);
        store.failing(Some(("list", "403 AccessDenied")));
        let error = receive(&store, &claims).expect_err("not listed");
        assert_eq!(error.message, "403 AccessDenied");
        store.failing(Some(("create", "503 SlowDown")));
        let error = receive(&store, &claims).expect_err("not claimed");
        assert_eq!(error.message, "503 SlowDown");
        store.failing(Some(("get", "503 SlowDown")));
        let arrived = receive(&store, &claims).expect("listed").remove(0);
        let error = arrived.taken().expect_err("its get failed");
        assert!(error.message.contains("503 SlowDown"), "{error}");
        assert_eq!(store.names(), ["a.edi"], "left, and its claim let go");
    }

    #[test]
    fn two_nodes_listing_one_object_never_both_receive_it() {
        let store = Memory::holding(&[("a.edi", b"one")]);
        // Each node lists before either reads, as two nodes polling one
        // bucket do, and reads what it listed before either is told.
        let nodes = [node(0), node(1)];
        let listed: Vec<Arrived> = nodes
            .iter()
            .flat_map(|node| receive(&store, node).expect("listed"))
            .collect();
        let received: Vec<(Vec<u8>, Acknowledgement)> = listed.into_iter().map(read).collect();
        let bytes: Vec<&[u8]> = received.iter().map(|(bytes, _)| bytes.as_slice()).collect();
        assert_eq!(bytes, [b"one"], "one node receives it");
        for (_, told) in received {
            told.acknowledge(Verdict::Accepted).expect("accepted");
        }
        assert!(store.names().is_empty(), "the object and its claim gone");
    }

    #[test]
    fn two_nodes_racing_over_one_store_take_every_object_exactly_once() {
        const OBJECTS: usize = 300;
        let store = Memory::default();
        for object in 0..OBJECTS {
            store.write(
                &format!("order-{object}.edi"),
                object.to_string().as_bytes(),
            );
        }
        let nodes = [node(0), node(1)];
        let took: Vec<Vec<Vec<u8>>> = std::thread::scope(|scope| {
            let racing: Vec<_> = nodes
                .iter()
                .map(|claims| {
                    let store = store.clone();
                    scope.spawn(move || {
                        let mut took = Vec::new();
                        while !store.names().is_empty() {
                            for arrived in receive_up_to(&store, 7, claims).expect("listed") {
                                // A claim won after another node took the
                                // object whole reads nothing: never twice.
                                match arrived.taken() {
                                    Ok(taken) => took.push(taken.bytes),
                                    Err(gone) => assert!(gone.message.contains(CHANGED)),
                                }
                            }
                        }
                        took
                    })
                })
                .collect();
            racing
                .into_iter()
                .map(|one| one.join().expect("a node"))
                .collect()
        });
        let all: Vec<&Vec<u8>> = took.iter().flatten().collect();
        let once: std::collections::BTreeSet<&Vec<u8>> = all.iter().copied().collect();
        assert_eq!(all.len(), OBJECTS, "every object received, none twice");
        assert_eq!(once.len(), OBJECTS);
    }

    /// A store whose listing was made earlier: what a node sees that
    /// listed while another claimed.
    struct Before(Memory, Vec<Object>);

    impl Store for Before {
        fn list(&self) -> Result<Vec<Object>> {
            Ok(self.1.clone())
        }
        fn get(&self, name: &str, stamp: &str) -> Result<AsListed<Vec<u8>>> {
            self.0.get(name, stamp)
        }
        fn delete(&self, name: &str, stamp: &str) -> Result<AsListed<()>> {
            self.0.delete(name, stamp)
        }
        fn create(&self, name: &str, bytes: &[u8]) -> Result<Option<String>> {
            self.0.create(name, bytes)
        }
        fn origin(&self, name: &str) -> String {
            self.0.origin(name)
        }
    }

    #[test]
    fn an_object_another_claims_is_passed_over_and_its_claim_is_never_an_arrival() {
        let store = Memory::holding(&[("a.edi", b"one")]);
        let (first, second) = (node(0), node(1));
        let earlier = store.list().expect("listed before the first claims");
        let held = receive(&store, &first).expect("listed").remove(0);
        store.asked();
        assert!(receive(&store, &second).expect("listed").is_empty());
        assert_eq!(store.asked(), ["list"], "a claim listed beside it: unasked");
        let late: Arc<dyn Store> = Arc::new(Before(store.clone(), earlier.clone()));
        assert!(second.listed(&late, usize::MAX).expect("listed").is_empty());
        assert_eq!(
            store.asked(),
            ["create a.edi.xmip-claim"],
            "a write the store refuses: passed over"
        );
        held.taken().expect("the first node's");
        assert!(receive(&store, &second).expect("listed").is_empty());
        // Listed before the first node took it whole, its claim, read,
        // delete and letting go all between: the claim is won on nothing,
        // whose read fails and receives nothing.
        let won = second.listed(&late, usize::MAX).expect("listed").remove(0);
        let gone = won.taken().expect_err("gone since it was listed");
        assert!(gone.message.contains(CHANGED) && gone.retryable, "{gone}");
        assert!(store.names().is_empty(), "its claim let go");
    }

    #[test]
    fn a_claim_holds_the_name_the_file_familys_claim_records() {
        let store = Memory::holding(&[("in/a.edi", b"one")]);
        let claims = node(0);
        let _held = receive(&store, &claims).expect("listed");
        let held = store.bytes("in/a.edi.xmip-claim").expect("claimed");
        let held = String::from_utf8(held).expect("a name");
        let claim = claim_of(&held).expect("a claimed name");
        assert_eq!(claim.dropped, "in/a.edi");
        assert_eq!(claim.claimant, claims.claimant().as_str());
    }

    #[test]
    fn every_verdict_lets_its_claim_go_and_an_arrival_let_go_unheard_too() {
        let objects: [(&str, &[u8]); 4] = [("a", b"1"), ("b", b"2"), ("c", b"3"), ("d", b"4")];
        let store = Memory::holding(&objects);
        let mut arrived = receive(&store, &node(0)).expect("listed").into_iter();
        arrived.next().expect("a").taken().expect("accepted");
        let refused = arrived.next().expect("b");
        refused.refused(Refusal::Unacceptable).expect("left");
        arrived.next().expect("c").failed().expect("left");
        drop(arrived.next().expect("d"));
        assert_eq!(store.names(), ["b", "c", "d"], "no claim left behind");
    }

    #[test]
    fn a_claim_the_store_did_not_let_go_is_let_go_at_the_next_listing() {
        let store = Memory::holding(&[("a.edi", b"one")]);
        let claims = node(0);
        let arrived = receive(&store, &claims).expect("listed").remove(0);
        store.failing(Some(("delete", "503 SlowDown")));
        let kept = arrived.failed().expect_err("not let go");
        assert!(kept.retryable, "{kept}");
        assert!(
            kept.message.contains("let go at the next listing"),
            "{kept}"
        );
        assert!(store.bytes(CLAIM).is_some(), "kept");
        store.failing(None);
        store.asked();
        let again = receive(&store, &claims).expect("listed").remove(0);
        assert_eq!(
            store.asked(),
            ["list", "delete a.edi.xmip-claim", "create a.edi.xmip-claim"],
            "let go first, then claimed anew"
        );
        assert_eq!(again.taken().expect("received").bytes, b"one");
    }

    #[test]
    fn a_dead_nodes_claim_is_let_go_as_it_starts_again_and_by_no_other() {
        let store = Memory::holding(&[("a.edi", b"one")]);
        // The node dies holding its claim: nothing is told, nothing let go.
        std::mem::forget(receive(&store, &node(0)).expect("listed"));
        assert!(
            receive(&store, &node(1)).expect("listed").is_empty(),
            "held"
        );
        let other = Claims::recovering(claimant(1));
        assert!(receive(&store, &other).expect("listed").is_empty());
        assert!(store.bytes(CLAIM).is_some(), "another node's claim is left");
        store.asked();
        let again = Claims::recovering(claimant(0));
        let arrived = receive(&store, &again).expect("listed").remove(0);
        assert_eq!(
            store.asked(),
            [
                "list",
                "get a.edi.xmip-claim",
                "delete a.edi.xmip-claim",
                "create a.edi.xmip-claim"
            ],
            "its own claim read, let go, and the object claimed anew"
        );
        assert_eq!(arrived.taken().expect("received").bytes, b"one");
        store.write("b.edi", b"two");
        store.asked();
        let later = receive(&store, &again).expect("listed");
        assert_eq!(names(&later), ["memory://inbox/b.edi"]);
        assert!(!store.asked().iter().any(|a| a.starts_with("get")), "once");
    }

    #[test]
    fn a_location_starting_beside_a_live_one_of_its_start_leaves_its_claims() {
        let store = Memory::holding(&[("a.edi", b"one")]);
        // One Location's arrival, held unacknowledged.
        let first = node(0);
        let held = receive(&store, &first).expect("listed").remove(0);
        // Another Location of the same start, starting over the same prefix.
        let second = Claims::recovering(first.claimant().clone());
        assert!(
            receive(&store, &second).expect("listed").is_empty(),
            "not taken again"
        );
        assert!(store.bytes(CLAIM).is_some(), "the live claim is left");
        assert_eq!(held.taken().expect("the first's").bytes, b"one");
        assert!(store.names().is_empty(), "the object and its claim gone");
    }

    #[test]
    fn a_claim_naming_its_node_alone_is_an_earlier_starts_and_let_go() {
        let store = Memory::holding(&[("a.edi", b"one")]);
        let again = Claims::recovering(claimant(0));
        let (node, _) = again.claimant().as_str().split_once('~').expect("a start");
        // Written before a claim named the start of its node.
        let before = format!("a.edi{MARK}{node}.1790000000123456789");
        store.write(CLAIM, before.as_bytes());
        let arrived = receive(&store, &again).expect("listed").remove(0);
        assert_eq!(arrived.taken().expect("received").bytes, b"one");
        assert!(store.names().is_empty(), "the object and its claim gone");
    }

    #[test]
    fn a_starting_node_the_store_fails_lets_go_at_the_first_listing_that_reaches_it() {
        let store = Memory::holding(&[("a.edi", b"one")]);
        std::mem::forget(receive(&store, &node(0)).expect("listed"));
        let again = Claims::recovering(claimant(0));
        store.failing(Some(("list", "503 SlowDown")));
        assert!(receive(&store, &again).is_err(), "out of reach");
        store.failing(None);
        let arrived = receive(&store, &again).expect("listed").remove(0);
        assert_eq!(arrived.taken().expect("received").bytes, b"one");
    }

    #[test]
    fn one_object_is_claimed_by_its_address_and_released_back() {
        let store = Memory::holding(&[("in/a.edi", b"one")]);
        let (claims, other) = (node(0), node(1));
        let artifact = Artifact::new("memory://inbox/in/a.edi");
        assert!(claims.is_available(&store, &artifact).expect("asked"));
        let claimed = claims.claim(&store, &artifact).expect("claimed");
        assert!(
            !claims.is_available(&store, &artifact).expect("asked"),
            "held"
        );
        let held = other.claim(&store, &artifact).expect_err("another's");
        assert!(held.retryable, "{held}");
        assert!(receive(&store, &other).expect("listed").is_empty());
        claims.release(&store, &claimed).expect("released");
        assert!(claims.is_available(&store, &artifact).expect("asked again"));
        let elsewhere = Artifact::new("aws-s3://bucket/in/a.edi");
        let error = claims.claim(&store, &elsewhere).expect_err("not here");
        assert!(!error.retryable, "{error}");
        assert!(
            !claims
                .is_available(&store, &Artifact::new("memory://inbox/in/a.edi.xmip-claim"))
                .expect("asked"),
            "a claim is no artifact"
        );
    }
}
