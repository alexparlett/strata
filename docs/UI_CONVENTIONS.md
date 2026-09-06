# UI conventions

Strata uses shared theme roles, typography components, form rows, and modal behaviour across its windows. Midnight uses neutral charcoal surfaces and Daylight uses neutral light surfaces. Both use blue for primary actions and selection. Hover, active, selected, and disabled elements have distinct theme values; keyboard focus uses its own border role.

Functional labels and metadata use readable secondary text. Disabled colours are reserved for unavailable controls. SQL and data retain monospace typography; form labels use the UI family and sentence case, preserving acronyms. Numeric grid cells align to the right. NULL uses a secondary text role rather than the row-number or disabled colour.

The catalog uses the same empty-state rule for every expanded group: “No tables”, “No views”, or “No saved queries”. Collapsing a group hides its note. A search with no matching objects shows “No matches” and “Clear filter”. Group creation actions open the existing table editor or SQL editor; creating a view starts a definition that the user executes through the normal statement pipeline.

Modals move focus inside on opening and restore it on dismissal. Tab navigation and explicit focus requests stay within the active modal. Escape dismisses a surface, subject to an inner control handling it first. Enter activates the focused control; a checkbox does not also confirm the dialog. An unhandled Enter can invoke a dialog's explicit confirmation action. The palette, record viewer, and cell viewer obey the same focus boundary.

Removing an external table is labelled “Remove table”. Deleting a managed table is labelled “Delete table and data”. The confirmation explains the corresponding engine-owned consequence and any dependent views. Load failures make “Try again” the primary action.

Settings is a draft until Apply. A provider configuration dialog uses “Done” to return its changes to that draft. Cancel discards the inner edit. The launcher and unavailable assistant provide direct actions to open a project or configure the assistant.

The table editor's location choices are Files, Data source, and Managed table. Source provider pickers use full registered names. Writable sources show their read-only control near the name. Required text fields show an inline error when an edited value is emptied; the footer still explains why Save is unavailable.

Export places a compact format selector and preview before its options. Short character and numeric options share a row with their labels. Chart mark selection uses compact text tiles; axes use the caption typography role. Activity rails share their dimensions from the spacing constants.

The Freya fork supplies the framework's modal boundary. Workspace dependencies pin its commit directly; Strata does not vendor Freya core. Native title-bar decoration is applied only on macOS so the same components can be tested headlessly on Linux.
