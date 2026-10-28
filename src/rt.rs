//! One Tokio runtime for every Home Assistant and desktop-service request, and
//! a way to hand results back to the GTK thread.
use std::{future::Future, sync::OnceLock};

pub fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("nyx-home-io")
            .enable_all()
            .build()
            .expect("Tokio runtime")
    })
}

/// Run `work` off the GTK thread, then `done` with its result on the GTK thread.
pub fn spawn<T: Send + 'static>(
    work: impl Future<Output = T> + Send + 'static,
    done: impl FnOnce(T) + 'static,
) {
    let (tx, rx) = async_channel::bounded(1);
    runtime().spawn(async move {
        let _ = tx.send(work.await).await;
    });
    gtk::glib::spawn_future_local(async move {
        if let Ok(value) = rx.recv().await {
            done(value);
        }
    });
}

/// Run blocking `work` (a keyring prompt, a child process) off the GTK thread.
pub fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(T) + 'static,
) {
    spawn(
        async move {
            tokio::task::spawn_blocking(work)
                .await
                .expect("blocking task")
        },
        done,
    );
}
