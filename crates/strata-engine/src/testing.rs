//! Test kit for embedders: the conformance ring a [`DataSource`](crate::sources::source::DataSource)
//! is judged by, and two sources with no server behind them.
//!
//! Behind the `testing` cargo feature, which is not on by default: a dev-dependency on
//! `strata-engine` with `features = ["testing"]` is what turns it on.
//!
//! ```toml
//! [dev-dependencies]
//! strata-engine = { version = "0.2", features = ["testing"] }
//! ```
//!
//! [`conforms`] is the whole point — run your own registrant through the same body the shipped
//! sources are run through. [`TestDoc`] and [`TestSql`] are here for the surfaces *above* the
//! seam: a test about what a catalog pane, a forget confirm or a completion offer does with a
//! connected data source needs one registered, and a real server is not a reasonable ask of a UI
//! test. They are also the shortest complete implementations of the trait to read.
//!
//! See [`guide::data_source`](crate::guide::data_source) for what each ring covers.

pub use crate::sources::conformance::{conforms, conforms_with, declares_a_drawable_form};
pub use crate::sources::fake::{fake_def, fake_schema, Rows, TestDoc, TestSql};
