//! Test kit for embedders: the six conformance rings a seam's implementation is judged by, and
//! the fixtures that stand in for the things a test cannot have.
//!
//! Behind the `testing` cargo feature, which is not on by default: a dev-dependency on
//! `strata-engine` with `features = ["testing"]` is what turns it on.
//!
//! ```toml
//! [dev-dependencies]
//! strata-engine = { version = "0.2", features = ["testing"] }
//! ```
//!
//! # The rings
//!
//! Every seam this crate offers ships the body its own implementations are run through, so what
//! you wrote is held to the same law they are. Each is one call:
//!
//! | Seam | Ring |
//! |---|---|
//! | [`DataSource`](crate::sources::source::DataSource) | [`sources::conforms`] |
//! | [`SnapshotStore`](crate::snapshots::SnapshotStore) | [`snapshots::conforms`] |
//! | [`InternalTableStore`](crate::tables::InternalTableStore) | [`tables::conforms`] |
//! | [`PolicyProvider`](crate::policy::PolicyProvider) | [`policy::conforms`] |
//! | [`FormatProvider`](crate::formats::FormatProvider) | [`formats::conforms`] |
//! | [`SecretProvider`](crate::secrets::SecretProvider) | [`secrets::conforms`] |
//!
//! The **SQL** half of the data-source contract is not among them: pushdown proven, JSON
//! exercised and one `compute_context` per connection are only meaningful against a live server,
//! so they live in the container suites as a shape to reproduce rather than a body to call. See
//! [`guide::data_source`](crate::guide::data_source).
//!
//! # The fixtures
//!
//! [`TestDoc`] and [`TestSql`] are here for the surfaces *above* the seam: a test about what a
//! catalog pane, a forget confirm or a completion offer does with a connected data source needs
//! one registered, and a real server is not a reasonable ask of a UI test. [`TestFormat`] and
//! [`TestReader`] are the same thing one seam over. They are also the shortest complete
//! implementations of their traits to read.

pub use crate::formats::conformance as formats;
pub use crate::policy::conformance as policy;
pub use crate::secrets::conformance as secrets;
pub use crate::snapshots::conformance as snapshots;
pub use crate::sources::conformance as sources;
pub use crate::tables::conformance as tables;

pub use crate::formats::fake::{TestFormat, TestReader};
pub use crate::sources::conformance::{conforms, conforms_with, declares_a_drawable_form};
pub use crate::sources::fake::{fake_def, fake_schema, Rows, TestDoc, TestSql};
