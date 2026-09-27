//! Who a transport logs in as: a user and a password.
//!
//! Eight technologies and MQTT each declared this pair as a type of their
//! own until 2026-09-27. What a protocol adds to it — AMQP's virtual host,
//! MQTT's client identifier — stays the protocol's, beside it.

use std::fmt;

/// A user and the password it logs in with. The password is never
/// printed: `{:?}` shows the user alone.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Login {
    pub user: String,
    /// Empty where the account has none.
    pub password: String,
}

impl Login {
    /// `user` with `password`.
    #[must_use]
    pub fn new(user: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            user: user.into(),
            password: password.into(),
        }
    }
}

impl fmt::Debug for Login {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Login")
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_login_is_printed_without_its_password() {
        let login = Login::new("orders", "s3cret");
        let printed = format!("{login:?}");
        assert!(
            printed.contains("orders") && !printed.contains("s3cret"),
            "{printed}"
        );
    }
}
