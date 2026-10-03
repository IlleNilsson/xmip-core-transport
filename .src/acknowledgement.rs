//! What the far end is told once a receive cycle has ended.
//!
//! The owner, 2026-10-01: *On arrival each Stream is written to the node's
//! Ledger and then when the Receive cycle is complete, Promote, Validate and
//! what not then the sender is acknowledged* (runtime-model section 5, *How
//! a receive runs*). A transport therefore consumes nothing as it receives:
//! the file stays where it was dropped, the queue message stays leased, the
//! HTTP caller waits for its answer. It hands the runtime an
//! [`Acknowledgement`] beside the Stream, and the runtime gives it one
//! [`Verdict`] after the whole cycle — the only place a transport consumes.
//!
//! **Nothing acknowledges by default.** An acknowledgement is built either
//! [`Acknowledgement::deferred`], with what the protocol does for each
//! verdict, or [`Acknowledgement::at_most_once`], where the protocol took
//! the Stream from its sender as it arrived and nothing is left to defer —
//! a datagram, a fire-and-forget push — and the reason is said in words, in
//! the code and in that transport's README. One dropped without a verdict
//! does nothing: what was not consumed stays with the far end, as a failed
//! cycle leaves it, and a caller still waiting is let go unanswered.
//!
//! **Refused is not Failed.** A refused Stream — an unknown sender, one not
//! permitted, content that fails Validation — is told the protocol's
//! permanent rejection and is not sent again; a cycle Xmip could not
//! complete is told the protocol's transient answer, and the far end sends
//! it again.

use std::fmt;

use crate::error::Result;

/// How a receive cycle ended, as the far end is told it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Verdict {
    /// The cycle completed: the Stream is durably Xmip's. The far end
    /// consumes — the file moved or deleted, the queue message deleted or
    /// committed, the protocol's positive answer (`202`, an MDN, a
    /// `PUBACK`) sent.
    Accepted,
    /// The cycle refused the Stream, and saying it again will not change
    /// that: the far end hears the protocol's permanent rejection — a
    /// `4xx`, an error MDN, an HL7 `AE`, a reject without requeue — and
    /// does not send it again. [`Refusal`] says why, for a protocol whose
    /// answer differs by cause.
    ///
    /// **A refusal is not a consumption.** A Stream refused at a transport
    /// gate was never written to the Ledger (runtime-model section 5), so
    /// where Xmip polled it from a far end that holds the only copy — a
    /// file in a drop directory — the refusal sets it aside and never
    /// destroys it: moved to the technology's refused place where it has
    /// one, otherwise left where it lies and not received again while it
    /// stays as it was refused. Only [`Verdict::Accepted`] consumes there.
    Refused(Refusal),
    /// Xmip could not complete the cycle — Xmip Storage did not take the
    /// Stream, its body broke — so the far end keeps it and sends it again:
    /// the file left, the message abandoned, nacked or left to its
    /// visibility timeout, the protocol's transient answer (`503`, an HL7
    /// `AR`, a busy code).
    Failed,
}

/// Why the cycle refused a Stream: what a protocol whose permanent
/// rejection differs by cause — HTTP's `401`, `403` and `422` — answers
/// from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Refusal {
    /// The sender could not be identified or authenticated.
    Unidentified,
    /// The sender is known and not permitted to post here.
    Forbidden,
    /// The content was refused: a property its routing reads could not be
    /// read, or it failed Validation.
    Unacceptable,
}

/// What a protocol does with each [`Verdict`]: the far end consumes on
/// `Accepted`, rejects for good on `Refused` and keeps to send again on
/// `Failed`, or the protocol could not defer at all.
pub struct Acknowledgement(Answer);

/// A deferred answer, or the reason there is none.
enum Answer {
    Deferred(Box<dyn FnOnce(Verdict) -> Result<()> + Send>),
    AtMostOnce(&'static str),
}

impl Acknowledgement {
    /// The far end is told after the cycle: `answer` consumes on
    /// [`Verdict::Accepted`], answers the protocol's permanent rejection on
    /// [`Verdict::Refused`] and leaves or transiently answers on
    /// [`Verdict::Failed`], as the protocol allows. It runs once, on the
    /// thread that carried the Stream.
    #[must_use]
    pub fn deferred(answer: impl FnOnce(Verdict) -> Result<()> + Send + 'static) -> Self {
        Self(Answer::Deferred(Box::new(answer)))
    }

    /// The Stream was read without consuming anything at the far end — a
    /// register polled, a variable read, a `SELECT` that deletes nothing —
    /// so neither verdict has anything to tell it. This is not
    /// at-most-once: a cycle that did not complete loses nothing, the next
    /// read finds it again.
    #[must_use]
    pub fn unconsumed() -> Self {
        Self::deferred(|_| Ok(()))
    }

    /// The protocol took the Stream from its sender as it arrived — a
    /// datagram has nobody to answer, a push has already been answered by
    /// the protocol itself — so a crash before the Stream is durable loses
    /// it: acceptance is at-most-once there. `because` says why, in words
    /// an operator reads; the transport's README says it too.
    #[must_use]
    pub const fn at_most_once(because: &'static str) -> Self {
        Self(Answer::AtMostOnce(because))
    }

    /// Whether the far end waits for the verdict. `false` is an
    /// at-most-once protocol.
    #[must_use]
    pub const fn defers(&self) -> bool {
        matches!(self.0, Answer::Deferred(_))
    }

    /// Why this protocol cannot defer, where it cannot.
    #[must_use]
    pub const fn at_most_once_because(&self) -> Option<&'static str> {
        match self.0 {
            Answer::Deferred(_) => None,
            Answer::AtMostOnce(because) => Some(because),
        }
    }

    /// Tell the far end how the cycle ended. Nothing happens for an
    /// at-most-once protocol.
    ///
    /// # Errors
    /// Where the far end could not be told: the connection broke, the
    /// delete or the commit failed. On `Accepted` the Stream is already
    /// Xmip's, so a failure here means the far end may deliver it again —
    /// at-least-once, never a loss.
    pub fn acknowledge(self, verdict: Verdict) -> Result<()> {
        match self.0 {
            Answer::Deferred(answer) => answer(verdict),
            Answer::AtMostOnce(_) => Ok(()),
        }
    }
}

impl fmt::Debug for Acknowledgement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Answer::Deferred(_) => f.write_str("Acknowledgement::Deferred"),
            Answer::AtMostOnce(because) => write!(f, "Acknowledgement::AtMostOnce({because:?})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn a_deferred_acknowledgement_hands_the_far_end_its_verdict() {
        let told = Arc::new(Mutex::new(Vec::new()));
        for verdict in [
            Verdict::Accepted,
            Verdict::Refused(Refusal::Forbidden),
            Verdict::Failed,
        ] {
            let into = Arc::clone(&told);
            let acknowledgement = Acknowledgement::deferred(move |verdict| {
                into.lock().expect("lock").push(verdict);
                Ok(())
            });
            assert!(acknowledgement.defers());
            assert_eq!(acknowledgement.at_most_once_because(), None);
            acknowledgement.acknowledge(verdict).expect("told");
        }
        assert_eq!(
            *told.lock().expect("lock"),
            [
                Verdict::Accepted,
                Verdict::Refused(Refusal::Forbidden),
                Verdict::Failed
            ]
        );
    }

    #[test]
    fn an_at_most_once_acknowledgement_says_why_and_does_nothing() {
        let acknowledgement = Acknowledgement::at_most_once("a datagram has nobody to answer");
        assert!(!acknowledgement.defers());
        assert_eq!(
            acknowledgement.at_most_once_because(),
            Some("a datagram has nobody to answer")
        );
        assert_eq!(
            format!("{acknowledgement:?}"),
            r#"Acknowledgement::AtMostOnce("a datagram has nobody to answer")"#
        );
        acknowledgement
            .acknowledge(Verdict::Failed)
            .expect("nothing to tell");
    }

    #[test]
    fn an_unconsumed_read_defers_and_tells_nothing() {
        let acknowledgement = Acknowledgement::unconsumed();
        assert!(acknowledgement.defers());
        assert_eq!(acknowledgement.at_most_once_because(), None);
        acknowledgement
            .acknowledge(Verdict::Failed)
            .expect("nothing to tell");
    }

    #[test]
    fn a_failed_answer_is_said() {
        let acknowledgement = Acknowledgement::deferred(|_| {
            Err(crate::error::protocol_error("the delete was refused"))
        });
        let error = acknowledgement
            .acknowledge(Verdict::Accepted)
            .expect_err("failed");
        assert_eq!(error.message, "the delete was refused");
    }
}
