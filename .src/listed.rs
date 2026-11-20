//! A receive from a store of named objects: what is listed and claimed by
//! one node, each fetched as it was listed when the runtime first reads
//! it, deleted as it was listed once the cycle accepted it, left and
//! remembered once the cycle refused it.
//!
//! An object store — an S3 bucket, an Azure Blob container, a Google
//! Cloud Storage bucket — holds what was dropped under a name until it is
//! deleted. A receive lists, and gets and deletes nothing: each listed
//! object it wins is an [`Arrived`](crate::Arrived) whose body `GET`s it on
//! its first read, so a receive that lists a hundred objects holds none of
//! them in memory and one never read is never fetched. Its acknowledgement
//! deletes it on `Verdict::Accepted` only.
//!
//! **One node wins an object** (ADR-0024, amendments 2026-10-13 and
//! 2026-11-20): its claim, `<name>.xmip-claim` beside it, is written only
//! where none lies — the store's own exclusive step, `If-None-Match: *` on
//! S3 and Azure Blob, `ifGenerationMatch=0` on Google Cloud Storage — and
//! holds the claimed name the file family's claim records, the node's
//! location and the time ([`crate::claim::Claimant::claimed`]). A node
//! that finds another's claim, or loses the write, leaves the object to
//! whoever holds it; the claim is let go however the cycle ends, and a
//! node that starts again lets go of every claim its name still holds
//! ([`Claims`]).
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
//! has no place for a refused one. On `Verdict::Refused` it is left where
//! it lies and remembered with the stamp its listing gave it, and no
//! receive lists it again while it lies so (`crate::Refused`); one written
//! again under its name is a new arrival. On `Verdict::Failed` it is left,
//! and the next receive lists and gets it again: at least once, never a
//! loss. What a technology lists, gets, deletes and creates with is its
//! own ([`Store`]):
//!
//! ```text
//!   claims.rs   one Location's claims: the listing won, the recovery of a
//!               starting node, the claims not yet let go, one by address
//!   hold.rs     a claimed object's acknowledgement, its claim let go
//!               however the cycle ends
//! ```

mod claims;
mod hold;
#[cfg(test)]
mod memory;

use crate::error::{Result, protocol_error};

pub use claims::Claims;

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

/// An object store as a receive reaches it, in its own words: what each
/// object-store technology implements, and all it implements, for the
/// receive and the claim written once here ([`Claims`]). Open: any store
/// whose writes can be made only where nothing lies implements it.
pub trait Store: Send + Sync {
    /// Every object under the Location's prefix, each with its stamp,
    /// claims included.
    ///
    /// # Errors
    /// Where the store refused or could not be reached.
    fn list(&self) -> Result<Vec<Object>>;

    /// The object `name` while it bears `stamp`: [`AsListed::Changed`]
    /// where it was written again or deleted since.
    ///
    /// # Errors
    /// Where the store refused for another reason or could not be reached.
    fn get(&self, name: &str, stamp: &str) -> Result<AsListed<Vec<u8>>>;

    /// Delete the object `name` while it bears `stamp`:
    /// [`AsListed::Changed`], and nothing deleted, where it was written
    /// again or deleted since.
    ///
    /// # Errors
    /// Where the store refused for another reason or could not be reached.
    fn delete(&self, name: &str, stamp: &str) -> Result<AsListed<()>>;

    /// Write `bytes` as the object `name` only where no object lies under
    /// it, in one request the store decides alone: the stamp it was written
    /// with, or `None` where one lay there and nothing was written.
    ///
    /// # Errors
    /// Where the store refused for another reason or could not be reached.
    fn create(&self, name: &str, bytes: &[u8]) -> Result<Option<String>>;

    /// The object `name` as an origin says it: `aws-s3://bucket/in/a.edi`.
    fn origin(&self, name: &str) -> String;
}

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
