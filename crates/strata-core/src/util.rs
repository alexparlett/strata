//! Small shared helpers: SQL hashing, byte formatting, name derivation, wall-clock
//! timestamps, the one case-insensitive substring test every filter shares
//! ([`contains_lowercased`]), and the one crash-safe file write every file `.strata/` owns
//! goes through ([`write_atomic`]). (Domain vocabulary like `Kind` lives in `crate::model`.)

use chrono::Timelike;
use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Wall-clock `HH:MM:SS` in the machine's **local** zone — the timestamp on an event-log row
/// (P3-13).
///
/// Local, unlike [`iso8601`] below, and deliberately: this stamp is read against the clock in the
/// user's menu bar ("did that scan run just now, or before I fixed the path?"), so an unmarked UTC
/// time would be a lie the reader cannot detect. The zone is not a guess — `chrono`'s `clock`
/// feature (already in the graph via datafusion → arrow) reads the real system zone. An absolute
/// instant, which has no such frame of reference, still prints UTC and says so.
pub fn now_hms() -> String {
    let now = chrono::Local::now();
    // Formatted by hand rather than through `strftime`: the same three fields, and it keeps this
    // module's one timestamp shape in one recognisable place.
    format!("{:02}:{:02}:{:02}", now.hour(), now.minute(), now.second())
}

/// An instant as an ISO-8601 timestamp in **UTC** (`2026-07-26T09:22:48Z`) — the exact-time
/// tooltip behind a relative age the UI shows ("scanned 5 min ago").
///
/// **UTC, and marked `Z`.** An absolute instant is read on its own, with no clock beside it to
/// compare against, so the zone has to be *stated* — and a stamp that says which zone it is in is
/// never ambiguous, wherever the reader is. That is the opposite case to [`now_hms`], a wall clock
/// read against the user's own, which is why the two differ.
///
/// Like [`fmt_int`], one path to one function: every surface that prints an instant imports it from
/// here, so two places can't disagree about what a timestamp looks like.
///
/// A pre-epoch instant (a clock set decades back) reads as the epoch rather than as a negative
/// year: total, and the alternative is a panic in a tooltip.
pub fn iso8601(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let (days, sod) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60
    )
}

/// The civil date `days` after 1970-01-01, as `(year, month, day)`.
///
/// Howard Hinnant's `civil_from_days` (the algorithm behind every `chrono`-free date conversion):
/// shift the era to start in March so the leap day lands at the *end* of the year, which is what
/// removes February from the arithmetic entirely. Correct for the proleptic Gregorian calendar,
/// which is what ISO-8601 asks for — including the 400-year rule that makes 2000 a leap year and
/// 2100 not one.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    // Days from 0000-03-01 rather than 1970-01-01.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    // Day of era: [0, 146096].
    let doe = z - era * 146_097;
    // Year of era: [0, 399].
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    // Day of the *March-based* year: [0, 365].
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    // January and February belong to the next calendar year.
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// A stable FNV-1a hash of the **trimmed** SQL — the tab dirty-tracking baseline.
/// Cheaper than storing/comparing whole strings, and deterministic across runs so
/// a persisted baseline still matches after reload.
pub fn sql_hash(sql: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in sql.trim().as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The SQL as one line — every run of whitespace collapsed to a single space, ends trimmed.
///
/// Deliberately **one** function for two jobs, because they have to agree: it is the History
/// drawer's row preview *and* the key query history dedupes on. A key looser than the preview
/// would collapse two rows a reader can tell apart; a key tighter than it would let two visually
/// identical rows sit in the list. Sharing it makes both impossible.
///
/// Whitespace only — never case, and never anything that looks inside the statement. Quoted
/// identifiers are case-sensitive, so `"Id"` and `"id"` are different queries; re-indenting one
/// is not.
pub fn collapse_sql(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Characters a **displayed** value keeps before it is clipped — a grid cell, a value-tree leaf, a
/// string inside a record-view preview. One number, because those surfaces sit next to each other:
/// a field row whose scalar branch clipped at a different length from its nested branch would read
/// as two different rules.
pub const DISPLAY_CHARS: usize = 400;

/// `s` capped at `max` **characters**, clipped on a char boundary with a trailing `…`.
///
/// Characters, not bytes: a byte cap silently shortens a CJK or emoji string to a third of the
/// length it promises, and the cap exists to bound what is *read*. Like [`fmt_int`], one path to one
/// function — the grid's cells, the value tree's leaves and the preview's strings all clip here, so
/// no two of them can disagree about where a value stops.
pub fn clip(s: &str, max: usize) -> Cow<'_, str> {
    // `take(max + 1)` distinguishes "exactly max" from "longer" without walking the whole string.
    if s.chars().take(max + 1).count() <= max {
        return Cow::Borrowed(s);
    }
    let end = s.char_indices().nth(max).map_or(s.len(), |(i, _)| i);
    Cow::Owned(format!("{}…", &s[..end]))
}

/// `haystack.to_lowercase().contains(needle)` — with `needle` already lowercased once by the
/// caller — **without** the per-call `String` that form allocates. Written for find-in-results,
/// where a 1000-row page times its column count is tens of thousands of allocations per
/// keystroke on the render thread; here because it is *the* case-insensitive substring test.
/// Like [`fmt_int`], one path to one function: the results find, the sidebar catalog's filter,
/// the launcher's search, the tab switcher's, the export partition picker's and the engine key
/// suggestions all ask the same question, and two implementations of it would let the same
/// needle produce different sets on different surfaces.
///
/// Lowercasing is Unicode-aware, so it is *not* a windowed byte compare: one char can lower
/// to several ('İ' → "i̇") and to a different byte length ('K' U+212A → 'k'). This walks the
/// haystack's **lowercased char stream** from each starting char instead, so expansions fall
/// out naturally and nothing is allocated.
///
/// The allocating form searches every position of the *lowered* string, and a char that lowers
/// to several contributes several of them — so the starts tried here are the positions **inside**
/// each char's expansion, not just its first. Without that inner loop a needle beginning
/// mid-expansion (a bare combining dot against "İstanbul") would be missed, which is a genuine
/// difference in result and not just in spelling.
///
/// One divergence from `str::to_lowercase` remains, which is the only context-sensitive case in
/// it: word-final 'Σ' lowers to 'ς' there but to 'σ' char-wise, so the two sigma forms are folded
/// together here. That makes the match a strict *superset* of the allocating form — a needle
/// in either sigma form finds both — rather than silently dropping matches at word ends.
pub fn contains_lowercased(haystack: &str, needle: &str) -> bool {
    // An empty needle matches everything, `str::contains`-style — including an empty haystack,
    // which has no starting char to try. (A caller that trims its query first never hands one
    // over, but the equivalence this function claims shouldn't have a hole in it.)
    needle.is_empty()
        || haystack.char_indices().any(|(i, c)| {
            // `count()` is 1 for all but a handful of chars ('İ' is the only one Rust maps to
            // more than one lowercase char without context), so this is a one-iteration loop
            // on the hot path.
            (0..c.to_lowercase().count())
                .any(|skip| starts_with_lowercased(&haystack[i..], skip, needle))
        })
}

/// Does `haystack`, lowercased char by char, *start with* the (already lowercase) `needle` —
/// beginning `skip` chars into the **first** char's lowercase expansion?
///
/// Consumes the needle against each char's expansion, so a match may begin or end part-way
/// through one: "i" matches "İstanbul" (whose 'İ' lowers to "i" + a combining dot) and so does
/// the combining dot on its own. `skip` is always less than the first char's expansion length,
/// so it is spent before the second char is reached.
fn starts_with_lowercased(haystack: &str, mut skip: usize, needle: &str) -> bool {
    let mut needle = needle.chars();
    let mut want = needle.next();
    for c in haystack.chars() {
        for lc in c.to_lowercase() {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let Some(w) = want else { return true };
            if fold_sigma(w) != fold_sigma(lc) {
                return false;
            }
            want = needle.next();
        }
    }
    want.is_none()
}

/// Greek final sigma folded onto plain sigma — see [`contains_lowercased`].
fn fold_sigma(c: char) -> char {
    if c == 'ς' {
        'σ'
    } else {
        c
    }
}

/// Group a non-negative integer with thousands separators (`48213` → `48,213`).
///
/// Every surface that prints a count imports it from here — the EXPLAIN plan's metrics, the
/// results footer and the column inspector's row counts — so two places can't disagree about
/// how a number looks. Deliberately not re-exported from any of them: one path to one function.
pub fn fmt_int(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

/// Resolve a **single-character field**: the two escapes the canvases document (`\t`, `\n`), a
/// literal backslash, or one plain character. Empty is `None` (such a field is optional);
/// anything longer is an error the surface shows rather than a silent truncation.
///
/// Shared, because a delimiter, a quote and a comment marker are the same field wherever they
/// appear — and there are now three surfaces: the export window, the Configure window, and a
/// typed `CREATE EXTERNAL TABLE`'s `OPTIONS` (ED-10). A `\t` that resolved in one and not the
/// others would be the same field meaning two things, and the typed statement is the one that
/// has to land on the def Configure would have written.
///
/// **Not DataFusion's `u8` config parse**, which the same key goes through in `datafusion-cli`:
/// that reads a numeric string as the byte *value*, so `'format.delimiter' '9'` silently means
/// tab, and it has no escape for one — a `'\t'` reaches it as two characters and is refused as
/// "Non-ASCII". This is the rule the two windows already publish, and `what` names the field so
/// the message reads the same wherever it is raised.
pub fn one_char(what: &str, raw: &str) -> Result<Option<char>, String> {
    let resolved = match raw {
        "" => return Ok(None),
        "\\t" => '\t',
        "\\n" => '\n',
        "\\\\" => '\\',
        other => {
            let mut chars = other.chars();
            let first = chars.next().expect("non-empty");
            if chars.next().is_some() {
                return Err(format!(
                    "The CSV {what} has to be a single character (or \\t for tab), not {other:?}"
                ));
            }
            first
        }
    };
    Ok(Some(resolved))
}

/// A counted noun — `12 columns`, `1 problem` — with the count grouped by [`fmt_int`].
///
/// **Regular nouns only** (`+s`), which is every noun the UI counts: columns, rows, problems,
/// tables, views, files, events. A count that needs an irregular plural needs its own sentence
/// anyway.
pub fn plural(n: usize, noun: &str) -> String {
    format!("{} {}", fmt_int(n as u64), plural_noun(n, noun))
}

/// `noun` agreeing with `n`, **without** the count — for a phrase that puts something between the
/// two (`… 19,296 more keys`). [`plural`] is this plus the number, and shares it so the two cannot
/// disagree about a plural.
pub fn plural_noun(n: usize, noun: &str) -> Cow<'_, str> {
    match n {
        1 => Cow::Borrowed(noun),
        _ => Cow::Owned(format!("{noun}s")),
    }
}

/// How long ago something happened, `secs` seconds old — `just now`, `4 min ago`, `3 h ago`,
/// `2 d ago`.
///
/// Coarse on purpose: every surface stating an age wants to say whether a number is minutes or
/// days old, and a figure any more precise would have to tick to stay true. Here rather than in
/// either caller because the inspector's scan age and the History drawer's timestamps are the
/// same sentence about different things, and two spellings of it is exactly the near-duplicate
/// wording AGENTS.md §3 says to merge.
pub fn ago(secs: u64) -> String {
    match secs {
        s if s < 60 => "just now".to_string(),
        s if s < 3600 => format!("{} min ago", s / 60),
        s if s < 86_400 => format!("{} h ago", s / 3600),
        s => format!("{} d ago", s / 86_400),
    }
}

/// Human-readable byte size (e.g. `1.4 MB`).
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut f = n as f64;
    let mut i = 0;
    while f >= 1024.0 && i < UNITS.len() - 1 {
        f /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{f:.1} {}", UNITS[i])
    }
}

/// A path's last component for display — how every surface names a project folder (dialog
/// subject lines, the header switcher's rows). Falls back to the whole path when there is
/// no final component (`/`), so the subject is never blank. Display only: the
/// SQL-identifier mangle is [`derive_table_name`]'s, and the scaffold's `"untitled"`
/// fallback is deliberately its own.
pub fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Turn a file/dir name into a valid, unique `lower_snake` SQL identifier.
pub fn derive_table_name(path: &Path, existing: &BTreeSet<String>) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("table");
    let mut base: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    if base.is_empty() {
        base = "table".into();
    }
    if base.chars().next().is_none_or(|c| c.is_ascii_digit()) {
        base = format!("t_{base}");
    }
    let mut name = base.clone();
    let mut i = 2;
    while existing.contains(&name) {
        name = format!("{base}_{i}");
        i += 1;
    }
    name
}

/// Split `"1.5G"` → `("1.5", "G")` — the leading numeric run and the trailing unit.
fn split_num_unit(s: &str) -> (&str, &str) {
    let idx = s.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(s.len());
    (s[..idx].trim(), s[idx..].trim())
}

/// A number with an optional byte-size unit (K/M/G/T, optionally `i`/`B`).
pub fn is_byte_size(v: &str) -> bool {
    let (num, unit) = split_num_unit(v);
    if num.parse::<f64>().is_err() {
        return false;
    }
    unit.is_empty()
        || matches!(
            unit.chars().next().map(|c| c.to_ascii_lowercase()),
            Some('k' | 'm' | 'g' | 't' | 'b')
        )
}

/// A number with an optional duration unit (s/m/h).
pub fn is_duration(v: &str) -> bool {
    parse_duration(v).is_some()
}

/// A duration written as a number with an optional s/m/h unit (a bare number is seconds), or
/// `None` if it isn't one.
///
/// [`is_duration`] *is* this — the validator that answers "would this apply?" and the parser that
/// applies it have to agree, and the only way to guarantee that is for there to be one of them.
/// A settings field that accepts `2h` and an engine that then reads it as two seconds is the
/// failure mode worth designing out.
pub fn parse_duration(v: &str) -> Option<Duration> {
    let (num, unit) = split_num_unit(v);
    let num: f64 = num.parse().ok()?;
    let seconds = match unit.chars().next().map(|c| c.to_ascii_lowercase()) {
        None | Some('s') => num,
        Some('m') => num * 60.,
        Some('h') => num * 3600.,
        Some(_) => return None,
    };
    // `try_from_secs_f64`, not `from_secs_f64`: the latter **panics** on a value `Duration` can't
    // hold, and this function is the Properties editor's per-keystroke validator — a number too
    // big to be a duration is something a user types on the way to a smaller one, not a bug. It
    // is also the only bound worth stating: it rejects negative, non-finite and overflowing
    // values, so a hand-rolled guard beside it could only drift from it. That covers the
    // multiplication above too, which is why `seconds` is what's checked rather than `num`.
    Duration::try_from_secs_f64(seconds).ok()
}

/// A `±HH:MM` offset (hours 00-14, minutes 00-59) or a named zone (letters, digits, `/_+-`).
pub fn is_time_zone(v: &str) -> bool {
    if let Some(rest) = v.strip_prefix(['+', '-']) {
        let b = rest.as_bytes();
        return rest.len() == 5
            && b[2] == b':'
            && matches!(
                (rest[0..2].parse::<u32>(), rest[3..5].parse::<u32>()),
                (Ok(h), Ok(m)) if h <= 14 && m <= 59
            );
    }
    v.chars().any(|c| c.is_ascii_alphabetic())
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '+' | '-'))
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Epoch millis now — what a satellite stamps a record with so it can be ordered later.
///
/// Beside [`now_secs`] rather than copied into each satellite that wants one: the history log
/// and the chat store both order by it, and two private copies of the same four lines is two
/// places for the fallback to disagree.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Distinguishes concurrent temp files written by *this* process (see [`write_atomic`]).
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// The first component of every [`write_atomic`] temp name — a `.`, so a temp is hidden
/// from a plain listing for the moment it exists.
const TEMP_PREFIX: &str = ".";
/// The last component of every [`write_atomic`] temp name.
const TEMP_SUFFIX: &str = ".tmp";

/// The `.gitignore` pattern covering every [`write_atomic`] temp: [`TEMP_PREFIX`], any
/// name, [`TEMP_SUFFIX`]. (A gitignore `*` matches a leading dot — gitignore is fnmatch,
/// not a shell, and has no "hidden file" rule — so this does cover `.session.json.41.0.tmp`.)
///
/// **Any directory written through [`write_atomic`] has to ignore this.** The error path
/// removes its temp, but a `SIGKILL` or power loss between `File::create` and `fs::rename`
/// has no error path to run — precisely the crash the helper exists to survive — so a
/// stranded temp is possible by construction, and in a committed project folder it would
/// otherwise surface as untracked. [`sweep_stale_temps`] clears it; this keeps it invisible
/// until then.
///
/// One literal, beside the writer ([`temp_name`]), the reader ([`temp_pid`]) and the sweep,
/// so the four cannot drift — asserted in this module's tests.
pub const TEMP_GLOB: &str = ".*.tmp";

/// How long a temp must have sat untouched before [`sweep_stale_temps`] will remove it.
/// [`write_atomic`] writes one small file and renames it in the next breath, so an hour is
/// orders of magnitude past any write in flight; the margin is for a suspended laptop, a
/// process stopped under a debugger, and coarse or skewed filesystem clocks.
const TEMP_STALE_AGE: Duration = Duration::from_secs(60 * 60);

/// The temp name [`write_atomic`] publishes `name` through: [`TEMP_GLOB`]'s prefix and
/// suffix around the target's own name, the writing **pid**, and a process-local counter —
/// so two threads, two windows or two app instances saving the same file can't scribble on
/// one another's temp. The pid is what later lets [`sweep_stale_temps`] tell a temp its
/// writer abandoned from one a live writer is still filling.
fn temp_name(name: &str) -> String {
    format!(
        "{TEMP_PREFIX}{name}.{}.{}{TEMP_SUFFIX}",
        process::id(),
        TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

/// The pid recorded in `name` if `name` is one of our temps — the inverse of [`temp_name`],
/// and `None` for everything else in the directory, which is what stops the sweep touching
/// a file that merely looks temp-ish. Parsed from the **right**: the wrapped file name has
/// dots of its own (`.session.json.4711.7.tmp` → `4711`).
fn temp_pid(name: &str) -> Option<u32> {
    let inner = name.strip_prefix(TEMP_PREFIX)?.strip_suffix(TEMP_SUFFIX)?;
    // What's left is `<file>.<pid>.<seq>`: peel the seq, then the pid. Both must parse as
    // numbers and the wrapped file name must be non-empty, or this is not a name we wrote —
    // some other tool's `.swap.tmp` is not ours to delete.
    let (inner, seq) = inner.rsplit_once('.')?;
    seq.parse::<u64>().ok()?;
    let (file, pid) = inner.rsplit_once('.')?;
    if file.is_empty() {
        return None;
    }
    pid.parse().ok()
}

/// Remove [`write_atomic`] temps stranded in `dir` by a process that died between creating
/// one and renaming it. Best-effort and silent: housekeeping on the way past a save, never
/// a reason to fail one.
///
/// **What makes a temp safe to unlink.** Its name carries the pid of the process that
/// created it ([`temp_name`]), and unlinking a temp a live writer still holds open turns
/// that writer's good save into a failed one. So:
///
/// * temps written by **this** process are never swept — another thread may be between
///   `File::create` and `fs::rename` right now, and our own pid is the one pid we know for
///   certain belongs to a running process;
/// * for any **other** pid we can't ask the OS whether it is still alive (that needs a
///   syscall crate, and pid reuse would make the answer a lie anyway), so age stands in for
///   liveness: a temp untouched for [`TEMP_STALE_AGE`] cannot be a write in flight.
///
/// Anything that can't be classified — an unreadable entry, a missing mtime, an mtime in
/// the future (a clock-skewed network mount) — is left alone. Littering is the cheap
/// failure here; deleting a live write is not.
pub fn sweep_stale_temps(dir: &Path) {
    sweep_temps_older_than(dir, TEMP_STALE_AGE);
}

/// [`sweep_stale_temps`] with the threshold injected, so both arms are testable without
/// waiting an hour or back-dating files.
fn sweep_temps_older_than(dir: &Path, min_age: Duration) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let own = process::id();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(temp_pid) else {
            continue;
        };
        if pid == own {
            continue;
        }
        if abandoned(&entry, min_age) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Whether `entry` has sat untouched for `min_age` — the age half of "no live writer owns this".
/// Anything unreadable, or with an mtime in the future (a clock-skewed network mount), answers
/// `false`: littering is the cheap failure, deleting a live write is not.
fn abandoned(entry: &fs::DirEntry, min_age: Duration) -> bool {
    entry
        .metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age >= min_age)
}

/// The first component of every [`temp_dir_name`] — the **directory** counterpart of
/// [`TEMP_PREFIX`], and a `.` for the same reason.
///
/// Its own prefix rather than [`TEMP_GLOB`]'s, because the two are published differently and
/// swept differently: a temp *file* is renamed over a file, a temp *directory* is renamed into
/// place as a whole, and only the second can be removed with `remove_dir_all`.
const TEMP_DIR_PREFIX: &str = ".tmp-";

/// A temp **directory** name for a caller that builds a directory and then publishes it by
/// rename — an internal table's Arrow spool (ED-04).
///
/// Carries the writing pid and a process-local counter, exactly as [`temp_name`] does and for
/// the same reason: two windows spooling the same table must not share a staging directory, and
/// the pid is what later lets [`sweep_stale_temp_dirs`] tell an abandoned spool from one a live
/// process is still filling.
pub fn temp_dir_name() -> String {
    format!(
        "{TEMP_DIR_PREFIX}{}-{}",
        process::id(),
        TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

/// The pid recorded in `name` if `name` is one of our temp directories — the inverse of
/// [`temp_dir_name`], and `None` for everything else in the directory, which is what stops the
/// sweep touching a directory that merely looks temp-ish.
fn temp_dir_pid(name: &str) -> Option<u32> {
    let (pid, seq) = name.strip_prefix(TEMP_DIR_PREFIX)?.split_once('-')?;
    seq.parse::<u64>().ok()?;
    pid.parse().ok()
}

/// Remove temp **directories** stranded in `dir` by a process that died between filling one and
/// renaming it into place. Best-effort and silent, like [`sweep_stale_temps`], and safe by the
/// same two rules: never this process's own (another thread may be spooling right now), and for
/// any other pid, age stands in for liveness.
///
/// [`TEMP_STALE_AGE`] is generous for a `write_atomic` and merely sufficient here — a CTAS over a
/// large lake is the one write in this codebase that can legitimately run for minutes. An hour is
/// still well past it, and the exposure is narrow: only a *different* process's spool is ever
/// eligible, and the cost of getting it wrong is one interrupted CTAS rather than lost data.
pub fn sweep_stale_temp_dirs(dir: &Path) {
    sweep_temp_dirs_older_than(dir, TEMP_STALE_AGE);
}

/// [`sweep_stale_temp_dirs`] with the threshold injected, so both arms are testable without
/// waiting an hour.
fn sweep_temp_dirs_older_than(dir: &Path, min_age: Duration) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let own = process::id();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(temp_dir_pid) else {
            continue;
        };
        if pid == own || !abandoned(&entry, min_age) {
            continue;
        }
        let _ = fs::remove_dir_all(entry.path());
    }
}

/// Write `contents` to `path` **atomically**: into a temp file first, flushed to disk, then
/// renamed over the target. Every file `.strata/` owns (`project.json`, `session.json`, the
/// rotated `history.jsonl`, its `.gitignore`) goes through here instead of [`fs::write`],
/// which truncates the target *before* writing — a crash, kill or power loss mid-write
/// leaves an empty or half-written file, and these files are the user's catalog and working
/// session. A rename is all-or-nothing, so a reader only ever sees the whole old file or the
/// whole new one. (The app config is not ours to write this way — the `preferences` crate
/// owns that file; see [`crate::config::save`].)
///
/// The temp file is deliberately created in the **same directory** as `path`: `rename` is
/// only atomic within one filesystem, so a temp in the OS temp dir would degrade into a
/// cross-device copy. Its name ([`temp_name`]) carries the pid + a process-local counter so
/// two windows — or two app instances — saving the same file can't scribble on one
/// another's temp. On any *returned* failure the temp is removed and the previous file is
/// left exactly as it was; a kill has no error path to run, so its temp survives — see
/// [`TEMP_GLOB`] (keeps it out of the project's git status) and [`sweep_stale_temps`]
/// (clears it once it can't belong to a live writer).
///
/// Durability stops at the file: we fsync the temp's *contents* but not the directory entry,
/// so a power loss immediately after a save can roll back to the previous version. That
/// costs a save, never the file — which is the guarantee that matters here.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    // `parent()` is `Some("")` for a bare file name (write into the cwd) and `None` only for
    // a root path — neither is a directory we can put a temp in, so fall back to the cwd.
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "out".into());
    let temp = dir.join(temp_name(&name));

    let write = || -> io::Result<()> {
        let mut file = File::create(&temp)?;
        file.write_all(contents)?;
        // The bytes must reach the disk *before* the rename publishes them; without this a
        // crash can land the rename and lose the contents — a zero-length "new" file.
        file.sync_all()?;
        // Closed before the rename: Windows won't rename a file that's still open.
        drop(file);
        fs::rename(&temp, path)
    };
    let res = write();
    if res.is_err() {
        // Never strand a temp beside the real file (a failed rename leaves one behind).
        let _ = fs::remove_file(&temp);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::path::PathBuf;

    /// A fresh temp directory, cleaned up on drop.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = env::temp_dir().join(format!("strata-util-test-{tag}-{}", process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        /// The directory's entries, sorted — used to prove no `.tmp` is left behind.
        fn entries(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(&self.0)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// The instants a tooltip has to get right, against values taken from a real calendar
    /// implementation rather than from the algorithm being tested. The two leap cases are the
    /// point: 2000 *is* a leap year (the 400-rule) and 2100 is *not* (the 100-rule), and an
    /// implementation that fumbles either is only wrong for a day at a time — which is exactly
    /// the kind of wrong nobody notices in a tooltip.
    #[test]
    fn instants_print_as_iso_8601_utc() {
        let at = |secs: u64| iso8601(UNIX_EPOCH + Duration::from_secs(secs));
        assert_eq!(at(0), "1970-01-01T00:00:00Z");
        assert_eq!(at(1_000_000_000), "2001-09-09T01:46:40Z");
        assert_eq!(at(1_735_689_600), "2025-01-01T00:00:00Z");
        assert_eq!(at(1_774_521_768), "2026-03-26T10:42:48Z");
        assert_eq!(
            at(951_782_400),
            "2000-02-29T00:00:00Z",
            "2000 is a leap year"
        );
        assert_eq!(
            at(4_107_542_400),
            "2100-03-01T00:00:00Z",
            "2100 is not — the day after 2100-02-28"
        );
    }

    /// A clock set before the epoch reads as the epoch rather than panicking or printing a
    /// negative year. A tooltip is not a place to fail.
    #[test]
    fn a_pre_epoch_instant_reads_as_the_epoch() {
        assert_eq!(
            iso8601(UNIX_EPOCH - Duration::from_secs(60)),
            "1970-01-01T00:00:00Z"
        );
    }

    #[test]
    fn ints_group_by_thousands() {
        assert_eq!(fmt_int(0), "0");
        assert_eq!(fmt_int(999), "999");
        assert_eq!(fmt_int(1_000), "1,000");
        assert_eq!(fmt_int(48_213), "48,213");
        assert_eq!(fmt_int(2_413_118), "2,413,118");
    }

    /// A counted noun agrees with its count, and rides the same grouping as every other figure.
    #[test]
    fn counted_nouns_agree_and_group() {
        assert_eq!(plural(0, "problem"), "0 problems");
        assert_eq!(plural(1, "problem"), "1 problem");
        assert_eq!(plural(2, "column"), "2 columns");
        assert_eq!(plural(48_213, "row"), "48,213 rows");
    }

    #[test]
    fn write_atomic_creates_replaces_and_leaves_no_temp() {
        let dir = TempDir::new("write");
        let path = dir.0.join("f.json");
        write_atomic(&path, b"first").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "first");
        // Replacing is the common case (every autosave) — the rename must overwrite.
        write_atomic(&path, b"second").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second");
        assert_eq!(dir.entries(), ["f.json"], "no temp files survive a save");
    }

    #[test]
    fn write_atomic_failed_rename_strands_no_temp() {
        let dir = TempDir::new("fail-rename");
        // A non-empty directory can't be renamed over: the temp is created and written, and
        // the failure lands on the rename — the error path that has a temp to clean up.
        let path = dir.0.join("occupied");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("inner"), "x").unwrap();
        assert!(write_atomic(&path, b"nope").is_err());
        assert_eq!(dir.entries(), ["occupied"]);
        assert_eq!(fs::read_to_string(path.join("inner")).unwrap(), "x");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_failure_leaves_the_previous_file_intact() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("fail-readonly");
        let path = dir.0.join("f.json");
        write_atomic(&path, b"good").unwrap();
        // A read-only directory fails the temp's `create` — nothing touches the target.
        fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o500)).unwrap();
        let res = write_atomic(&path, b"bad");
        fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(res.is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "good");
        assert_eq!(dir.entries(), ["f.json"]);
    }

    /// The writer, the reader and the `.gitignore` line are one shape or they are a bug:
    /// a temp the ignorer misses is the untracked file in a committed project folder, and
    /// a temp the parser misses is one the sweep can never remove.
    #[test]
    fn the_temp_name_the_writer_makes_is_the_one_the_glob_and_the_parser_describe() {
        assert_eq!(
            TEMP_GLOB,
            format!("{TEMP_PREFIX}*{TEMP_SUFFIX}"),
            "the gitignore pattern must be exactly the writer's prefix/suffix"
        );
        // A dotted target name is the normal case here (`session.json`), and the reason the
        // pid is parsed from the right.
        let name = temp_name("session.json");
        assert!(name.starts_with(TEMP_PREFIX) && name.ends_with(TEMP_SUFFIX));
        assert_eq!(temp_pid(&name), Some(process::id()));
        // Neither a plain file nor a foreign `.tmp` is ours to sweep.
        assert_eq!(temp_pid("session.json"), None);
        assert_eq!(temp_pid(".editor-swap.tmp"), None);
        assert_eq!(temp_pid(".session.json.notapid.0.tmp"), None);
    }

    /// A temp stranded by a killed writer is swept; one this process may still be filling
    /// never is, whatever its age — unlinking it would break a good save.
    #[test]
    fn sweep_removes_a_dead_writers_temp_and_never_our_own() {
        let dir = TempDir::new("sweep");
        let own = process::id();
        let foreign = own.wrapping_add(1);
        fs::write(dir.0.join(format!(".session.json.{foreign}.0.tmp")), b"x").unwrap();
        fs::write(dir.0.join(format!(".session.json.{own}.9.tmp")), b"x").unwrap();
        // Neither a real file nor something else's `.tmp` is ours to delete.
        fs::write(dir.0.join("session.json"), b"{}").unwrap();
        fs::write(dir.0.join(".editor-swap.tmp"), b"x").unwrap();

        // Age zero: every temp qualifies on age, so only the pid rule can spare one.
        sweep_temps_older_than(&dir.0, Duration::ZERO);

        assert_eq!(
            dir.entries(),
            [
                ".editor-swap.tmp",
                format!(".session.json.{own}.9.tmp").as_str(),
                "session.json"
            ]
        );
    }

    /// Age is the stand-in for "is the writer still alive": a temp younger than the
    /// threshold may be another instance's write in flight, so it stays.
    #[test]
    fn sweep_spares_a_temp_too_young_to_be_abandoned() {
        let dir = TempDir::new("sweep-young");
        let foreign = process::id().wrapping_add(1);
        let name = format!(".session.json.{foreign}.0.tmp");
        fs::write(dir.0.join(&name), b"x").unwrap();
        sweep_temps_older_than(&dir.0, Duration::from_secs(3600));
        assert_eq!(dir.entries(), [name]);
    }

    /// Housekeeping never gets in the way: a directory that isn't there is a no-op.
    #[test]
    fn sweep_of_a_missing_directory_is_a_no_op() {
        let dir = TempDir::new("sweep-missing");
        sweep_stale_temps(&dir.0.join("nope"));
        sweep_stale_temp_dirs(&dir.0.join("nope"));
    }

    /// The **directory** sweep, on the same two rules as the file one: a spool a dead process
    /// left behind goes, ours never does whatever its age, and a real table's directory is not
    /// a temp at all.
    #[test]
    fn the_directory_sweep_removes_a_dead_spool_and_never_our_own() {
        let dir = TempDir::new("sweep-dirs");
        let own = process::id();
        let foreign = own.wrapping_add(1);
        let mine = temp_dir_name();
        assert_eq!(temp_dir_pid(&mine), Some(own));
        // Not ours to touch: a real internal table's directory, and something else's `.tmp-`.
        assert_eq!(temp_dir_pid("orders"), None);
        assert_eq!(temp_dir_pid(".tmp-notapid-0"), None);

        for name in [
            format!(".tmp-{foreign}-0").as_str(),
            mine.as_str(),
            ".tmp-notapid-0",
            "orders",
        ] {
            fs::create_dir_all(dir.0.join(name).join("nested")).unwrap();
        }

        // Age zero: every spool qualifies on age, so only the pid rule can spare one.
        sweep_temp_dirs_older_than(&dir.0, Duration::ZERO);

        // Sorted by `entries`: a numeric pid sorts before `notapid`, and both before `orders`.
        assert_eq!(dir.entries(), [mine.as_str(), ".tmp-notapid-0", "orders"]);
    }

    /// Age is the stand-in for liveness here too: a CTAS in another window may still be
    /// spooling, so a young directory stays.
    #[test]
    fn the_directory_sweep_spares_a_spool_too_young_to_be_abandoned() {
        let dir = TempDir::new("sweep-dirs-young");
        let name = format!(".tmp-{}-0", process::id().wrapping_add(1));
        fs::create_dir_all(dir.0.join(&name)).unwrap();
        sweep_temp_dirs_older_than(&dir.0, Duration::from_secs(3600));
        assert_eq!(dir.entries(), [name]);
    }

    #[test]
    fn a_duration_carries_its_unit() {
        assert_eq!(parse_duration("30"), Some(Duration::from_secs(30)));
        assert_eq!(parse_duration("30s"), Some(Duration::from_secs(30)));
        assert_eq!(parse_duration("2m"), Some(Duration::from_secs(120)));
        assert_eq!(parse_duration("1h"), Some(Duration::from_secs(3600)));
        assert_eq!(parse_duration(" 90 S "), Some(Duration::from_secs(90)));
        assert_eq!(parse_duration("nonsense"), None);
        assert_eq!(parse_duration("5d"), None, "days are not a unit we take");
    }

    /// A number too big to be a `Duration` is something a user types on the way to a smaller one,
    /// so it has to come back `None` rather than take the app down. `from_secs_f64` **panics** on
    /// overflow, and this function is `is_duration` — the per-keystroke validator behind the
    /// Properties editor's `Kind::Duration` rows — so the panic would land before Apply is ever
    /// pressed.
    #[test]
    fn a_duration_too_large_to_hold_is_refused_rather_than_fatal() {
        for huge in ["99999999999999999999", "1e30", "99999999999999999h"] {
            assert_eq!(parse_duration(huge), None, "{huge}");
            assert!(!is_duration(huge), "{huge}");
        }
        // Negative and non-finite go the same way, through the same constructor.
        assert_eq!(parse_duration("-1"), None);
        assert_eq!(parse_duration("-5m"), None);
        assert_eq!(parse_duration("inf"), None);
        assert_eq!(parse_duration("NaN"), None);
    }

    /// The allocation-free match must agree with the `to_lowercase().contains()` form it
    /// replaced — including where lowering changes a char's byte length or char count, which
    /// is exactly what a windowed byte compare would get wrong.
    #[test]
    fn matching_agrees_with_the_allocating_form_on_non_ascii() {
        // (haystack, needle — already lowercased, as every caller hands it over).
        let cases: &[(&str, &str)] = &[
            ("CAFÉ au lait", "café"),
            ("Straße", "straße"),
            ("ÅNGSTRÖM", "ström"),
            // U+212A KELVIN SIGN lowers to a 1-byte 'k' — three bytes become one.
            ("\u{212A}ELVIN", "kelvin"),
            // 'İ' lowers to TWO chars ("i" + U+0307), so a needle can end mid-expansion —
            // and a needle that skips the combining dot does *not* match, in either form.
            ("İstanbul", "i"),
            ("İstanbul", "istanbul"),
            ("İstanbul", "i\u{307}stanbul"),
            // …and it can *begin* mid-expansion too: the allocating form searches every
            // position of the lowered string, including the one the 'İ' expanded into.
            ("İstanbul", "\u{307}stanbul"),
            ("İ", "\u{307}"),
            ("日本語のテキスト", "本語"),
            // Near-misses: an accent is not its bare letter, and a needle can outrun the text.
            ("cafe", "café"),
            ("é", "éé"),
            ("", "x"),
            ("", ""),
        ];
        for (haystack, needle) in cases {
            assert_eq!(
                contains_lowercased(haystack, needle),
                haystack.to_lowercase().contains(*needle),
                "{haystack:?} contains {needle:?}"
            );
        }
        // The expansion cases are meant to *match* — pin that down too, so an agreeing pair
        // of `false`s can't pass for equivalence.
        assert!(contains_lowercased("İstanbul", "i"));
        assert!(contains_lowercased("İstanbul", "i\u{307}stanbul"));
        assert!(contains_lowercased("\u{212A}ELVIN", "kelvin"));
        assert!(!contains_lowercased("cafe", "café"));
    }

    /// A needle that begins **inside** a char's lowercase expansion. `str::to_lowercase`
    /// searches every position of the string it built, and 'İ' contributes two of them; a scan
    /// that only tried the first char of each expansion would silently miss the second. The
    /// only char Rust maps to more than one lowercase char without context, so this is the
    /// whole of the case — but the equivalence the function claims has to hold for it.
    #[test]
    fn a_needle_can_begin_mid_expansion() {
        assert!(contains_lowercased("İ", "\u{307}"));
        assert!(contains_lowercased("İstanbul", "\u{307}stanbul"));
        // Not a free-for-all: the dot is the *second* char of that expansion, so a needle
        // that wants it first still has to match what follows.
        assert!(!contains_lowercased("İstanbul", "\u{307}i"));
    }

    /// The one deliberate divergence (see `contains_lowercased`): `str::to_lowercase` maps a
    /// word-final 'Σ' to 'ς', so the allocating form missed a "σ" needle there. Folding the
    /// two sigma forms together finds the row under either spelling.
    #[test]
    fn either_sigma_form_finds_the_other() {
        assert!(contains_lowercased("ΟΔΟΣ", "σ"));
        assert!(contains_lowercased("ΟΔΟΣ", "ς"));
        assert!(contains_lowercased("οδος", "ς"));
        // …which the form this replaced did not do.
        assert!(!"ΟΔΟΣ".to_lowercase().contains('σ'));
    }

    /// Clipping counts **characters**, and lands on a boundary rather than inside one. The grid's
    /// cells clipped by bytes until this became shared, which cut a CJK string to a third of the
    /// 400 it promised.
    #[test]
    fn clip_counts_characters_not_bytes() {
        assert_eq!(clip("h\u{e9}llo", 5), Cow::Borrowed("h\u{e9}llo"));
        assert_eq!(
            clip("h\u{e9}llo", 3),
            Cow::Owned::<str>("h\u{e9}l\u{2026}".into())
        );
        // Five multi-byte chars is ten bytes: a byte cap would have clipped this, chars do not.
        assert_eq!(
            clip("\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}", 5),
            Cow::Borrowed("\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}")
        );
        assert_eq!(clip("", 3), Cow::Borrowed(""));
    }
}
