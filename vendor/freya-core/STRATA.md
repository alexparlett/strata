# Freya core patch

Source: alexparlett/freya, Cargo.lock revision b717d10. The local Cargo patch keeps the focus fix reviewable and buildable without publishing a dependency commit. All Freya consumers use this same core.

The patch adds an accessibility modal boundary: navigation and explicit focus requests stay inside the topmost modal subtree. Strata handles initial focus and restoration through existing lifecycle and accessibility APIs. This is a framework change intended for the Freya fork; after it is incorporated and pinned, remove this vendor override.

Cargo.toml expands the upstream workspace dependency declarations for standalone use. Other source changes are limited to accessibility modal state, its builder, and focus traversal.
