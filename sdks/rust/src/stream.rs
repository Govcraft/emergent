//! Message stream for receiving pushed messages.

use crate::message::EmergentMessage;
use futures::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::mpsc;

/// An async stream of messages received from subscriptions.
///
/// This is returned by `subscribe()` on `EmergentHandler` and `EmergentSink`.
/// Implements [`futures::Stream`] for use with [`futures::StreamExt`] combinators.
///
/// # Example
///
/// ```rust,ignore
/// use futures::StreamExt;
///
/// let mut stream = handler.subscribe(["timer.tick"]).await?;
///
/// // Basic iteration
/// while let Some(msg) = stream.next().await {
///     println!("Received: {}", msg.message_type);
/// }
///
/// // Or use StreamExt combinators
/// stream
///     .filter(|msg| futures::future::ready(msg.message_type.starts_with("timer.")))
///     .for_each(|msg| async move { println!("{:?}", msg) })
///     .await;
/// ```
pub struct MessageStream {
    /// The receiver channel for incoming messages.
    receiver: mpsc::Receiver<EmergentMessage>,
}

impl MessageStream {
    /// Create a new message stream from a receiver channel.
    pub(crate) fn new(receiver: mpsc::Receiver<EmergentMessage>) -> Self {
        Self { receiver }
    }

    /// Receive the next message from the stream.
    ///
    /// Returns `None` if the stream is closed.
    pub async fn next(&mut self) -> Option<EmergentMessage> {
        self.receiver.recv().await
    }

    /// Try to receive the next message without blocking.
    ///
    /// Returns `None` if no message is available.
    pub fn try_next(&mut self) -> Option<EmergentMessage> {
        self.receiver.try_recv().ok()
    }

    /// Close the stream.
    pub fn close(&mut self) {
        self.receiver.close();
    }
}

impl Stream for MessageStream {
    type Item = EmergentMessage;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.receiver).poll_recv(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_message_stream() -> Result<(), Box<dyn std::error::Error>> {
        let (tx, rx) = mpsc::channel(16);
        let mut stream = MessageStream::new(rx);

        // Send a message
        let msg = EmergentMessage::new("test.event").with_payload(json!({"key": "value"}));
        tx.send(msg).await?;

        // Receive the message
        let received = stream.next().await.ok_or("stream ended unexpectedly")?;
        assert_eq!(received.message_type, "test.event");

        // Close and verify stream ends
        drop(tx);
        assert!(stream.next().await.is_none());
        Ok(())
    }
}
