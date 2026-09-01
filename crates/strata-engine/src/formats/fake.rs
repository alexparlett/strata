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
pub struct TestFormat;

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
pub struct TestReader;

impl FileFormatKind for TestReader {
    const NAME: &'static str = "testread";
}

impl TestReader {
    /// The one key this format takes, named here so the offer and the refusal agree.
    pub const HEADER: &'static str = "format.header";
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
    use super::super::shipped;
    use super::*;
    use crate::formats::conformance::conforms;

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
