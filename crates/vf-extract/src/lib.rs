//! Local-pattern extraction (PHASES P08): recognizes W-2, 1099, 1098, K-1 and SSA-1099 forms
//! from a page's text, and proposes the client (and, for a K-1, both parties) they belong to.
//! Pure: text and client lists in, findings out. TINs are last 4 digits only, by type.

pub mod forms;
pub mod matching;
#[cfg(any(test, feature = "test-support"))]
pub mod synthetic;

pub use forms::{Extract, Form, Last4, recognize, recognize_filled};
