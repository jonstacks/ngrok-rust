use std::{
    sync::atomic::{
        AtomicBool,
        Ordering,
    },
    task::{
        Context,
        Poll,
    },
};

use tokio::sync::mpsc;

use crate::frame::MAX_PAYLOAD_LENGTH;

struct FlowWindowState {
    inc_rx: mpsc::UnboundedReceiver<u32>,
    available: i64,
}

impl FlowWindowState {
    /// Deduct up to `max_bytes` (capped at MAX_PAYLOAD_LENGTH) from
    /// the available window and return the amount taken.
    fn deduct(&mut self, max_bytes: usize) -> usize {
        let cap = max_bytes.min(MAX_PAYLOAD_LENGTH as usize) as i64;
        let amount = cap.min(self.available) as usize;
        self.available -= amount as i64;
        amount
    }
}

/// Flow control window for outbound writes.
///
/// Tracks how many bytes the remote peer has authorized us to send.
/// Uses an mpsc channel internally to provide poll-friendly acquire
/// semantics without requiring stored futures or unsafe code.
pub struct FlowWindow {
    inc_tx: mpsc::UnboundedSender<u32>,
    state: std::sync::Mutex<FlowWindowState>,
    closed: AtomicBool,
}

impl FlowWindow {
    /// Create a new flow window with the given initial size.
    pub fn new(initial_size: u32) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            inc_tx: tx,
            state: std::sync::Mutex::new(FlowWindowState {
                inc_rx: rx,
                available: initial_size as i64,
            }),
            closed: AtomicBool::new(false),
        }
    }

    /// Add permits to the window. Called by the session when a WNDINC
    /// frame arrives from the remote peer.
    pub fn add_permits(&self, n: u32) {
        let _ = self.inc_tx.send(n);
    }

    /// Poll to acquire up to `max_bytes` from the flow window.
    ///
    /// Returns `Ok(n)` where `1 <= n <= min(max_bytes, MAX_PAYLOAD_LENGTH)`.
    /// Returns `Err(())` if the window is closed (stream reset or session dead).
    /// Returns `Poll::Pending` if no permits are available yet.
    pub fn poll_acquire(&self, cx: &mut Context<'_>, max_bytes: usize) -> Poll<Result<usize, ()>> {
        if self.closed.load(Ordering::Acquire) {
            return Poll::Ready(Err(()));
        }

        let mut state = self.state.lock().unwrap();

        // Drain pending increments
        while let Ok(inc) = state.inc_rx.try_recv() {
            state.available += inc as i64;
        }

        if state.available > 0 {
            return Poll::Ready(Ok(state.deduct(max_bytes)));
        }

        // No permits available — wait for an increment
        match state.inc_rx.poll_recv(cx) {
            Poll::Ready(Some(inc)) => {
                state.available += inc as i64;
                if state.available > 0 {
                    Poll::Ready(Ok(state.deduct(max_bytes)))
                } else {
                    // Zero increment (e.g. close sentinel) — re-check
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            }
            Poll::Ready(None) => Poll::Ready(Err(())),
            Poll::Pending => {
                if self.closed.load(Ordering::Acquire) {
                    Poll::Ready(Err(()))
                } else {
                    Poll::Pending
                }
            }
        }
    }

    /// Close the flow window. Wakes any pending poll_acquire calls.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        // Send dummy to wake any waiting poll_acquire
        let _ = self.inc_tx.send(0);
    }
}

#[cfg(test)]
mod tests {
    use std::future::poll_fn;

    use super::*;

    #[tokio::test]
    async fn acquire_within_initial_window() {
        let w = FlowWindow::new(1024);
        let n = poll_fn(|cx| w.poll_acquire(cx, 100)).await.unwrap();
        assert_eq!(n, 100);
    }

    #[tokio::test]
    async fn acquire_capped_at_available() {
        let w = FlowWindow::new(50);
        let n = poll_fn(|cx| w.poll_acquire(cx, 100)).await.unwrap();
        assert_eq!(n, 50);
    }

    #[tokio::test]
    async fn acquire_capped_at_max_payload() {
        let w = FlowWindow::new(u32::MAX);
        let n = poll_fn(|cx| w.poll_acquire(cx, usize::MAX)).await.unwrap();
        assert_eq!(n, MAX_PAYLOAD_LENGTH as usize);
    }

    #[tokio::test]
    async fn acquire_blocks_then_resumes_on_permit() {
        let w = std::sync::Arc::new(FlowWindow::new(0));
        let w2 = w.clone();

        let handle =
            tokio::spawn(async move { poll_fn(|cx| w2.poll_acquire(cx, 100)).await.unwrap() });

        tokio::task::yield_now().await;
        w.add_permits(75);

        let n = handle.await.unwrap();
        assert_eq!(n, 75);
    }

    #[tokio::test]
    async fn multiple_acquires_drain_window() {
        let w = FlowWindow::new(100);
        let n1 = poll_fn(|cx| w.poll_acquire(cx, 60)).await.unwrap();
        assert_eq!(n1, 60);
        let n2 = poll_fn(|cx| w.poll_acquire(cx, 60)).await.unwrap();
        assert_eq!(n2, 40);
    }

    #[tokio::test]
    async fn close_unblocks_pending_acquire() {
        let w = std::sync::Arc::new(FlowWindow::new(0));
        let w2 = w.clone();

        let handle = tokio::spawn(async move { poll_fn(|cx| w2.poll_acquire(cx, 100)).await });

        tokio::task::yield_now().await;
        w.close();

        let result = handle.await.unwrap();
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn add_permits_after_partial_drain() {
        let w = FlowWindow::new(10);
        let n = poll_fn(|cx| w.poll_acquire(cx, 10)).await.unwrap();
        assert_eq!(n, 10);

        w.add_permits(20);
        let n = poll_fn(|cx| w.poll_acquire(cx, 100)).await.unwrap();
        assert_eq!(n, 20);
    }
}
