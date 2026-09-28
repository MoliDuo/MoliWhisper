//! One-shot check that the server accepts a set of credentials.
//!
//! An invalid session still completes the handshake; the server rejects it
//! right after (code 710022013 or close reason "2013"). So the check sends a
//! little silence plus the finish frame and waits for the verdict.

use std::time::Duration;

use super::protocol;
use super::transport::{ConnectOptions, connect};
use crate::asr::{AsrEvent, SAMPLE_RATE, ServerMsg};

const SILENCE: Duration = Duration::from_millis(300);
const READ_TIMEOUT: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The server answered normally.
    Accepted,
    /// The server refused the session.
    Rejected,
    /// Could not tell (network trouble, WAF, timeout). Not a reason to drop credentials.
    Inconclusive(String),
}

/// Opens a session with `opts` and tells whether the service accepts it.
pub async fn verify(opts: &ConnectOptions) -> Verdict {
    let (mut sink, mut stream, _) = match connect(opts).await {
        Ok(c) => c,
        Err(e) => return Verdict::Inconclusive(e.to_string()),
    };
    let samples = (SAMPLE_RATE as u128 * SILENCE.as_millis() / 1000) as usize;
    let sent = async {
        sink.audio(vec![0u8; samples * 2]).await?;
        sink.finish().await
    }
    .await;
    if let Err(e) = sent {
        // The server may already have closed on us; the stream says why.
        tracing::debug!("verify send: {e}");
    }

    let read = async {
        let mut answered = false;
        while let Some(event) = stream.next().await {
            match event {
                AsrEvent::Server(ServerMsg::Error { code, .. }) => {
                    if protocol::is_session_rejected(Some(code), None) {
                        return Verdict::Rejected;
                    }
                    return Verdict::Inconclusive(format!("server error code {code}"));
                }
                AsrEvent::Server(ServerMsg::Finish) => return Verdict::Accepted,
                AsrEvent::Server(_) => answered = true,
                AsrEvent::Garbage(_) => {}
                AsrEvent::Closed { code, reason, .. } => {
                    if protocol::is_session_rejected(None, Some(&reason)) {
                        return Verdict::Rejected;
                    }
                    if answered {
                        return Verdict::Accepted;
                    }
                    return Verdict::Inconclusive(format!(
                        "closed {code:?} {reason:?} without answer"
                    ));
                }
                AsrEvent::Failed(e) => {
                    if answered {
                        return Verdict::Accepted;
                    }
                    return Verdict::Inconclusive(e);
                }
            }
        }
        Verdict::Inconclusive("stream ended".into())
    };
    let verdict = tokio::time::timeout(READ_TIMEOUT, read)
        .await
        .unwrap_or_else(|_| Verdict::Inconclusive("no answer in time".into()));
    let _ = sink.close().await;
    verdict
}
