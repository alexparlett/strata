# P6-03 · Platform seam for OS shims

**Phase:** 6 · **Status:** ⬜ · **DEV_TASKS:** F5 · **Depends on:** —

## Goal
Any remaining OS-specific bits sit behind one platform trait with per-OS impls (macOS-first, but the
seam is explicit).

## Current state
Freya removes most of the Dioxus objc shims (native events + `winit` close). Whatever's left (titlebar
insets, traffic-light chrome, reveal-in-Finder, etc.) should be behind a seam, not scattered `#[cfg]`s.

## Build
- Consolidate the residual platform calls behind a `platform` trait + per-OS impls; non-mac gets real
  (not silently no-op) fallbacks where feasible.

## Acceptance
- [ ] OS-specific calls go through one seam; non-mac builds don't silently no-op critical paths.

## Freya / references
- Plan §3 (`platform/`), DEV_TASKS F5. `winit` window APIs.
