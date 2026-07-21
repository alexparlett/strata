# P2-21 · Tabs & split — remaining nits

**Phase:** 2 — Workbench · **Status:** 🟡 · **Depends on:** —

## Goal
Close the small gaps on the otherwise-complete tab strip + workbench split.

## Current state
`tab_bar/{bar,tab,controls,drag,menu}` is built (open/close/rename/context-menu/drag/overflow/dot→×).

## Build
- **Overflow menu:** disable **Reopen closed** when nothing is closed (TODO in `controls.rs`).

## Acceptance
- [ ] Reopen-closed is disabled with an empty closed-stack; the split is a Freya resizable container.

## Freya / references
- `tab_bar/controls.rs`, Freya `ResizableContainer`. (OS-close intercept moved to P2-20.)
