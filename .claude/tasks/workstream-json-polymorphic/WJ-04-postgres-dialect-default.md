# WJ-04 · Should the SQL parser default to the postgres dialect?

**Workstream:** JSON · **Status:** ✅ (decided: **no**; the lexer coupling is fixed) · **Depends on:** WJ-01

## The decision

**The default stays `generic`.** `postgresql` costs more than it buys, and the cost is in ordinary
SQL rather than in exotica. The lexer/planner coupling that made this a task rather than a default
flip is **fixed**: `sql::lex` now takes the dialect, so a user who sets the key gets a whole
language service on the dialect they chose.

The survey is below. It was run as a program against sqlparser 0.62 rather than read off the
source, because the interesting differences are in *combinations* of flags.

## What actually differs (measured, sqlparser 0.62, `generic` vs `postgresql`)

| Surface | Result |
|---|---|
| No-arg `supports_*` flags | **59 of 136 differ** |
| `is_identifier_start` / `is_identifier_part` | differ on `#` `@` `£` `¥` `¢` |
| `is_delimited_identifier_start` | differs on `` ` `` |
| `is_custom_operator_part` | differs on 17 characters: `!` `#` `%` `&` `*` `+` `-` `/` `<` `=` `>` `?` `@` `^` `` ` `` `\|` `~` |
| `identifier_quote_style` | `None` vs `Some('"')` |
| `prec_value` | every level differs (different scale, same relative order) |
| `is_reserved_for_identifier` | one keyword: `INTERVAL` |
| The repo's own SQL (156 literals scraped from `strata-core`/`strata-freya`/`strata-agent`) | **every one parses identically** |

So nothing existing regresses — the answer is about what the *user* can type.

**What postgres gains:** `doc ? 'a'` (the JSON containment operator, the measurement that opened
this task), `a NOTNULL`, `5!`, `1_000` numeric underscores, geometric types (`point '(1,2)'`),
`f(a := 1)`, `ALTER COLUMN … USING`, `LISTEN`/`NOTIFY`, an INSERT table alias, `SELECT * AS alias`.

**What postgres loses, and why it settles it:**

1. **`WHERE a>-1` stops meaning what it says.** Postgres makes every operator character a
   custom-operator part, so the tokenizer takes `>-` as one operator:
   ```
   generic  →  SELECT a > -1 FROM t
   postgres →  SELECT a >- 1 FROM t      (BinaryOperator::Custom, which the planner cannot lower)
   ```
   Same for `a<>-1` and `a||-1`. This is a *tokenizer* divergence, so it is invisible to any
   grammar-level reasoning, and it breaks a predicate people write every day. (It is also not
   Postgres's own behaviour — real Postgres strips a trailing `+`/`-` from a multi-character
   operator; sqlparser doesn't implement that rule.)
2. **`SELECT * EXCEPT (a)` and `SELECT * EXCLUDE a` stop parsing** — and both are *DataFusion*
   features, handled in `datafusion-sql`'s `check_wildcard_options` (`opt_except` / `opt_exclude`).
   Also lost: `* REPLACE (…)`, `* RENAME (…)`, `* ILIKE '…'`.
3. **`SELECT a, FROM t` stops parsing.** Trailing commas in a projection are a mid-edit state the
   editor sees constantly; under postgres every one of them is a hard parse error rather than a
   valid statement.
4. Smaller: `SELECT * FROM (t)`, `WITH c (SELECT 1)`, `LIMIT 1, 2`, `{'a': 1}` map literals. And
   `SELECT * FROM VALUES (1)` **silently** re-parses as a call to a function named `VALUES`, which
   is worse than an error.

The one thing worth having, `?`, already has a spelling that works in every dialect —
`json_contains(doc, 'a')`, which is what the docs name. Trading three classes of ordinary SQL for
one operator's punctuation is not a trade worth making, and a user who wants it can still set the
key.

**Also worth recording:** the task text called the key `Kind::Text` "catalogued", but in DataFusion
54 `sql_parser.dialect` is a typed `Dialect` **enum** with 13 accepted names (sqlparser's
`dialect_from_str` knows 18 — `spark`, `oracle` and `teradata` are unreachable through DataFusion).
`ConfigOptions::set` rejects an unknown name, so a bad value never reaches the planner. The key's
`desc` now names DataFusion's actual 13 rather than a subset of them.

## What was built

The coupling, not the default.

- **`engine/sql/lex.rs` no longer hardcodes a dialect.** `lex(sql, dialect)` resolves the name the
  way the *planner* resolves it, so the two cannot drift: DataFusion's typed
  `config::Dialect::from_str` **first**, then sqlparser. The order matters and review caught it —
  the two vocabularies are different sizes. sqlparser's `dialect_from_str` knows `spark`,
  `sparksql`, `oracle` and `teradata`; `ConfigOptions` knows none of them and `set_config`
  **skips** a value it can't parse. Handing the raw config string to sqlparser alone would have
  lexed the editor as Spark while the planner sat on `generic` (`a#b` is one token under one and
  three under the other) — the exact split this task exists to close, reintroduced through the new
  UI-side resolution path. A new `lex::dialect(name)` is the one resolution site, and an unknown
  name falls back to `generic` — where a fresh engine also lands — rather than going blank,
  because `ConfigOptions::set` and `policy_verdicts` are already saying that fault out loud and an
  editor that stopped tokenising would hide the message instead of showing it.
- **`is_word_char` is gone.** It was the second hardcode, it had no caller, and its doc claimed a
  role (completion's word boundaries) that the editor's own `is_ident_char` actually plays.
  `COMPLETION_SPEC.md` records the gap; when a provider-supplied word predicate is built it will
  reach the dialect the same way `lex` does.
- **`validate` reads the dialect before it lexes** rather than after — reading it after the lex is
  literally how the two came apart.
- **The dialect rides on `sql::Catalog`**, the language service's one snapshot of engine state, so
  completion (which is reached synchronously from a keystroke and has no engine to ask) tokenises
  by the same rules. The editor tab's catalog effect now subscribes to the settings as well as the
  project, so changing the key **re-lexes**. It resolves the value from the config through
  `config::effective`, not off the engine: `use_engine_config` is a *sibling* effect on the same
  write, so asking the engine there would make the answer depend on which of the two Freya runs
  first.
- **`export::is_bare_word` takes the dialect too.** It validates `PARTITIONED BY` column names
  against "what the tokenizer reads as one word", and that answer is dialect-dependent —
  `region#eu` is one identifier under `generic` and three tokens under `postgresql`. Hardcoded, it
  would wave through a name that then emits a `COPY` the planner chokes on, which is the exact
  parser message the check exists to replace.
- `config::DIALECT_KEY` — the key is spelled once now that three layers read it.

Pinned by `lex::tests::{the_dialect_comes_from_the_engine, an_unknown_dialect_lexes_as_generic,
a_dialect_datafusion_refuses_never_reaches_the_tokenizer}` and
`export::tests::bare_words_are_judged_by_the_configured_dialect`.

One residual, recorded rather than fixed: `set_config` skips a value it cannot apply, so an engine
already running `postgresql` *stays* on it when the user then types an invalid name, while the
lexer falls back to `generic`. That is `set_config`'s general behaviour for every key, not
something the dialect path can close on its own, and the engine warns about it.

## Note: nothing is blocked
`json_contains` is the spelling that works in every dialect and is what the docs name. A user who
wants `?` sets `datafusion.sql_parser.dialect = postgresql` in Settings ▸ Engine and now gets a
lexer, completion and squiggles that agree with the planner about it.
