use std::future::Future;
use std::sync::{Arc, OnceLock};

use tokio::runtime::Runtime;
use tokio::sync::{Notify, oneshot};

/// The tokio runtime network and disk work runs on; gpui's executor drives only the UI.
fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("riven-io")
            .build()
            .expect("tokio runtime starts")
    })
}

/// Runs `fut` on tokio; await the receiver from a gpui task.
pub fn spawn<T, F>(fut: F) -> oneshot::Receiver<T>
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
{
    let (tx, rx) = oneshot::channel();
    runtime().spawn(async move {
        let _ = tx.send(fut.await);
    });
    rx
}

/// Runs blocking work (hashing, directory scans) on tokio's blocking pool.
pub fn blocking<T, F>(work: F) -> oneshot::Receiver<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = oneshot::channel();
    runtime().spawn_blocking(move || {
        let _ = tx.send(work());
    });
    rx
}

/// Stops the work it was handed out with; the receiver then reports an error.
#[derive(Clone, Default)]
pub struct Abort(Arc<Notify>);

impl Abort {
    pub fn abort(&self) {
        self.0.notify_one();
    }

    async fn race<T>(&self, work: impl Future<Output = T>) -> Option<T> {
        tokio::select! {
            value = work => Some(value),
            () = self.0.notified() => None,
        }
    }
}

/// [`spawn`] that can be stopped halfway; dropping the future cancels its downloads.
pub fn spawn_abortable<T, F>(fut: F) -> (oneshot::Receiver<T>, Abort)
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
{
    let (tx, rx) = oneshot::channel();
    let abort = Abort::default();
    let stop = abort.clone();
    runtime().spawn(async move {
        if let Some(value) = stop.race(fut).await {
            let _ = tx.send(value);
        }
    });
    (rx, abort)
}

/// Runs a future rustc cannot prove `Send` (pack updates) on a thread of its own, built there.
pub fn pinned_abortable<T, F, Fut>(make: F) -> (oneshot::Receiver<T>, Abort)
where
    T: Send + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = T>,
{
    let (tx, rx) = oneshot::channel();
    let abort = Abort::default();
    let stop = abort.clone();
    let handle = runtime().handle().clone();
    runtime().spawn_blocking(move || {
        if let Some(value) = handle.block_on(stop.race(make())) {
            let _ = tx.send(value);
        }
    });
    (rx, abort)
}
