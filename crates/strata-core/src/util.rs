//! Small shared helpers: SQL hashing, byte formatting, name derivation, wall-clock
//! timestamps, and the one crash-safe file write every file `.strata/` owns goes through
//! ([`write_atomic`]). (Domain vocabulary like `Kind` lives in `crate::model`.)

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Wall-clock `HH:MM:SS` (UTC) for log timestamps — avoids a chrono dependency.
pub fn now_hms() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        % 86_400;
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
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
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
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

/// Turn a file/dir name into a valid, unique lower_snake SQL identifier.
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
    if base.chars().next().map_or(true, |c| c.is_ascii_digit()) {
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
            Some('k') | Some('m') | Some('g') | Some('t') | Some('b')
        )
}

/// A number with an optional duration unit (s/m/h).
pub fn is_duration(v: &str) -> bool {
    let (num, unit) = split_num_unit(v);
    if num.parse::<f64>().is_err() {
        return false;
    }
    unit.is_empty()
        || matches!(
            unit.chars().next().map(|c| c.to_ascii_lowercase()),
            Some('s') | Some('m') | Some('h')
        )
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
    sweep_temps_older_than(dir, TEMP_STALE_AGE)
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
        let age = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok());
        if age.is_some_and(|age| age >= min_age) {
            let _ = fs::remove_file(entry.path());
        }
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

    #[test]
    fn ints_group_by_thousands() {
        assert_eq!(fmt_int(0), "0");
        assert_eq!(fmt_int(999), "999");
        assert_eq!(fmt_int(1_000), "1,000");
        assert_eq!(fmt_int(48_213), "48,213");
        assert_eq!(fmt_int(2_413_118), "2,413,118");
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
    }
}
