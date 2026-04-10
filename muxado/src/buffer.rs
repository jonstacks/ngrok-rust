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

use bytes::Bytes;
use tokio::sync::mpsc;

/// A shared inbound byte buffer backed by an unbounded mpsc channel.
///
/// Designed to be embedded in `Arc<StreamInner>` so both the session
/// (push side) and stream (read side) can access it via `&self`.
pub struct InboundBuffer {
    tx: mpsc::UnboundedSender<Bytes>,
    rx: std::sync::Mutex<mpsc::UnboundedReceiver<Bytes>>,
    closed: AtomicBool,
}

impl InboundBuffer {
    /// Create a new inbound buffer.
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            tx,
            rx: std::sync::Mutex::new(rx),
            closed: AtomicBool::new(false),
        }
    }

    /// Push data into the buffer. Called by the session reader task.
    pub fn push(&self, data: Bytes) {
        if data.is_empty() {
            return;
        }
        let _ = self.tx.send(data);
    }

    /// Close the buffer, waking any blocked readers.
    /// Called on stream reset or EOF.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        // Send empty sentinel to wake any blocked poll_pop
        let _ = self.tx.send(Bytes::new());
    }

    /// Returns true if the buffer has been closed.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Poll for the next chunk of data. Returns:
    /// - `Ready(Some(bytes))` when data is available
    /// - `Ready(None)` on close/EOF
    /// - `Pending` when no data is available yet
    pub fn poll_pop(&self, cx: &mut Context<'_>) -> Poll<Option<Bytes>> {
        let mut rx = self.rx.lock().unwrap();
        match rx.poll_recv(cx) {
            Poll::Ready(Some(bytes)) => {
                if bytes.is_empty() {
                    // Sentinel — check if closed
                    if self.closed.load(Ordering::Acquire) {
                        Poll::Ready(None)
                    } else {
                        // Spurious empty chunk, re-poll
                        cx.waker().wake_by_ref();
                        Poll::Pending
                    }
                } else {
                    Poll::Ready(Some(bytes))
                }
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => {
                if self.closed.load(Ordering::Acquire) {
                    Poll::Ready(None)
                } else {
                    Poll::Pending
                }
            }
        }
    }

    /// Async pop — convenience wrapper around poll_pop.
    pub async fn pop(&self) -> Option<Bytes> {
        std::future::poll_fn(|cx| self.poll_pop(cx)).await
    }
}

impl Default for InboundBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::future::poll_fn;

    use super::*;

    #[tokio::test]
    async fn push_and_pop() {
        let buf = InboundBuffer::new();
        buf.push(Bytes::from("hello"));
        let chunk = buf.pop().await.unwrap();
        assert_eq!(chunk, Bytes::from("hello"));
    }

    #[tokio::test]
    async fn push_empty_is_ignored() {
        let buf = InboundBuffer::new();
        buf.push(Bytes::new());
        buf.close();
        let result = buf.pop().await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn multiple_chunks() {
        let buf = InboundBuffer::new();
        buf.push(Bytes::from("abc"));
        buf.push(Bytes::from("def"));
        assert_eq!(buf.pop().await.unwrap(), Bytes::from("abc"));
        assert_eq!(buf.pop().await.unwrap(), Bytes::from("def"));
    }

    #[tokio::test]
    async fn close_returns_none() {
        let buf = InboundBuffer::new();
        buf.push(Bytes::from("data"));
        buf.close();
        // Should still get buffered data first
        assert_eq!(buf.pop().await.unwrap(), Bytes::from("data"));
        // Then None
        assert!(buf.pop().await.is_none());
    }

    #[tokio::test]
    async fn pop_wakes_on_push() {
        let buf = std::sync::Arc::new(InboundBuffer::new());
        let buf2 = buf.clone();

        let handle = tokio::spawn(async move { buf2.pop().await.unwrap() });

        tokio::task::yield_now().await;
        buf.push(Bytes::from("delayed"));

        let chunk = handle.await.unwrap();
        assert_eq!(chunk, Bytes::from("delayed"));
    }

    #[tokio::test]
    async fn pop_wakes_on_close() {
        let buf = std::sync::Arc::new(InboundBuffer::new());
        let buf2 = buf.clone();

        let handle = tokio::spawn(async move { buf2.pop().await });

        tokio::task::yield_now().await;
        buf.close();

        let result = handle.await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn poll_pop_returns_pending_when_empty() {
        let buf = InboundBuffer::new();
        let result = poll_fn(|cx| {
            let poll = buf.poll_pop(cx);
            // Should be pending since buffer is empty and not closed
            Poll::Ready(poll.is_pending())
        })
        .await;
        assert!(result);
    }
}
