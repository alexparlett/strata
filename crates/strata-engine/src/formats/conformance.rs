//! The contract every [`FormatProvider`] keeps, as a body any registrant can be run through.
//!
//! One body, six callers: the four shipped formats, the two fixtures beside them, and whatever
//! an embedder registered. What it pins is the round trip — a def a format's own `read` produced
//! is one its own `build` accepts — plus the three things every surface that offers a format
//! relies on: an extension, an offer it honours, and a writer that writes if it declared one.
//!
//! Available to embedders under the `testing` cargo feature.

use std::collections::BTreeMap;

use datafusion::prelude::SessionContext;

use super::{FileFormatKind, FormatProvider, OptionKind, ReadFor};

/// Runs `format` through the contract, panicking on the first clause it does not keep: a def its
/// own `read` produced is one its own `build` accepts, its files have an extension, an offer it
/// makes is an offer it honours, and a writer it declares writes.
///
/// # Examples
///
/// ```
/// strata_engine::testing::formats::conforms(strata_engine::formats::fake::TestFormat);
/// ```
///
/// # Panics
///
/// On any clause the format does not keep.
pub fn conforms<F: FormatProvider + FileFormatKind>(format: F) {
    let name = F::NAME;
    assert_eq!(
        name.to_ascii_lowercase(),
        name,
        "'{name}' is registered under a name a STORED AS word cannot be written as"
    );
    let at = ReadFor {
        format: name,
        table: "fixture",
    };
    let def = format
        .read(at, &BTreeMap::new())
        .unwrap_or_else(|why| panic!("'{name}' cannot be named with no options: {why}"));
    assert_eq!(
        def.name(),
        name,
        "'{name}' reads into a def filed under another format's name, so the registry would \
         dispatch its build elsewhere"
    );
    format
        .build(&def)
        .unwrap_or_else(|why| panic!("'{name}' refused the def its own read produced: {why}"));

    let ext = format.extension(&def);
    assert!(
        ext.starts_with('.') && ext.len() > 1,
        "'{name}' answers with '{ext}', which is not a file extension"
    );

    let offers = format.reader_options();
    for offer in &offers {
        assert!(
            offer.key.starts_with("format."),
            "'{name}' offers '{}', which is not a read option's spelling",
            offer.key
        );
        let value = match offer.kind {
            OptionKind::Bool => "true",
            OptionKind::Char => "x",
            OptionKind::Int => "10",
            OptionKind::Enum(words) => words[0],
        };
        let one = BTreeMap::from([(offer.key.to_string(), value.to_string())]);
        format.read(at, &one).unwrap_or_else(|why| {
            panic!("'{name}' offers '{}' and then refuses it: {why}", offer.key)
        });
    }
    if !offers.is_empty() {
        let unoffered = BTreeMap::from([("format.not_offered".to_string(), "1".to_string())]);
        assert!(
            format.read(at, &unoffered).is_err(),
            "'{name}' offers keys but takes one it never offered"
        );
    }

    match format.writer() {
        None => {}
        Some(writer) => {
            assert!(
                format.copy_to(),
                "'{name}' brought a writer without declaring that it can be written"
            );
            let state = SessionContext::new().state();
            writer
                .create(&state, &Default::default())
                .unwrap_or_else(|why| {
                    panic!("'{name}' declared a writer that will not write: {why}")
                });
        }
    }
}
