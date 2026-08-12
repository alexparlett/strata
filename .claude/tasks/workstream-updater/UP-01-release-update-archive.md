# UP-01 · Release-side: update archive + team identity

**Workstream:** Updater · **Status:** ⬜ · **Depends on:** —

## Goal
Every release carries an artifact an updater can consume programmatically, and the app compiles
in the identity that updater will verify downloads against. Pure release-pipeline + one constant;
no app behaviour changes.

## Current state (verified 2026-08-12)
- `scripts/bundle-macos.sh` produces `Strata-$VERSION-$ARCH_MODE.dmg` and nothing else. It
  already `ditto`-zips the bundle once — for notarization (`bundle-macos.sh:321-324`) — and
  deletes that zip after the verdict.
- `.github/workflows/release.yml` publishes `target/dist/*.dmg` in exactly two places: the
  artifact upload (`release.yml:305`) and `gh release create` (`release.yml:488`).
- `strata_core::secret::APP_ID` (`crates/strata-core/src/secret.rs:68-75`) is the one
  Rust-side identity constant. The bundle script reads it with a `sed` pinned to the exact
  single-line `pub const APP_ID: &str = "…";` shape (`bundle-macos.sh:116`). There is **no**
  Rust-side team identifier anywhere — the team id appears only as the optional notary env var
  `AC_TEAM_ID` (`bundle-macos.sh:299,310`).
- Signing is now real: Developer ID + notarization secrets are configured (2026-08-12), so a
  release build takes the notarized path and staples the app (`bundle-macos.sh:329`).

## Build

1. **The update archive.** In `bundle-macos.sh`, produce
   `$DIST/Strata-$VERSION-$ARCH_MODE.app.zip` with `ditto -c -k --keepParent` — the same
   invocation as the notarize zip, because it is the only archiver that round-trips a signed
   bundle's symlinks and extended attributes. Create it **after** the notarize section (after
   `stapler staple`), so the archived bundle carries the ticket and validates offline; on an
   unsigned run it is created all the same, so the artifact set never changes shape. Report it
   in the Done section beside the DMG.
2. **Attach it.** Widen the two globs in `release.yml` (`:305`, `:488`) to include
   `target/dist/*.zip`. Nothing else in the workflow changes — the release-notes step and the
   naming machinery are version-driven, not asset-driven.
3. **`TEAM_ID`.** A constant beside `APP_ID` in `crates/strata-core/src/secret.rs`:

   ```rust
   pub const TEAM_ID: &str = "FX37775A96";
   ```

   (Alex's Apple team id, supplied 2026-08-12 — the parenthesised suffix of the signing
   identity. It is an identity, not a credential: it is readable out of any distributed
   bundle's signature, so it belongs in source; only the private key can sign for it.) Keep the
   exact single-line shape so a script can read it the way `bundle-macos.sh:116` reads
   `APP_ID`, and write the doc comment in that constant's house style: the one-line thesis,
   then why it lives here (it is the anchor the updater verifies a downloaded bundle against —
   UP-02). Step 4's cross-check confirms the value against the real signing identity on the
   first signed build, so a transcription slip cannot survive a release.
4. **The cross-check.** When the script signs with a real identity, extract the team id from
   the identity string and compare it against `TEAM_ID` read out of the source (same `sed`
   shape). A mismatch is a **hard fail**: an app signed by a team its own updater refuses is a
   release that can never update itself, and the build is the only place that can notice.
5. **Docs.** `docs/RELEASING.md` gains the zip in its artifact description: what it is for, and
   that its `.app.zip` suffix is the contract UP-02's asset selection keys on (the version in
   the filename is informative; the tag is authoritative).

## Acceptance
- [ ] A release run attaches both `Strata-<v>-universal.dmg` and `Strata-<v>-universal.app.zip`;
      a plain artifact-only run uploads both.
- [ ] The zip extracts via `ditto -x -k` to a bundle that passes
      `codesign --verify --deep --strict` and `xcrun stapler validate`.
- [ ] `TEAM_ID` exists beside `APP_ID`, single-line, sed-readable; a real signing identity whose
      team id disagrees with it fails the build with a message naming both.
- [ ] `docs/RELEASING.md` describes the new asset and its naming contract.
- [ ] **Cut a release after landing this** — UP-02's end-to-end verification needs a published
      release with the zip attached.

## References
- `scripts/bundle-macos.sh` — notarize zip at `:321-324`, staple at `:329`, DMG section from
  `:343`, the `APP_ID` sed at `:116`.
- `.github/workflows/release.yml:305,488` — the two asset globs.
- `crates/strata-core/src/secret.rs:68-75` — `APP_ID` and its doc style.
