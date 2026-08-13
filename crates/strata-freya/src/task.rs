//! **Blocking work, off the render thread.**
//!
//! Freya is one event loop driving every window and its [`spawn`](freya::prelude::spawn) polls
//! futures on that very thread, so wrapping a synchronous call in an `async` block moves nothing. A
//! blocking `std::fs` read is not a slow frame, it is the whole app — and an SMB or NFS mount that
//! stopped answering blocks in the kernel with no timeout and no way to interrupt it.
//!
//! [`offload`] is the one way across: the work runs on a thread of its own and the caller awaits
//! the answer, which reaches the UI executor through the same cross-thread wake `async_io::Timer`
//! already uses for the autosave debounce.
//!
//! **A thread per call, not a shared worker.** A pool would serialize the calls, so one wedged
//! mount would hold up the next project's open — the very failure this removes, moved one step
//! along. These calls are rare, so a thread each is the cheaper answer as well as the safer one.
//!
//! **And not the engine's runtime**, which would happily host blocking work but is only ever built
//! *after* a project has loaded — the loads being exactly what this is for.
//!
//! **Cancelling is dropping the answer, not stopping the work.** A blocking syscall cannot be
//! interrupted, so a caller that gives up drops the receiver and the thread finishes into nothing.
//! The cost is one parked thread per attempt against a mount that never answers.

use std::future::Future;
use std::thread::Builder;

use futures::channel::oneshot;

/// Run `work` off the render thread and await its result.
///
/// `None` means the work never answered: the thread could not be started, or it panicked.
/// Neither is a fact about whatever the work was reading, so a caller reporting this must not
/// claim to know why. (In release Freya installs a panic hook that ends the process, so the
/// panic arm mostly belongs to debug builds.)
pub fn offload<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> impl Future<Output = Option<T>> {
    let (tx, rx) = oneshot::channel();
    let spawned = Builder::new()
        .name("strata-offload".to_owned())
        .spawn(move || {
            let _ = tx.send(work());
        });
    if let Err(e) = spawned {
        tracing::error!("could not start a worker thread: {e}");
    }
    async move { rx.await.ok() }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use futures::executor::block_on;

    use super::*;

    /// The whole point, asserted rather than assumed: the work does not run on the thread that
    /// asked for it, and its value still comes back.
    #[test]
    fn the_work_runs_on_another_thread_and_still_answers() {
        let here = thread::current().id();

        let there = block_on(offload(move || thread::current().id())).expect("answered");

        assert_ne!(here, there, "the work ran on the caller's thread");
    }

    /// **Dropping the wait does not stop the work**, which is the cost named in the module doc
    /// rather than a leak to fix: a blocking syscall cannot be interrupted, so a caller that
    /// gives up can only decline the answer. Pinned as a test because the opposite belief —
    /// that dropping the future cancels the read — is what would make a deadline look free.
    #[test]
    fn dropping_the_wait_leaves_the_work_running() {
        let (ran, observed) = mpsc::channel();

        let waiting = offload(move || {
            let _ = ran.send(());
            7
        });
        drop(waiting);

        observed
            .recv_timeout(Duration::from_secs(5))
            .expect("the work ran even though nobody was waiting for it");
    }
}
