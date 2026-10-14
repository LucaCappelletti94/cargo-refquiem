//! Harness pieces the refquiem fuzz targets share.

use std::panic::PanicHookInfo;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

type Job = Box<dyn FnOnce() + Send>;

/// Longest a target waits for the worker, past libFuzzer's own `-timeout`.
const REPLY_DEADLINE: Duration = Duration::from_secs(120);

/// One reused 1 GiB-stack thread, as `syn` leaves deep nesting to the stack (https://github.com/dtolnay/syn/issues/901).
static WORKER: LazyLock<Sender<Job>> = LazyLock::new(|| {
    let (jobs, queue) = mpsc::channel::<Job>();
    std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(move || queue.into_iter().for_each(|job| job()))
        .expect("spawn the fuzz worker");
    jobs
});

/// Run `f` on the worker and return its value.
pub fn on_large_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let (reply, answer) = mpsc::sync_channel(1);
    WORKER
        .send(Box::new(move || {
            // The receiver only goes away after its deadline, the panic below reports that.
            let _ = reply.send((id, f()));
        }))
        .expect("the fuzz worker is running");
    let (answered, value) = answer
        .recv_timeout(REPLY_DEADLINE)
        .expect("the fuzz worker answers within its deadline");
    assert_eq!(answered, id, "reply to request {answered}, expected {id}");
    value
}

/// Lets pulldown-cmark panics reach refquiem's `catch_unwind` (pulldown-cmark#1129) and aborts on every other panic.
pub fn tolerate_pulldown_cmark_panics() {
    let abort = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info: &PanicHookInfo<'_>| {
        let in_pulldown_cmark = info
            .location()
            .is_some_and(|location| location.file().contains("/pulldown-cmark-"));
        if !in_pulldown_cmark {
            abort(info);
        }
    }));
}
