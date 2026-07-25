//! The launcher's three regions (design `Launcher.dc.html`): the title bar, the branded
//! left rail, and the projects pane.

mod open;
mod projects;
mod rail;
mod row;
mod title_bar;

pub use open::pick_and_open;
pub use projects::ProjectsPane;
pub use rail::LauncherRail;
pub use title_bar::TitleBar;
