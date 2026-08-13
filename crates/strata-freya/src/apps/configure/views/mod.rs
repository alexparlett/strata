//! The Configure window's views: the title bar, the scrolling body, and the footer.
//!
//! The body's order is the canvas's, and one thing in it contradicts `DEV_TASKS` D7: the busy and
//! failure blocks are the **last** things in the body, after Hive, not "below import-options,
//! above Hive". The canvas is newer; it wins.

mod columns;
mod footer;
mod hive;
mod identity;
mod location;
mod options;
mod paths;
mod status;
mod title_bar;

use std::time::Duration;

use async_io::Timer;
use freya::prelude::*;
use freya::radio::use_radio;

pub use footer::Footer;
pub use title_bar::TitleBar;

use crate::apps::configure::views::columns::Columns;
use crate::apps::configure::views::hive::Hive;
use crate::apps::configure::views::identity::Identity;
use crate::apps::configure::views::location::{Location, ObjectStore};
use crate::apps::configure::views::options::ImportOptions;
use crate::apps::configure::views::paths::SourcePaths;
use crate::apps::configure::views::status::StatusBlock;
use crate::apps::configure::{ConfigureCtx, Status};
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::{ProjChan, ProjectState, Reg};
use crate::components::form::Form;
use crate::components::metrics::SP_5;

/// The window body's inset (canvas `padding: var(--sp-5)`). The gap *between* sections is the
/// form's own `ROW_GAP` — this body is a [`Form`], so it does not get to invent one.
const BODY_PADDING: Gaps = Gaps::new(SP_5, SP_5, SP_5, SP_5);

/// Everything between the title bar and the footer, scrolling as one.
#[derive(PartialEq)]
pub struct ConfigureBody;

impl Component for ConfigureBody {
    fn render(&self) -> impl IntoElement {
        // The per-row type probe behind an internal table's COLUMNS list (IT-01). Here rather
        // than at the window root because it is this form's own reconciliation — over the whole
        // draft, not a hook per row — and because a driver the body owns is one the body's tests
        // get for nothing.
        use_probes(&use_consume::<EngineCtx>(), use_consume::<ConfigureCtx>());

        rect().width(Size::fill()).height(Size::flex(1.)).child(
            ScrollView::new()
                .width(Size::fill())
                .height(Size::fill())
                .child(
                    // **A `Form`.** Every section here is a `Row` (or a pair of them), so the
                    // rhythm between them is the shared form's, and the register is set once at
                    // the top rather than assumed by each section. A `rect()` with a spacing of
                    // its own would be this window quietly keeping its own copy of both.
                    rect().width(Size::fill()).padding(BODY_PADDING).child(
                        Form::new()
                            // The canvas's order: where the files are, then what the table is
                            // called, then — on a remote table — which store.
                            //
                            // `ObjectStore` and the two below it are **always mounted** and draw
                            // nothing when they have nothing to say (`views::hive`'s rule, for
                            // the differ). The cost is that an invisible row still takes a
                            // `Form` gap either side, so the local layout carries a doubled gap
                            // where the store row would be. Removing it means keying every child
                            // here — the shape `apps::connection::views::form` uses — which is a
                            // change to five components that have no key today.
                            .child(Location)
                            .child(Identity)
                            .child(ObjectStore)
                            // A internal table's own section, on the same terms as the three
                            // below it: always mounted, drawing nothing when LOCATION is not on
                            // Internal. It sits where SOURCE PATHS does because it answers the
                            // same question — what is in this table — and the two are never
                            // both shown.
                            .child(Columns)
                            .child(SourcePaths)
                            .child(ImportOptions)
                            .child(Hive)
                            .child(StatusBlock),
                    ),
                ),
        )
    }
}

/// Watch the catalog row this window is waiting on, and settle the save.
///
/// The registration itself belongs to the project window's one scan driver — this is a
/// **reconciliation over the shared store**, not an await on a second registration path. While
/// the status is `Registering(name)`, the row's `Reg` is the answer: `Ready` means the table is
/// registered and this window's work is done, `Failed` brings the engine's own message back to
/// the footer. `Loading` is simply "not yet".
///
/// The status gate is what makes this safe to mount unconditionally: an existing table's row is
/// already `Ready` when the window opens, and without the gate that would close the window at
/// mount.
pub fn use_watch_registration(mut ctx: ConfigureCtx) {
    // Subscribes to the tables channel — the only thing this window watches over there.
    let project = use_radio::<ProjectState, ProjChan>(ProjChan::Tables);
    let platform = use_hook(Platform::get);

    use_side_effect(move || {
        let Status::Registering(name) = ctx.status.read().clone() else {
            return;
        };
        let answer = project.read().tables.iter().find_map(|row| {
            ProjectState::same_name(&row.def.name, &name).then(|| match &row.reg {
                Reg::Loading => None,
                Reg::Ready(_) => Some(Ok(())),
                Reg::Failed(why) => Some(Err(why.clone())),
            })
        });
        match answer.flatten() {
            // Still registering, or the row went while we waited (a drop from the catalog) —
            // either way there is nothing for this window to say.
            None => {}
            Some(Ok(())) => platform.close_current_window(),
            Some(Err(why)) => ctx.status.set(Status::Failed(why)),
        }
    });
}

/// How long the column list must sit still before its unanswered types are probed. A typing
/// burst therefore asks once, about what the user stopped at, rather than once per prefix.
const PROBE_DEBOUNCE: Duration = Duration::from_millis(300);

/// Keep the planner's verdicts in step with the column types the draft holds (IT-01) — a
/// reconciliation over the whole draft, on the project window's validation driver's shape.
///
/// The work list is [`ConfigureDraft::unprobed`], a **projection** rather than a queue: a row
/// retyped mid-pass simply changes what is pending, and a spelling two rows share is one
/// question. A pass waits for that list to stop moving across a whole [`PROBE_DEBOUNCE`] before
/// it asks anything, so a burst of keystrokes probes what the user settled on rather than every
/// prefix on the way there.
///
/// The answers are cached for the window's life and never evicted: they are a pure function of
/// the text on this session, and re-asking about a spelling typed a moment ago is the one thing
/// that would make the form feel slow.
fn use_probes(engine: &EngineCtx, ctx: ConfigureCtx) {
    let mut probing = use_state(|| false);
    let engine = engine.clone();

    use_side_effect(move || {
        // Subscribes to both, which is what re-arms the driver when a box is typed into or an
        // answer lands — and it has to happen **before** any early return, because
        // `ReactiveContext::run` drains this effect's subscriptions on every pass and only the
        // reads it actually performs put them back. Guarding on `probing` first would leave a
        // wake during a pass with no subscriptions at all, and the driver would never run again.
        if ctx.draft.read().unprobed(&ctx.probes.read()).is_empty() || *probing.peek() {
            return;
        }
        probing.set(true);
        let engine = engine.clone();
        spawn(async move {
            let _guard = Probing(probing);
            loop {
                let before = ctx.draft.peek().unprobed(&ctx.probes.peek());
                if before.is_empty() {
                    break;
                }
                Timer::after(PROBE_DEBOUNCE).await;
                // Still moving: wait it out from here rather than asking about a half-typed
                // spelling that is about to be replaced.
                if ctx.draft.peek().unprobed(&ctx.probes.peek()) != before {
                    continue;
                }
                // One at a time, like the validation drain: each is a plan on the engine's own
                // runtime, and a settled form with a dozen rows should not queue a dozen of them
                // ahead of whatever the user runs next.
                for typed in before {
                    let answer = engine.column_type(typed.clone()).await;
                    let mut probes = ctx.probes;
                    probes.write().insert(typed, answer);
                }
            }
        });
    });
}

/// Marks a probe pass as running for as long as it is alive — cleared on `Drop`, so a pass that
/// is *cancelled* clears it too and the driver can arm again. A latched flag would mean nothing
/// was ever probed after the first interruption.
///
/// **It asks before it writes**: the way a pass ends most often is this window closing under it,
/// at which point the state is freed and writing one panics (`State::is_alive`). There is nothing
/// to clear then — the driver is going with it.
struct Probing(State<bool>);

impl Drop for Probing {
    fn drop(&mut self) {
        if self.0.is_alive() {
            self.0.set(false);
        }
    }
}
