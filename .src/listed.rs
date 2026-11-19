//! A receive from a store of named objects: what is listed, each fetched
//! as it was listed when the runtime first reads it, deleted as it was
//! listed once the cycle accepted it, left and remembered once the cycle
//! refused it.
//!
//! An object store — an S3 bucket, an Azure Blob container, a Google
//! Cloud Storage bucket — holds what was dropped under a name until it is
//! deleted, and has no lock, lease or rejected place a receive takes. So a
//! receive lists, and gets and deletes nothing: each listed object is an
//! [`Arrived`] whose body `GET`s it on its first read ([`fetched`]), so a
//! receive that lists a hundred objects holds none of them in memory and
//! one never read is never fetched. Its acknowledgement deletes it on
//! [`Verdict::Accepted`] only.
//!
//! The get and the delete hold to the object as it was listed: each names
//! the stamp its listing gave it as the store's precondition — S3's and
//! Azure Blob's `ETag` in `If-Match`, Google Cloud Storage's generation in
//! `ifGenerationMatch` — and the store answers [`AsListed::Changed`] where
//! it was written again or deleted since, for no extra request. A body
//! changed so is not read: its read fails, the cycle fails, nothing is
//! deleted, and the next listing takes what lies there now as a new
//! arrival. An accepted object changed so is done with: the delete leaves
//! what lies there now for the next listing, and one deleted since is gone
//! already. So an object written again under its name after it was listed
//! is neither read under the old stamp nor deleted unread.
//!
//! A refusal is not a consumption: a Stream refused at a transport gate was
//! never written to the Ledger, so the object is the only copy, and a store
//! has no place for a refused one. On [`Verdict::Refused`] it is left where
//! it lies and remembered with the stamp its listing gave it, and no
//! receive lists it again while it lies so ([`Refused`]); one written again
//! under its name is a new arrival. On [`Verdict::Failed`] it is left, and
//! the next receive lists and gets it again: at least once, never a loss.
//! What a technology lists, gets and deletes with is its own, handed in.

use std::sync::Arc;

use crate::acknowledgement::{Acknowledgement, Verdict};
use crate::arrived::Arrived;
use crate::body::fetched;
use crate::error::{Result, TransportError, protocol_error};
use crate::refused::Refused;

/// An object as a listing names it: its name and its stamp, which changes
/// whenever it is written again — an `ETag`, Google Cloud Storage's
/// generation.
pub type Object = (String, String);

/// What a store answers when asked for an object as it was listed: under
/// its name, on the condition that it still bears the stamp the listing
/// gave it.
pub enum AsListed<T> {
    /// It still lies as listed, and this is what was asked of it.
    Held(T),
    /// It was written again or deleted since it was listed: nothing was
    /// done to it.
    Changed,
}

/// Why a body the store no longer holds as listed is not read.
const CHANGED: &str = "changed since it was listed: the next listing takes it as a new arrival";

/// Where an XML listing names its objects: the element each object is, and
/// the elements inside it that hold its name and its stamp. S3 lists
/// `<Contents>` with `<Key>` and `<ETag>`, Azure Blob `<Blob>` with `<Name>`
/// and `<Etag>`; the scan is this one (`codec::xml`, a flat scan).
pub struct Listing {
    pub entry: &'static str,
    pub name: &'static str,
    pub stamp: &'static str,
}

impl Listing {
    /// Every object `xml` names, each its name with its stamp.
    ///
    /// # Errors
    /// Where an entry names no name or no stamp, or a text holds an entity
    /// XML does not define.
    pub fn objects(&self, xml: &str) -> Result<Vec<Object>> {
        codec::xml::elements(xml, self.entry)
            .map(|entry| {
                let named = |name: &str| {
                    codec::xml::text(entry.content(), name)?.ok_or_else(|| {
                        protocol_error(format!("a listed {} without its {name}", self.entry))
                    })
                };
                Ok((named(self.name)?, named(self.stamp)?))
            })
            .collect()
    }
}

/// At most `most` of the objects `list` lists and `refused` does not hold as
/// they lie, each an arrival from `origin` of its name whose body `get`
/// fetches by its name and listed stamp on its first read, failing where
/// the store answers it changed, and whose acknowledgement `delete`s it by
/// the same on acceptance, done where it changed, leaves it and remembers
/// it in `refused` on refusal, and leaves it on failure. The rest stay in
/// the store for a later listing.
///
/// # Errors
/// As `list`: nothing is fetched or deleted here.
pub fn listed<G, D>(
    list: impl FnOnce() -> Result<Vec<Object>>,
    most: usize,
    refused: &Refused<String, String>,
    origin: impl Fn(&str) -> String,
    get: G,
    delete: D,
) -> Result<Vec<Arrived>>
where
    G: Fn(&str, &str) -> Result<AsListed<Vec<u8>>> + Send + Sync + 'static,
    D: Fn(&str, &str) -> Result<AsListed<()>> + Send + Sync + 'static,
{
    let (get, delete) = (Arc::new(get), Arc::new(delete));
    Ok(refused
        .sift(list()?, |(name, _)| name, |(_, stamp)| Some(stamp.clone()))
        .into_iter()
        .take(most)
        .map(|(name, stamp)| {
            let from = origin(&name);
            let (fetching, named, as_listed) = (Arc::clone(&get), name.clone(), stamp.clone());
            let body = fetched(move || match fetching(&named, &as_listed)? {
                AsListed::Held(bytes) => Ok(bytes),
                AsListed::Changed => Err(TransportError::retryable(CHANGED)),
            });
            let (delete, deleting, as_listed) = (Arc::clone(&delete), name.clone(), stamp.clone());
            let told = Acknowledgement::deferred(move |verdict| match verdict {
                // Gone since, or written again: what lies there now is the
                // next listing's, and what was accepted is done with.
                Verdict::Accepted => match delete(&deleting, &as_listed)? {
                    AsListed::Held(()) | AsListed::Changed => Ok(()),
                },
                Verdict::Refused(_) | Verdict::Failed => Ok(()),
            });
            Arrived::new(from, body, refused.remembering(name, stamp, told)).detected()
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acknowledgement::Refusal;
    use std::collections::BTreeMap;
    use std::io::Read;
    use std::sync::Mutex;

    /// The objects held, by name, each with its body and its stamp, and
    /// what was asked of the store. A get or a delete asking a stamp the
    /// object no longer bears, or an object gone, is answered `Changed`, as
    /// the three vendors' preconditions answer.
    type Store = Arc<Mutex<(BTreeMap<String, (Vec<u8>, String)>, Vec<String>)>>;

    /// A store holding `objects`, each tagged `"1"`.
    fn store(objects: &[(&str, &[u8])]) -> Store {
        let held = objects
            .iter()
            .map(|(name, bytes)| ((*name).to_string(), (bytes.to_vec(), "1".to_string())))
            .collect();
        Arc::new(Mutex::new((held, Vec::new())))
    }

    fn receive(store: &Store, refused: &Refused<String, String>) -> Result<Vec<Arrived>> {
        receive_up_to(store, usize::MAX, refused)
    }

    fn receive_up_to(
        store: &Store,
        most: usize,
        refused: &Refused<String, String>,
    ) -> Result<Vec<Arrived>> {
        let (getting, deleting) = (Arc::clone(store), Arc::clone(store));
        listed(
            || {
                let held = store.lock().expect("lock");
                Ok(held
                    .0
                    .iter()
                    .map(|(n, (_, tag))| (n.clone(), tag.clone()))
                    .collect())
            },
            most,
            refused,
            |name| format!("aws-s3://inbox/{name}"),
            move |name, stamp| {
                let mut held = getting.lock().expect("lock");
                held.1.push(format!("get {name}"));
                Ok(match held.0.get(name) {
                    Some((bytes, tag)) if tag == stamp => AsListed::Held(bytes.clone()),
                    _ => AsListed::Changed,
                })
            },
            move |name, stamp| {
                let mut held = deleting.lock().expect("lock");
                held.1.push(format!("delete {name}"));
                if held.0.get(name).is_none_or(|(_, tag)| tag != stamp) {
                    return Ok(AsListed::Changed);
                }
                held.0.remove(name);
                Ok(AsListed::Held(()))
            },
        )
    }

    /// `name` written again in `store`, holding `bytes` under `stamp`.
    fn written(store: &Store, name: &str, bytes: &[u8], stamp: &str) {
        let mut held = store.lock().expect("lock");
        held.0.insert(name.into(), (bytes.to_vec(), stamp.into()));
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
        let store = store(&[("a.edi", b"one"), ("b.edi", b""), ("c.edi", b"three")]);
        let refused = Refused::default();
        let mut arrived = receive(&store, &refused).expect("listed").into_iter();
        assert!(store.lock().expect("lock").1.is_empty(), "nothing got yet");
        let one = arrived.next().expect("a.edi");
        assert!(one.defers());
        assert_eq!(one.origin_uri, "aws-s3://inbox/a.edi");
        assert_eq!(one.taken().expect("taken").bytes, b"one");
        arrived.next().expect("b.edi").failed().expect("left");
        arrived
            .next()
            .expect("c.edi")
            .refused(Refusal::Unacceptable)
            .expect("left");
        let asked = store.lock().expect("lock").1.clone();
        assert_eq!(
            asked,
            ["get a.edi", "delete a.edi"],
            "the refused one is not deleted"
        );
        let again = receive(&store, &refused).expect("listed again");
        assert_eq!(
            names(&again),
            ["aws-s3://inbox/b.edi"],
            "the failed one, not the refused one"
        );
        assert!(
            store.lock().expect("lock").0.contains_key("c.edi"),
            "still there"
        );
    }

    #[test]
    fn a_listing_hands_over_no_more_than_asked_and_leaves_the_rest_in_the_store() {
        let store = store(&[("a.edi", b"1"), ("b.edi", b"2"), ("c.edi", b"3")]);
        let refused = Refused::default();
        for arrived in receive_up_to(&store, 2, &refused).expect("listed") {
            arrived.taken().expect("accepted");
        }
        assert_eq!(
            store.lock().expect("lock").0.len(),
            1,
            "one left in the store"
        );
        let rest = receive_up_to(&store, 2, &refused).expect("listed again");
        assert_eq!(names(&rest), ["aws-s3://inbox/c.edi"]);
    }

    #[test]
    fn a_refused_object_written_again_is_a_new_arrival() {
        let store = store(&[("a.edi", b"one")]);
        let refused = Refused::default();
        let arrived = receive(&store, &refused).expect("listed").remove(0);
        arrived.refused(Refusal::Forbidden).expect("left");
        assert!(receive(&store, &refused).expect("listed").is_empty());
        written(&store, "a.edi", b"two", "2");
        let again = receive(&store, &refused).expect("listed");
        assert_eq!(names(&again), ["aws-s3://inbox/a.edi"], "another ETag");
    }

    #[test]
    fn a_replacement_between_read_and_acceptance_stays_for_the_next_listing() {
        let store = store(&[("a.edi", b"one")]);
        let refused = Refused::default();
        let (bytes, told) = read(receive(&store, &refused).expect("listed").remove(0));
        assert_eq!(bytes, b"one");
        written(&store, "a.edi", b"two", "2");
        told.acknowledge(Verdict::Accepted)
            .expect("the one read is done with");
        let again = receive(&store, &refused).expect("listed again").remove(0);
        assert_eq!(again.taken().expect("a new arrival").bytes, b"two");
    }

    #[test]
    fn a_replacement_between_listing_and_first_read_is_not_read_under_the_old_stamp() {
        let store = store(&[("a.edi", b"one")]);
        let refused = Refused::default();
        let arrived = receive(&store, &refused).expect("listed").remove(0);
        written(&store, "a.edi", b"two", "2");
        let error = arrived.taken().expect_err("not the object listed");
        assert!(error.message.contains(CHANGED), "{error}");
        assert!(error.retryable, "the next listing takes it: {error}");
        assert_eq!(
            store.lock().expect("lock").1,
            ["get a.edi"],
            "nothing deleted"
        );
        let again = receive(&store, &refused).expect("listed again").remove(0);
        assert_eq!(again.taken().expect("a new arrival").bytes, b"two");
    }

    #[test]
    fn an_object_gone_before_acceptance_is_told_done() {
        let store = store(&[("a.edi", b"one")]);
        let (_, told) = read(
            receive(&store, &Refused::default())
                .expect("listed")
                .remove(0),
        );
        store.lock().expect("lock").0.clear();
        told.acknowledge(Verdict::Accepted)
            .expect("nothing left to delete is done");
    }

    #[test]
    fn a_listing_names_each_entry_with_its_stamp_and_an_unstamped_one_is_an_error() {
        const LISTING: Listing = Listing {
            entry: "Blob",
            name: "Name",
            stamp: "Etag",
        };
        let xml = "<Blobs><Blob><Name>in/a&amp;b</Name><Properties><Etag>0x1</Etag>\
                   </Properties></Blob><Blob><Name>b.edi</Name><Etag>0x2</Etag></Blob></Blobs>";
        assert_eq!(
            LISTING.objects(xml).expect("read"),
            [
                ("in/a&b".to_string(), "0x1".to_string()),
                ("b.edi".to_string(), "0x2".to_string())
            ]
        );
        assert_eq!(
            LISTING.objects("<Blob><Name>unclosed").expect("read"),
            [] as [(String, String); 0]
        );
        let error = LISTING
            .objects("<Blob><Name>a.edi</Name></Blob>")
            .expect_err("no stamp");
        assert_eq!(error.message, "a listed Blob without its Etag");
    }

    #[test]
    fn a_failed_list_is_the_receives_failure_and_a_failed_get_its_bodys() {
        let error = listed(
            || Err(protocol_error("403 AccessDenied")),
            usize::MAX,
            &Refused::default(),
            str::to_string,
            |_, _| Ok(AsListed::Held(Vec::new())),
            |_, _| Ok(AsListed::Held(())),
        )
        .expect_err("not listed");
        assert_eq!(error.message, "403 AccessDenied");
        let arrived = listed(
            || Ok(vec![("a.edi".into(), "1".into())]),
            usize::MAX,
            &Refused::default(),
            str::to_string,
            |_, _| Err(protocol_error("503 SlowDown")),
            |_, _| panic!("a body never read is never accepted"),
        )
        .expect("listed")
        .remove(0);
        let error = arrived.taken().expect_err("its get failed");
        assert!(error.message.contains("503 SlowDown"), "{error}");
    }
}
