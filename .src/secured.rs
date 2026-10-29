//! The secret a Location names, read when its technology connects
//! (ADR-0019, amendment 2026-10-29): `secret = "<name>"` on a Location
//! that authenticates to the party it connects to, the value kept in the
//! Xmip Administration Database and never in configuration.
//!
//! **Read at each connection, not at the node's start.** A secret set or
//! replaced reaches the next connection a Location makes, so one is
//! changed without a restart, and a node whose Location names a secret not
//! set yet still starts — so the secret can be set through it — and that
//! Location fails each attempt in words until it is. A read is one point
//! read of Xmip Storage, made only where a technology opens a connection,
//! never per Message on a kept one.

use std::fmt;
use std::sync::Arc;

use xcore::{Secret, SecretKind, SecretName, Secrets};

use crate::error::{Result, TransportError};
use crate::login::Login;

/// A Location's secret, by name, and where the node reads it.
#[derive(Clone)]
pub struct Secured {
    name: SecretName,
    secrets: Arc<dyn Secrets>,
}

impl Secured {
    /// The secret `name`, read from `secrets` as it is asked for.
    #[must_use]
    pub fn new(name: SecretName, secrets: Arc<dyn Secrets>) -> Self {
        Self { name, secrets }
    }

    /// Its name.
    #[must_use]
    pub const fn name(&self) -> &SecretName {
        &self.name
    }

    /// The secret as it is set now.
    ///
    /// # Errors
    ///
    /// Permanent where it is not set, saying how to set it; retryable where
    /// Xmip Storage did not answer.
    pub fn secret(&self) -> Result<Secret> {
        let read = self
            .secrets
            .secret(&self.name)
            .map_err(|failure| TransportError {
                message: format!(
                    "the secret '{}' could not be read: {}",
                    self.name, failure.message
                ),
                retryable: failure.retryable,
            })?;
        read.ok_or_else(|| {
            TransportError::permanent(format!(
                "the secret '{}' is not set: set it with Set-XmipSecret, or xmip secret --set",
                self.name
            ))
        })
    }

    /// The secret's value as `kind`, as `presenting` presents it.
    ///
    /// # Errors
    ///
    /// What [`Secured::secret`] refuses, and permanent where the secret is
    /// of another kind.
    pub fn as_kind(&self, kind: SecretKind, presenting: &str) -> Result<String> {
        let secret = self.secret()?;
        secret
            .as_kind(kind, presenting)
            .map(str::to_string)
            .map_err(|failure| TransportError::permanent(failure.message))
    }
}

impl fmt::Debug for Secured {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Secured")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// `login` with the password of `secured`, where the Location names a
/// secret, read now; `login` as it is where it names none: the one way a
/// technology that logs in with a user and a password presents its secret.
///
/// # Errors
///
/// What [`Secured::as_kind`] refuses for a password.
pub fn logged_in(login: &Login, secured: Option<&Secured>, presenting: &str) -> Result<Login> {
    match secured {
        Some(secured) => Ok(Login::new(
            login.user.clone(),
            secured.as_kind(SecretKind::Password, presenting)?,
        )),
        None => Ok(login.clone()),
    }
}

/// Secrets held in memory, for the tests of what presents one: what a test
/// sets is what is read. Never in a production build (ADR-0044).
#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
pub struct Kept(std::sync::Mutex<std::collections::BTreeMap<String, Secret>>);

#[cfg(any(test, feature = "test-support"))]
impl Kept {
    /// `value` set as the secret `name` of `kind`.
    ///
    /// # Panics
    ///
    /// Where `name` or `value` is not one: a test's own mistake.
    pub fn set(&self, name: &str, kind: SecretKind, value: &str) {
        let name = SecretName::new(name).expect("a secret name");
        let secret = Secret::new(name.clone(), kind, value.as_bytes().to_vec()).expect("a secret");
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(name.to_string(), secret);
    }

    /// A Location's secret `name`, read from here.
    ///
    /// # Panics
    ///
    /// Where `name` is no secret name.
    pub fn secured(self: &Arc<Self>, name: &str) -> Secured {
        let secrets: Arc<dyn Secrets> = Arc::clone(self) as Arc<dyn Secrets>;
        Secured::new(SecretName::new(name).expect("a secret name"), secrets)
    }

    /// A value drawn for a test, never one written down.
    #[must_use]
    pub fn drawn() -> String {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
        format!(
            "drawn-{:x}",
            now.map(|since| since.as_nanos()).unwrap_or_default()
        )
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Secrets for Kept {
    fn secret(&self, name: &SecretName) -> std::result::Result<Option<Secret>, xcore::Failure> {
        let kept = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(kept.get(name.as_str()).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_not_set_is_said_and_one_set_is_read_at_the_next_ask() {
        let kept = Arc::new(Kept::default());
        let secured = kept.secured("partner-x");
        let login = Login::new("orders", "");
        let refused = logged_in(&login, Some(&secured), "ftp").expect_err("not set");
        assert!(!refused.retryable);
        assert!(
            refused.message.contains("'partner-x' is not set"),
            "{refused}"
        );

        let value = Kept::drawn();
        kept.set("partner-x", SecretKind::Password, &value);
        let presented = logged_in(&login, Some(&secured), "ftp").expect("set now");
        assert_eq!(
            (presented.user.as_str(), presented.password.as_str()),
            ("orders", &*value)
        );
        assert_eq!(logged_in(&login, None, "ftp").expect("none named"), login);

        let wrong = secured
            .as_kind(SecretKind::Key, "sftp")
            .expect_err("a password");
        assert!(
            wrong
                .message
                .contains("is a password, and sftp presents a key"),
            "{wrong}"
        );
    }
}
