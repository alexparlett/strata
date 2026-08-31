//! Serve a project folder to an AI agent over MCP, with no window and no app.
//!
//! ```text
//! cargo run -p strata-agent --example headless_mcp -- /path/to/project
//! ```
//!
//! This is the whole of it: [`serve_stdio`] opens a plain `Engine` on the folder, replays the
//! project's registration pass over it, and serves the same tool vocabulary the in-app server
//! serves. One vocabulary, two deployments — the second one is a call, not a re-implementation.
//!
//! **stdout belongs to the transport.** Anything written to it that is not MCP framing is a
//! protocol error at the client. This example installs no `tracing` subscriber, so nothing is
//! emitted at all; a real host installs one and points it at **stderr** before this is reached.
//!
//! Point an MCP client at this command with the folder as its argument; `docs/MCP_CLIENTS.md`
//! has the configuration each client wants.

use std::path::PathBuf;
use std::process;

use strata_agent::serve_stdio;

fn main() {
    let Some(folder) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: headless_mcp <project folder>");
        process::exit(2);
    };

    // Blocking: it owns its runtime and returns when the client disconnects. A folder with no
    // project in it is refused rather than scaffolded — a server the user cannot see must not
    // create the files the app owns.
    if let Err(why) = serve_stdio(folder) {
        eprintln!("{why}");
        process::exit(1);
    }
}
