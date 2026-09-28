//! Networking both clients share.

use std::io;
use std::net::SocketAddr;
use std::sync::Once;
use std::time::{Duration, Instant};

use tokio::net::TcpStream;
use tokio::task::JoinSet;
use tokio_tungstenite::tungstenite::error::UrlError;
use tokio_tungstenite::tungstenite::handshake::client::{Request, Response};
use tokio_tungstenite::tungstenite::{self};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// Delay before racing the next address family (RFC 8305 recommends 250 ms).
const HAPPY_EYEBALLS_DELAY: Duration = Duration::from_millis(250);

pub(crate) type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Makes rustls use ring. Call before any TLS.
pub(crate) fn install_crypto_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // Fails only if another provider is already installed, which is fine.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Opens a WebSocket: TCP to whichever address answers first, then TLS for
/// `wss` and the upgrade. A refused upgrade is [`tungstenite::Error::Http`].
pub(crate) async fn connect_websocket(
    request: Request,
) -> Result<(WsStream, Response), tungstenite::Error> {
    install_crypto_provider();
    let uri = request.uri();
    let host = uri
        .host()
        .ok_or(tungstenite::Error::Url(UrlError::NoHostName))?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_owned();
    let port = uri.port_u16().unwrap_or(if uri.scheme_str() == Some("ws") {
        80
    } else {
        443
    });

    let t = Instant::now();
    let tcp = tcp_connect(&host, port).await?;
    tracing::debug!(peer = ?tcp.peer_addr().ok(), ms = t.elapsed().as_millis(), "tcp connected");

    let t = Instant::now();
    let connected =
        tokio_tungstenite::client_async_tls_with_config(request, tcp, None, None).await?;
    tracing::debug!(
        ms = t.elapsed().as_millis(),
        "tls + websocket handshake done"
    );
    Ok(connected)
}

/// Happy-eyeballs TCP connect: try addresses alternating between IPv6 and
/// IPv4, starting a new attempt every 250 ms or as soon as one fails.
async fn tcp_connect(host: &str, port: u16) -> io::Result<TcpStream> {
    let t = Instant::now();
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port)).await?.collect();
    tracing::debug!(?addrs, ms = t.elapsed().as_millis(), "resolved");
    let mut pending = interleave_families(addrs).into_iter().peekable();
    let mut attempts = JoinSet::new();
    let mut last_err = None;

    loop {
        if let Some(addr) = pending.next() {
            attempts.spawn(async move { TcpStream::connect(addr).await });
        } else if attempts.is_empty() {
            return Err(
                last_err.unwrap_or_else(|| io::Error::other(format!("no addresses for {host}")))
            );
        }
        let more = pending.peek().is_some();
        tokio::select! {
            Some(joined) = attempts.join_next() => match joined {
                Ok(Ok(stream)) => {
                    attempts.abort_all();
                    stream.set_nodelay(true)?;
                    return Ok(stream);
                }
                Ok(Err(e)) => last_err = Some(e),
                Err(e) => last_err = Some(io::Error::other(e)),
            },
            _ = tokio::time::sleep(HAPPY_EYEBALLS_DELAY), if more => {}
        }
    }
}

/// Keeps the resolver's first family first, then alternates.
fn interleave_families(addrs: Vec<SocketAddr>) -> Vec<SocketAddr> {
    let Some(first) = addrs.first() else {
        return addrs;
    };
    let first_v6 = first.is_ipv6();
    let (mut a, mut b): (Vec<_>, Vec<_>) = addrs.into_iter().partition(|x| x.is_ipv6() == first_v6);
    let mut out = Vec::with_capacity(a.len() + b.len());
    a.reverse();
    b.reverse();
    while !a.is_empty() || !b.is_empty() {
        out.extend(a.pop());
        out.extend(b.pop());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interleaves_starting_with_first_family() {
        let v6a: SocketAddr = "[::1]:1".parse().unwrap();
        let v6b: SocketAddr = "[::2]:1".parse().unwrap();
        let v4a: SocketAddr = "1.1.1.1:1".parse().unwrap();
        let v4b: SocketAddr = "2.2.2.2:1".parse().unwrap();
        assert_eq!(
            interleave_families(vec![v4a, v4b, v6a, v6b]),
            vec![v4a, v6a, v4b, v6b]
        );
        assert_eq!(
            interleave_families(vec![v6a, v6b, v4a]),
            vec![v6a, v4a, v6b]
        );
        assert_eq!(interleave_families(vec![]), vec![]);
    }
}
