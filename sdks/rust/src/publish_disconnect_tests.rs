//! Real socket regressions for publish after transport failure.
use super::*;
use acton_reactive::ipc::protocol::read_frame;
use tokio::net::{UnixListener, UnixStream};
use tokio::time::{Duration, timeout};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

async fn connected_pair(
    path: &std::path::Path,
) -> std::result::Result<(IpcClient, UnixStream), Box<dyn std::error::Error>> {
    let listener = UnixListener::bind(path)?;
    let client = IpcClient::connect(path).await?;
    let (server, _) = listener.accept().await?;
    Ok((client, server))
}

#[tokio::test]
async fn source_rejects_publishes_after_eof_and_accounts_for_queued_backlog() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (client, mut server) = connected_pair(&dir.path().join("source.sock")).await?;
    let source = EmergentSource::with_client("test-source", client);
    for _ in 0..5 {
        source.publish(EmergentMessage::new("test.event")).await?;
    }
    // The first request is in flight when the peer disappears without replying.
    timeout(Duration::from_secs(5), read_frame(&mut server, 1_048_576)).await??;
    drop(server);
    timeout(Duration::from_secs(5), source.watcher.flush()).await?;
    assert_eq!(
        source.publish_stats(),
        PublishStats {
            accepted: 0,
            rejected: 0,
            unanswered: 5
        }
    );
    for _ in 0..3 {
        assert!(matches!(
            source.publish(EmergentMessage::new("test.event")).await,
            Err(ClientError::ConnectionFailed(_))
        ));
    }
    assert_eq!(source.publish_stats().unanswered, 5);
    source.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn handler_clones_reject_publishes_after_eof() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (client, mut server) = connected_pair(&dir.path().join("handler.sock")).await?;
    let handler = EmergentHandler::with_client("test-handler", client);
    let clone = handler.clone();
    handler.publish(EmergentMessage::new("test.event")).await?;
    timeout(Duration::from_secs(5), read_frame(&mut server, 1_048_576)).await??;
    drop(server);
    timeout(Duration::from_secs(5), handler.watcher.flush()).await?;
    assert_eq!(handler.publish_stats().unanswered, 1);
    for publisher in [&handler, &clone] {
        assert!(matches!(
            publisher.publish(EmergentMessage::new("test.event")).await,
            Err(ClientError::ConnectionFailed(_))
        ));
    }
    assert_eq!(clone.publish_stats().unanswered, 1);
    handler.disconnect().await?;
    Ok(())
}
