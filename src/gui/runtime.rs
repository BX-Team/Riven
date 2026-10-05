use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::Runtime;
use tokio::sync::oneshot;

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

/// Runs a future that is not `Send` (lighty's launch) on a thread of its own, built there.
pub fn pinned<T, F, Fut>(make: F) -> oneshot::Receiver<T>
where
    T: Send + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = T>,
{
    let (tx, rx) = oneshot::channel();
    let handle = runtime().handle().clone();
    runtime().spawn_blocking(move || {
        let _ = tx.send(handle.block_on(make()));
    });
    rx
}
