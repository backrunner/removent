use core::fmt;

/// Generic Core Foundation / Core Graphics creation error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NullPointerError {
    operation: &'static str,
}

impl NullPointerError {
    /// Create a new error for the named Core Foundation / Core Graphics call.
    #[must_use]
    pub const fn new(operation: &'static str) -> Self {
        Self { operation }
    }

    /// The API call that failed.
    #[must_use]
    pub const fn operation(&self) -> &'static str {
        self.operation
    }
}

impl fmt::Display for NullPointerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} returned a null Core Foundation/Core Graphics pointer",
            self.operation
        )
    }
}

impl std::error::Error for NullPointerError {}
