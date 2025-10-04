use tokio::sync::oneshot;

/// When a `DropGuard` is dropped, it signals to its paired receiver that it has been dropped.
/// This allows an external observer, maybe in another task, to know when the
/// `DropGuard` was dropped.
pub(crate) struct DropGuard {
    tx: Option<oneshot::Sender<()>>,
}

impl DropGuard {
    pub fn new() -> (DropGuard, oneshot::Receiver<()>) {
        let (tx, rx) = oneshot::channel();
        (DropGuard { tx: Some(tx) }, rx)
    }
}

impl Drop for DropGuard {
    fn drop(&mut self) {
        // needed because we can't move out of a &mut
        let tx = self
            .tx
            .take()
            .expect("this should never happen, it should always be Some");
        let _ = tx.send(());
    }
}
