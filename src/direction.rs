//! Which directions an implementation supports.

/// Mirrors `XMIP_DIR_RECEIVE` and `XMIP_DIR_SEND` in `include/xmip_module.h`.
///
/// A bitfield rather than an enum because a protocol can do both and most do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Directions(u32);

impl Directions {
    pub const RECEIVE: Self = Self(1);
    pub const SEND: Self = Self(2);
    pub const BOTH: Self = Self(3);

    /// True when this implementation can receive.
    #[must_use]
    pub const fn receives(self) -> bool {
        self.0 & 1 != 0
    }

    /// True when this implementation can send.
    #[must_use]
    pub const fn sends(self) -> bool {
        self.0 & 2 != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_is_each_direction_at_once() {
        assert!(Directions::BOTH.receives());
        assert!(Directions::BOTH.sends());
    }

    #[test]
    fn one_direction_is_not_the_other() {
        assert!(Directions::RECEIVE.receives());
        assert!(!Directions::RECEIVE.sends());

        assert!(Directions::SEND.sends());
        assert!(!Directions::SEND.receives());
    }
}
