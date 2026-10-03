//! A receive from a store of named objects: what is listed, each fetched
//! when the runtime first reads it, deleted once the cycle accepted or
//! refused it.
//!
//! An object store — an S3 bucket, an Azure Blob container, a Google
//! Cloud Storage bucket — holds what was dropped under a name until it is
//! deleted, and has no lock, lease or rejected place a receive takes. So a
//! receive lists, and gets and deletes nothing: each listed object is an
//! [`Arrived`] whose body `GET`s it on its first read ([`fetched`]), so a
//! receive that lists a hundred objects holds none of them in memory and
//! one never read is never fetched. Its acknowledgement deletes it on
//! [`Verdict::Accepted`], and on [`Verdict::Refused`] too — a store has no
//! place for a rejected object; the runtime audited the refusal, and from
//! Message creation on the Stream is kept in Xmip (ADR-0013). On
//! [`Verdict::Failed`] it leaves it, and the next receive lists and gets it
//! again: at least once, never a loss. What a technology lists, gets and
//! deletes with is its own, handed in.

use std::sync::Arc;

use crate::acknowledgement::{Acknowledgement, Verdict};
use crate::arrived::Arrived;
use crate::body::fetched;
use crate::error::Result;

/// Every name `list` lists, as an arrival from `origin` of that name whose
/// body `get` fetches on its first read and whose acknowledgement `delete`s
/// it on acceptance or refusal and leaves it on failure.
///
/// # Errors
/// As `list`: nothing is fetched or deleted here.
pub fn listed<G, D>(
    list: impl FnOnce() -> Result<Vec<String>>,
    origin: impl Fn(&str) -> String,
    get: G,
    delete: D,
) -> Result<Vec<Arrived>>
where
    G: Fn(&str) -> Result<Vec<u8>> + Send + Sync + 'static,
    D: Fn(&str) -> Result<()> + Send + Sync + 'static,
{
    let (get, delete) = (Arc::new(get), Arc::new(delete));
    Ok(list()?
        .into_iter()
        .map(|name| {
            let from = origin(&name);
            let (fetching, named) = (Arc::clone(&get), name.clone());
            let body = fetched(move || fetching(&named));
            let delete = Arc::clone(&delete);
            let acknowledgement = Acknowledgement::deferred(move |verdict| match verdict {
                Verdict::Accepted | Verdict::Refused(_) => delete(&name),
                Verdict::Failed => Ok(()),
            });
            Arrived::new(from, body, acknowledgement)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acknowledgement::Refusal;
    use crate::error::protocol_error;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    type Store = Arc<Mutex<(BTreeMap<String, Vec<u8>>, Vec<String>)>>;

    /// A store holding `objects`, and what was asked of it.
    fn store(objects: &[(&str, &[u8])]) -> Store {
        let held = objects
            .iter()
            .map(|(name, bytes)| ((*name).to_string(), bytes.to_vec()))
            .collect();
        Arc::new(Mutex::new((held, Vec::new())))
    }

    fn receive(store: &Store) -> Result<Vec<Arrived>> {
        let (getting, deleting) = (Arc::clone(store), Arc::clone(store));
        listed(
            || Ok(store.lock().expect("lock").0.keys().cloned().collect()),
            |name| format!("s3://C1/{name}"),
            move |name| {
                let mut held = getting.lock().expect("lock");
                held.1.push(format!("get {name}"));
                held.0
                    .get(name)
                    .cloned()
                    .ok_or_else(|| protocol_error("no such object"))
            },
            move |name| {
                let mut held = deleting.lock().expect("lock");
                held.1.push(format!("delete {name}"));
                held.0.remove(name);
                Ok(())
            },
        )
    }

    #[test]
    fn only_what_is_read_is_got_and_accepted_or_refused_is_deleted_failed_kept() {
        let store = store(&[("R1", b"one"), ("R2", b""), ("R3", b"three")]);
        let mut arrived = receive(&store).expect("listed").into_iter();
        assert!(store.lock().expect("lock").1.is_empty(), "nothing got yet");
        let one = arrived.next().expect("R1");
        assert!(one.defers());
        assert_eq!(one.origin_uri, "s3://C1/R1");
        assert_eq!(one.taken().expect("taken").bytes, b"one");
        arrived.next().expect("R2").failed().expect("left");
        arrived
            .next()
            .expect("R3")
            .refused(Refusal::Unacceptable)
            .expect("deleted");
        let asked = store.lock().expect("lock").1.clone();
        assert_eq!(asked, ["get R1", "delete R1", "delete R3"]);
        let again = receive(&store).expect("listed again");
        let names: Vec<_> = again.iter().map(|a| a.origin_uri.clone()).collect();
        assert_eq!(names, ["s3://C1/R2"], "the failed one, and only it");
    }

    #[test]
    fn a_failed_list_is_the_receives_failure_and_a_failed_get_its_bodys() {
        let error = listed(
            || Err(protocol_error("403 AccessDenied")),
            str::to_string,
            |_| Ok(Vec::new()),
            |_| Ok(()),
        )
        .expect_err("not listed");
        assert_eq!(error.message, "403 AccessDenied");
        let store = store(&[("R1", b"one")]);
        let arrived = receive(&store).expect("listed").remove(0);
        store.lock().expect("lock").0.clear();
        let error = arrived.taken().expect_err("gone before it was read");
        assert!(error.message.contains("no such object"), "{error}");
    }
}
