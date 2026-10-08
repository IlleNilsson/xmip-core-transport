//! A receive from a store of named objects: what is listed, each fetched
//! when the runtime first reads it, deleted once the cycle accepted it,
//! left and remembered once the cycle refused it.
//!
//! An object store — an S3 bucket, an Azure Blob container, a Google
//! Cloud Storage bucket — holds what was dropped under a name until it is
//! deleted, and has no lock, lease or rejected place a receive takes. So a
//! receive lists, and gets and deletes nothing: each listed object is an
//! [`Arrived`] whose body `GET`s it on its first read ([`fetched`]), so a
//! receive that lists a hundred objects holds none of them in memory and
//! one never read is never fetched. Its acknowledgement deletes it on
//! [`Verdict::Accepted`] only. A refusal is not a consumption: a Stream
//! refused at a transport gate was never written to the Ledger, so the
//! object is the only copy, and a store has no place for a refused one. On
//! [`Verdict::Refused`] it is left where it lies and remembered with the
//! stamp its listing gave it — its `ETag` — and no receive lists it again
//! while it lies so ([`Refused`]); one written again under its name is a
//! new arrival. On [`Verdict::Failed`] it is left, and the next receive
//! lists and gets it again: at least once, never a loss. What a technology
//! lists, gets and deletes with is its own, handed in.

use std::sync::Arc;

use crate::acknowledgement::{Acknowledgement, Verdict};
use crate::arrived::Arrived;
use crate::body::fetched;
use crate::error::{Result, protocol_error};
use crate::refused::Refused;

/// An object as a listing names it: its name and its stamp, the `ETag`
/// that changes whenever it is written again.
pub type Object = (String, String);

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

/// Every object `list` lists and `refused` does not hold as it lies, as an
/// arrival from `origin` of its name whose body `get` fetches on its first
/// read and whose acknowledgement `delete`s it on acceptance, leaves it and
/// remembers it in `refused` on refusal, and leaves it on failure.
///
/// # Errors
/// As `list`: nothing is fetched or deleted here.
pub fn listed<G, D>(
    list: impl FnOnce() -> Result<Vec<Object>>,
    refused: &Refused<String, String>,
    origin: impl Fn(&str) -> String,
    get: G,
    delete: D,
) -> Result<Vec<Arrived>>
where
    G: Fn(&str) -> Result<Vec<u8>> + Send + Sync + 'static,
    D: Fn(&str) -> Result<()> + Send + Sync + 'static,
{
    let (get, delete) = (Arc::new(get), Arc::new(delete));
    Ok(refused
        .sift(list()?, |(name, _)| name, |(_, stamp)| Some(stamp.clone()))
        .into_iter()
        .map(|(name, stamp)| {
            let from = origin(&name);
            let (fetching, named) = (Arc::clone(&get), name.clone());
            let body = fetched(move || fetching(&named));
            let (delete, deleting) = (Arc::clone(&delete), name.clone());
            let told = Acknowledgement::deferred(move |verdict| match verdict {
                Verdict::Accepted => delete(&deleting),
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
    use std::sync::Mutex;

    /// The objects held, by name, each with its body and its `ETag`, and
    /// what was asked of the store.
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
            refused,
            |name| format!("s3://inbox/{name}"),
            move |name| {
                let mut held = getting.lock().expect("lock");
                held.1.push(format!("get {name}"));
                held.0
                    .get(name)
                    .map(|(bytes, _)| bytes.clone())
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
        assert_eq!(one.origin_uri, "s3://inbox/a.edi");
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
            ["s3://inbox/b.edi"],
            "the failed one, not the refused one"
        );
        assert!(
            store.lock().expect("lock").0.contains_key("c.edi"),
            "still there"
        );
    }

    #[test]
    fn a_refused_object_written_again_is_a_new_arrival() {
        let store = store(&[("a.edi", b"one")]);
        let refused = Refused::default();
        let arrived = receive(&store, &refused).expect("listed").remove(0);
        arrived.refused(Refusal::Forbidden).expect("left");
        assert!(receive(&store, &refused).expect("listed").is_empty());
        store
            .lock()
            .expect("lock")
            .0
            .insert("a.edi".into(), (b"two".to_vec(), "2".into()));
        let again = receive(&store, &refused).expect("listed");
        assert_eq!(names(&again), ["s3://inbox/a.edi"], "another ETag");
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
        assert!(
            LISTING
                .objects("<Blob><Name>unclosed")
                .expect("read")
                .is_empty()
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
            &Refused::default(),
            str::to_string,
            |_| Ok(Vec::new()),
            |_| Ok(()),
        )
        .expect_err("not listed");
        assert_eq!(error.message, "403 AccessDenied");
        let store = store(&[("a.edi", b"one")]);
        let arrived = receive(&store, &Refused::default())
            .expect("listed")
            .remove(0);
        store.lock().expect("lock").0.clear();
        let error = arrived.taken().expect_err("gone before it was read");
        assert!(error.message.contains("no such object"), "{error}");
    }
}
