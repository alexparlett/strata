//! **File formats**: the readers a table def may name, and the registry the engine looks one up
//! in.
//!
//! [`FormatProvider`] is the seam: implement it, register it with
//! [`EngineBuilder::with_format`](crate::EngineBuilder::with_format), and a def naming its
//! [`FileFormatKind::NAME`] registers, reads, completes and — where the format declares a writer —
//! is written by `COPY … STORED AS <name>`. The shipped formats are ordinary registrants,
//! pre-registered through the same public call, which is what makes the registry the only path in.
//!
//! **The registry key is the `STORED AS` word is the def's discriminator.** One word answers three
//! questions, so a format cannot be offered under a spelling that then fails to register. The
//! shipped formats keep **typed** defs ([`SourceFormat::Csv`] and friends), because a form that
//! edits one option at a time needs fields rather than a bag of strings; every other format's
//! options ride [`SourceFormat::Extension`] verbatim, in the typed statement's own `format.*`
//! spelling, its own reader being the only thing that knows what they mean.
//!
//! **The write path is DataFusion's own.** A `COPY … STORED AS <word>` is planned by DataFusion,
//! which resolves the word against its *session* factory map — so a registrant that brings a
//! [`writer`](FormatProvider::writer) has it registered there under the same word, and one that
//! does not is written by DataFusion's own. That map is why a name already in this registry cannot
//! be taken again ([`Formats::insert`]): a factory registered over `parquet` / `csv` / `json` /
//! `arrow` would silently replace the writer every other `COPY` in the session uses.

#[cfg(test)]
pub(crate) mod fake;
mod shipped;

pub(crate) use shipped::compression_type;

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::Arc;

use datafusion::catalog::Session;
use datafusion::common::file_options::file_type::GetExt;
use datafusion::datasource::file_format::{FileFormat, FileFormatFactory};
use datafusion::error::Result as DfResult;
use datafusion::prelude::SessionContext;

use strata_model::SourceFormat;

/// Names a format for the registry, and for the surfaces that offer it.
///
/// A companion trait rather than a method on [`FormatProvider`], because an associated const is
/// not dyn-compatible: the name is read once, where the concrete type is still in hand, so a
/// format cannot answer differently from the key it was filed under.
pub trait FileFormatKind {
    /// The word `STORED AS` takes, the key this format is registered under, and the discriminator
    /// a [`SourceFormat`] carries.
    ///
    /// A short lowercase word. It is matched case-insensitively wherever a user types it.
    const NAME: &'static str;
}

/// One `OPTIONS` key a format's reader takes, as the completion offer sees it.
///
/// What the key *does* is the provider's own business — this is only what a row shows and what
/// its value position may offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OptionOffer {
    /// The key as it is written, `format.`-prefixed like every read option.
    pub key: &'static str,
    pub kind: OptionKind,
    /// The short detail a completion row shows beside the key.
    pub what: &'static str,
}

/// The value shape of one `OPTIONS` key — what completion may offer at the key's value position.
///
/// [`Char`](Self::Char) and [`Int`](Self::Int) offer nothing, the values being the user's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionKind {
    Bool,
    Char,
    Int,
    Enum(&'static [&'static str]),
}

/// What a set of `OPTIONS` is being read for.
///
/// Both halves are named by the refusals a read produces: the word the user wrote, and the table
/// whose def the options land on.
#[derive(Clone, Copy, Debug)]
pub struct ReadFor<'a> {
    /// The `STORED AS` word, which is this format's registered name — the same word for every
    /// spelling of it, so a provider registered under two words tells them apart by this.
    pub format: &'a str,
    /// The table being created.
    pub table: &'a str,
}

/// One file format the engine can read, and may be able to write.
///
/// Only [`build`](Self::build) has no default: a format that can be read is the whole of what this
/// seam requires, and everything else describes a format that does more than the minimum.
pub trait FormatProvider: Send + Sync + fmt::Debug + 'static {
    /// Returns the reader `format` describes.
    ///
    /// `format` is always one this provider's own [`read`](Self::read) produced, the registry
    /// having dispatched on [`SourceFormat::name`].
    ///
    /// # Errors
    ///
    /// If the options on the def do not describe a reader. The sentence settles onto the table's
    /// catalog row, so word it as the thing to fix.
    fn build(&self, format: &SourceFormat) -> Result<Arc<dyn FileFormat>, String>;

    /// Reads a statement's `format.*` options onto the def this format is described by.
    ///
    /// The default keeps them verbatim, which is what a format whose options are its own reader's
    /// business wants; the shipped formats override it to land on their own typed structs. The
    /// keys arrive lowercased, with the `format.` prefix the statement wrote, and neither a
    /// duplicate nor an object-store setting reaches here.
    ///
    /// # Errors
    ///
    /// If an option is not one this format takes, named — a key kept and then ignored is a table
    /// that reads differently from what its def says.
    fn read(
        &self,
        at: ReadFor<'_>,
        options: &BTreeMap<String, String>,
    ) -> Result<SourceFormat, String> {
        Ok(SourceFormat::of(at.format, options.clone()))
    }

    /// The file extension a listing filters on for `format`.
    ///
    /// The default is the format's own name plus any whole-file compression the def carries,
    /// because that is what the files are actually called (`events.csv.gz`). Override it for a
    /// format whose files are not named after it.
    fn extension(&self, format: &SourceFormat) -> String {
        format!(".{}{}", format.name(), format.compression().extension())
    }

    /// Whether `COPY … STORED AS <NAME>` writes this format.
    ///
    /// What the Export window's list and the agent's export are filtered on. It is declared
    /// rather than derived from [`writer`](Self::writer), because the shipped formats are written
    /// by DataFusion's own writers and bring none of their own.
    fn copy_to(&self) -> bool {
        false
    }

    /// The writer to register on the session under this format's own name, if it brings one.
    ///
    /// `None` for a format that cannot be written, and for one DataFusion already writes.
    /// Declaring [`copy_to`](Self::copy_to) without either is a `COPY` that fails at plan time in
    /// DataFusion's own words.
    fn writer(&self) -> Option<Arc<dyn FileFormatFactory>> {
        None
    }

    /// The `OPTIONS` keys this format's reader takes, for the completion offer.
    ///
    /// Empty by default, which is the honest answer for a reader with nothing to set and the
    /// right one for a reader whose options are not worth offering. What is offered here must be
    /// what [`read`](Self::read) accepts, or the offer teaches a statement that is then refused.
    fn reader_options(&self) -> Vec<OptionOffer> {
        Vec::new()
    }
}

/// What one registered format is offered as.
///
/// One value for every surface that offers a format — the `STORED AS` completion, the export
/// format list and the agent's export — so a format cannot appear in one and not another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatInfo {
    /// The `STORED AS` word it is registered under.
    pub name: &'static str,
    /// Whether `COPY … TO` can write it.
    pub copy_to: bool,
    /// The `OPTIONS` keys its reader takes.
    pub options: Vec<OptionOffer>,
}

/// One entry: the name it was filed under, and what serves it.
#[derive(Clone)]
struct Registrant {
    name: &'static str,
    provider: Arc<dyn FormatProvider>,
}

impl fmt::Debug for Registrant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Registrant")
            .field("name", &self.name)
            .finish()
    }
}

/// The file formats one engine can read, in the order they were registered.
///
/// Registration order rather than name order because it is an **offer**: the shipped formats
/// register commonest-first, and an embedder's own follow the ones it did not write. Reached from
/// outside as [`FormatInfo`], through [`Engine::formats`](crate::Engine::formats).
#[derive(Clone, Debug, Default)]
pub(crate) struct Formats(Vec<Registrant>);

impl Formats {
    /// The shipped formats, in the order they are offered.
    ///
    /// Each goes through the same [`insert`](Self::insert) that
    /// [`with_format`](crate::EngineBuilder::with_format) calls, so a shipped format is held to
    /// every rule an embedder's is and there is no second way into this registry.
    pub(crate) fn shipped() -> Self {
        let mut formats = Self::default();
        formats.insert(shipped::Parquet);
        formats.insert(shipped::Csv);
        formats.insert(shipped::Json);
        formats.insert(shipped::Arrow);
        formats
    }

    /// Adds `format` under its own name.
    ///
    /// # Panics
    ///
    /// If that name is already registered. A format is not replaceable the way a data source is,
    /// and the reason is the write path: a registrant brings a `FileFormatFactory` that
    /// DataFusion resolves `COPY … STORED AS` against, so taking `parquet` / `csv` / `json` /
    /// `arrow` would swap the writer under every other `COPY` in the session. Composing an engine
    /// is not something that fails at runtime, so this is refused where it is written.
    pub(crate) fn insert<F: FormatProvider + FileFormatKind>(&mut self, format: F) {
        assert!(
            self.find(F::NAME).is_none(),
            "a format is already registered as '{}'. A format cannot be registered over another: \
             the writer DataFusion resolves 'COPY ... STORED AS {}' against is the one registered \
             under that name, so replacing it would change what every other COPY in the session \
             writes. Register it under a name of its own",
            F::NAME,
            F::NAME.to_uppercase()
        );
        self.0.push(Registrant {
            name: F::NAME,
            provider: Arc::new(format),
        });
    }

    fn find(&self, name: &str) -> Option<&Registrant> {
        self.0
            .iter()
            .find(|held| held.name.eq_ignore_ascii_case(name))
    }

    /// Every registered format, in registration order — the one read the `STORED AS` offer, an
    /// export format list and the agent's export share.
    pub(crate) fn registrants(&self) -> Vec<FormatInfo> {
        self.0
            .iter()
            .map(|held| FormatInfo {
                name: held.name,
                copy_to: held.provider.copy_to(),
                options: held.provider.reader_options(),
            })
            .collect()
    }

    /// The reader the table `table`'s `format` describes.
    ///
    /// # Errors
    ///
    /// If nothing is registered under the format's name — the sentence the table's own catalog
    /// row then settles as, which is why it names the table and the fix — or if the registered
    /// reader refused the def.
    pub(crate) fn build(
        &self,
        table: &str,
        format: &SourceFormat,
    ) -> Result<Arc<dyn FileFormat>, String> {
        let Some(held) = self.find(format.name()) else {
            return Err(format!(
                "Table '{table}' is defined as '{}', which no reader is registered for. Register \
                 one with EngineBuilder::with_format, or change the table's format.",
                format.name()
            ));
        };
        held.provider.build(format)
    }

    /// The file extension a listing of `format`'s files filters on.
    ///
    /// A format nothing is registered for answers with the shape every registrant's default
    /// gives, because the caller is about to fail the registration anyway and a listing
    /// extension is not the sentence it should fail with.
    pub(crate) fn extension(&self, format: &SourceFormat) -> String {
        match self.find(format.name()) {
            Some(held) => held.provider.extension(format),
            None => format!(".{}{}", format.name(), format.compression().extension()),
        }
    }

    /// The def a `STORED AS` word and its `format.*` options describe.
    ///
    /// # Errors
    ///
    /// If nothing is registered under `word` — naming every word that is, since this is what a
    /// user typed — or its reader refused one of the options.
    pub(crate) fn read(
        &self,
        word: &str,
        table: &str,
        options: &BTreeMap<String, String>,
    ) -> Result<SourceFormat, String> {
        let Some(held) = self.find(word) else {
            return Err(format!(
                "STORED AS {} is not a format Strata reads. Use {}",
                word.to_uppercase(),
                self.words()
            ));
        };
        held.provider.read(
            ReadFor {
                format: held.name,
                table,
            },
            options,
        )
    }

    /// The `STORED AS` words this engine takes, uppercased, as a sentence lists them.
    pub(crate) fn words(&self) -> String {
        let words: Vec<String> = self.0.iter().map(|held| held.name.to_uppercase()).collect();
        match words.split_last() {
            None => "no format at all".to_string(),
            Some((last, [])) => last.clone(),
            Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        }
    }

    /// Puts every registrant's own writer on `ctx`, under the name it was registered as.
    ///
    /// Called once, where the session is built. A name DataFusion already writes keeps
    /// DataFusion's writer: `register_file_format` refuses an occupied extension, and that
    /// refusal **is** the writer-swap ruling holding — so it is logged and the built-in stands.
    pub(crate) fn register_writers(&self, ctx: &SessionContext) {
        for held in &self.0 {
            let Some(writer) = held.provider.writer() else {
                continue;
            };
            let factory: Arc<dyn FileFormatFactory> = Arc::new(NamedFactory {
                name: held.name,
                writer,
            });
            if let Err(e) = ctx.state_ref().write().register_file_format(factory, false) {
                tracing::warn!(
                    "engine: the format '{}' brought a writer that could not be registered: {e}",
                    held.name
                );
            }
        }
    }
}

/// A registrant's writer, under the name the registrant was filed as.
///
/// DataFusion keys its factory map on `get_ext()`, and a provider's own factory answers with the
/// extension of the *files* it writes — which is not always the word `STORED AS` names. Wrapping
/// it is what keeps the registry key and the `STORED AS` word one thing.
#[derive(Debug)]
struct NamedFactory {
    name: &'static str,
    writer: Arc<dyn FileFormatFactory>,
}

impl GetExt for NamedFactory {
    fn get_ext(&self) -> String {
        self.name.to_string()
    }
}

impl FileFormatFactory for NamedFactory {
    fn create(
        &self,
        state: &dyn Session,
        format_options: &HashMap<String, String>,
    ) -> DfResult<Arc<dyn FileFormat>> {
        self.writer.create(state, format_options)
    }

    fn default(&self) -> Arc<dyn FileFormat> {
        self.writer.default()
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{TestFormat, TestReader};
    use super::*;

    fn at(word: &str) -> (String, BTreeMap<String, String>) {
        (word.to_string(), BTreeMap::new())
    }

    /// Every registered word reads, which is what makes the `STORED AS` offer safe to build from
    /// the registry: an offer the arm then refuses would teach a statement that cannot run.
    #[test]
    fn every_registered_word_reads() {
        let formats = Formats::shipped();
        for info in formats.registrants() {
            let (word, options) = at(info.name);
            formats
                .read(&word, "t", &options)
                .unwrap_or_else(|why| panic!("'{word}' is offered and then refused: {why}"));
        }
    }

    /// A word nothing is registered for is refused **naming every word that is** — so an
    /// embedder's format appears in the sentence the moment it is registered.
    #[test]
    fn an_unregistered_word_is_refused_naming_the_registered_ones() {
        let mut formats = Formats::shipped();
        formats.insert(TestFormat);
        assert_eq!(
            formats.read("avro", "t", &BTreeMap::new()),
            Err(
                "STORED AS AVRO is not a format Strata reads. Use PARQUET, CSV, JSON, ARROW or \
                 TESTFMT"
                    .to_string()
            )
        );
    }

    /// A registrant's own word reads into a def carrying its options verbatim, and that def
    /// builds — the whole read half of the seam, through the public call.
    #[test]
    fn an_embedders_format_reads_its_own_options_onto_the_def() {
        let mut formats = Formats::shipped();
        formats.insert(TestReader);
        let options = BTreeMap::from([(TestReader::HEADER.to_string(), "false".to_string())]);
        let def = formats
            .read("testread", "places", &options)
            .expect("the registrant reads it");
        assert_eq!(
            def,
            SourceFormat::Extension {
                format: "testread".to_string(),
                options
            }
        );
        assert!(formats.build("places", &def).is_ok());
        assert_eq!(
            formats.extension(&def),
            ".tr",
            "the extension is the format's own, not its name"
        );
    }

    /// The word a def carries is matched the way SQL matches a keyword, so `STORED AS TestFmt`
    /// and a def written `testfmt` are one format.
    #[test]
    fn a_format_word_resolves_case_insensitively() {
        let mut formats = Formats::shipped();
        formats.insert(TestFormat);
        assert!(formats.read("TestFmt", "t", &BTreeMap::new()).is_ok());
        assert!(formats
            .build("t", &SourceFormat::from_name("TESTFMT"))
            .is_ok());
    }

    /// **The writer-swap ruling.** A format cannot be registered over one already there, and the
    /// refusal says why rather than reporting a collision.
    #[test]
    #[should_panic(expected = "a format is already registered as 'json'")]
    fn a_format_registered_over_a_shipped_one_is_refused_at_the_builder() {
        #[derive(Debug)]
        struct MyJson;

        impl FileFormatKind for MyJson {
            const NAME: &'static str = "json";
        }

        impl FormatProvider for MyJson {
            fn build(&self, _format: &SourceFormat) -> Result<Arc<dyn FileFormat>, String> {
                unreachable!("registration is refused before anything is built")
            }
        }

        Formats::shipped().insert(MyJson);
    }
}
