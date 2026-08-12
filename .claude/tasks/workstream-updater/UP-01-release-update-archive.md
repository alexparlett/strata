# UP-01 · Release-side: update archive + team identity

**Workstream:** Updater · **Status:** ✅ (built 2026-08-12; release v0.3.1 cut and verified) ·
**Depends on:** —

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
   pub const TEAM_ID: &str = "397J3SJ3D4";
   ```

   **Corrected during the build (2026-08-12).** This task shipped the value `FX37775A96`, which
   is the parenthesised suffix of the *Apple Development* certificate
   (`Apple Development: alexparlett@gmail.com (FX37775A96)`) — an individual's personal team,
   and the certificate `bundle-macos.sh` deliberately refuses to sign with because Apple will
   not notarize it. The Developer ID Application certificate that actually signs releases is
   `Developer ID Application: Alexander James Parlett (397J3SJ3D4)`, `organizationalUnitName`
   `397J3SJ3D4`, and that is the `TeamIdentifier` a distributed bundle carries. Step 4's
   cross-check is what caught it, on the first signed build, exactly as intended.

   It is an identity, not a credential: it is readable out of any distributed bundle's
   signature, so it belongs in source; only the private key can sign for it. Keep the exact
   single-line shape so a script can read it the way `bundle-macos.sh:116` reads `APP_ID`, and
   write the doc comment in that constant's house style: the one-line thesis, then why it lives
   here (it is the anchor the updater verifies a downloaded bundle against — UP-02).
4. **The cross-check.** When the script signs with a real identity, compare the team id against
   `TEAM_ID` read out of the source (same `sed` shape). A mismatch is a **hard fail**: an app
   signed by a team its own updater refuses is a release that can never update itself, and the
   build is the only place that can notice.

   Built reading the team back out of the **signature** (`codesign -dvvv`, `TeamIdentifier=`)
   rather than parsed out of the identity string, which this task asked for: `codesign` accepts
   a certificate hash as an identity, so the identity string need not contain the team at all,
   and the signature is the thing the updater will actually read. Same check, one fewer way to
   be silently unenforced.
5. **Docs.** `docs/RELEASING.md` gains the zip in its artifact description: what it is for, and
   that its `.app.zip` suffix is the contract UP-02's asset selection keys on (the version in
   the filename is informative; the tag is authoritative).

## Acceptance
- [x] A release run attaches both `Strata-<v>-universal.dmg` and `Strata-<v>-universal.app.zip`;
      a plain artifact-only run uploads both. *(Both globs widened; a local `--arch arm64` run
      leaves exactly one `.zip` and one `.dmg` in `target/dist`. The notarize submission's own zip
      moved out of `$DIST` to keep that true after a failed submission — under `set -e` a rejection
      aborts before the cleanup, and nothing empties `$DIST` between runs, so it would have ridden
      the next successful build onto the release page as a stale second asset.)*
- [x] The zip extracts via `ditto -x -k` to a bundle that passes
      `codesign --verify --deep --strict` **and** carries `TeamIdentifier=397J3SJ3D4` /
      `Identifier=com.alexparlett.strata` — the three facts UP-02 verifies. `xcrun stapler
      validate` is **unverified locally**: no notary credentials on this machine, so the build
      took the signed-but-not-notarized rung. Confirm it on the release cut below.
- [x] `TEAM_ID` exists beside `APP_ID`, single-line, sed-readable; a real signing identity whose
      team id disagrees with it fails the build with a message naming both. *(Both directions
      exercised: a real Developer ID build reports `team: 397J3SJ3D4` and proceeds; the same guard
      against `FX37775A96` exits non-zero naming both values.)*
- [x] `docs/RELEASING.md` describes the new asset and its naming contract.
- [x] **Cut a release after landing this.** v0.3.1 is published (2026-08-12) carrying both
      `Strata-0.3.1-universal.app.zip` and the DMG, and UP-02 drove the real updater against it:
      the archive downloads, `ditto -x -k` unpacks it, and the extracted bundle passes all three
      of UP-02's checks. `xcrun stapler validate` on that bundle now **confirmed** ("The validate
      action worked"), and `spctl -a -t install` accepts it as `source=Notarized Developer ID`,
      `Developer ID Application: Alexander James Parlett (397J3SJ3D4)` — so the notarized path
      ran and the ticket survives the zip round trip.

## Note for whoever runs the next local build

`scripts/bundle-macos.sh` fails at `lipo` in any worktree where `CARGO_TARGET_DIR` is set (it is,
on Alex's machine): cargo writes to the main checkout's `target/` and the script reads build output
through relative `target/…` paths. It fails *after* the full release compile. Not a CI problem (no
such var on the runners) and out of UP-01's scope, so it is filed separately rather than fixed
here — the `DIST` half is a real design call, since a shared target dir means two worktrees'
bundles collide in one `dist/`.

## References
- `scripts/bundle-macos.sh` — notarize zip at `:321-324`, staple at `:329`, DMG section from
  `:343`, the `APP_ID` sed at `:116`.
- `.github/workflows/release.yml:305,488` — the two asset globs.
- `crates/strata-core/src/secret.rs:68-75` — `APP_ID` and its doc style.
