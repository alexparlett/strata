//! Two file formats with no bytes of their own, and the contract every format is judged by.
//!
//! [`TestFormat`] is a format an embedder could have written: it declares a writer, so it both
//! reads and is written by `COPY`, and it keeps its options verbatim the way the seam's own
//! default does. [`TestReader`] is the read-only half — it offers `OPTIONS` keys, refuses one it
//! did not offer, and names its files something other than itself.
//!
//! Both read and write **CSV bytes** under a name of their own. What is being proven is the
//! registry path, not a codec, and borrowing DataFusion's simplest one keeps the fixture to the
//! thing it is a fixture for.

use std::collections::BTreeMap;
use std::sync::Arc;

use datafusion::datasource::file_format::csv::{CsvFormat, CsvFormatFactory};
use datafusion::datasource::file_format::{FileFormat, FileFormatFactory};

use strata_model::SourceFormat;

use super::{FileFormatKind, FormatProvider, OptionKind, OptionOffer, ReadFor};

/// A format that reads and writes, registered the way an embedder's would be.
#[derive(Debug)]
pub(crate) struct TestFormat;

impl FileFormatKind for TestFormat {
    const NAME: &'static str = "testfmt";
}

impl FormatProvider for TestFormat {
    fn build(&self, _format: &SourceFormat) -> Result<Arc<dyn FileFormat>, String> {
        Ok(Arc::new(CsvFormat::default().with_has_header(true)))
    }

    fn copy_to(&self) -> bool {
        true
    }

    fn writer(&self) -> Option<Arc<dyn FileFormatFactory>> {
        Some(Arc::new(CsvFormatFactory::new()))
    }
}

/// A read-only format with options of its own, whose files are not named after it.
#[derive(Debug)]
pub(crate) struct TestReader;

impl FileFormatKind for TestReader {
    const NAME: &'static str = "testread";
}

impl TestReader {
    /// The one key this format takes, named here so the offer and the refusal agree.
    pub(crate) const HEADER: &'static str = "format.header";
}

impl FormatProvider for TestReader {
    fn build(&self, format: &SourceFormat) -> Result<Arc<dyn FileFormat>, String> {
        let header = match format {
            SourceFormat::Extension { options, .. } => {
                options.get(Self::HEADER).map(String::as_str) != Some("false")
            }
            other => return Err(format!("the '{}' reader reads no def", other.name())),
        };
        Ok(Arc::new(CsvFormat::default().with_has_header(header)))
    }

    fn read(
        &self,
        at: ReadFor<'_>,
        options: &BTreeMap<String, String>,
    ) -> Result<SourceFormat, String> {
        for key in options.keys() {
            if key != Self::HEADER {
                return Err(format!(
                    "'{key}' is not a read option for a {} table",
                    at.format
                ));
            }
        }
        Ok(SourceFormat::of(at.format, options.clone()))
    }

    fn extension(&self, _format: &SourceFormat) -> String {
        ".tr".to_string()
    }

    fn reader_options(&self) -> Vec<OptionOffer> {
        vec![OptionOffer {
            key: Self::HEADER,
            kind: OptionKind::Bool,
            what: "header row",
        }]
    }
}

#[cfg(test)]
mod tests {
    use datafusion::prelude::SessionContext;

    use super::super::shipped;
    use super::*;

    /// **The contract every format keeps**, run against each of them: a def its own `read`
    /// produced is one its own `build` accepts, its files have an extension, an offer it makes is
    /// an offer it honours, and a writer it declares writes.
    fn conforms<F: FormatProvider + FileFormatKind>(format: F) {
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

    #[test]
    fn every_shipped_format_keeps_the_contract() {
        conforms(shipped::Parquet);
        conforms(shipped::Csv);
        conforms(shipped::Json);
        conforms(shipped::Arrow);
    }

    #[test]
    fn every_test_format_keeps_the_contract() {
        conforms(TestFormat);
        conforms(TestReader);
    }
}
