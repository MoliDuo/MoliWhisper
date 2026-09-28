//! What can go wrong, over HTTP and on the socket.

use std::fmt;

use super::wire::{Response, status};

/// A failed call to one of the service's HTTP APIs.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    /// The reply lacks what it should carry.
    #[error("unexpected reply: {0}")]
    Malformed(&'static str),
    /// The service answered with an error code.
    #[error("service error {code}: {message}")]
    Service { code: i64, message: String },
}

/// Why connecting or a session failed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Kind {
    /// Socket or HTTP trouble; the credentials are not to blame.
    Network,
    Timeout,
    /// The service said no: a failure event or a refused handshake.
    Refused,
    /// The service cannot route sessions for this device id.
    Unroutable,
}

#[derive(Debug)]
pub(super) struct Failure {
    pub kind: Kind,
    pub msg: String,
}

impl Failure {
    pub fn network(e: impl fmt::Display) -> Self {
        Self {
            kind: Kind::Network,
            msg: e.to_string(),
        }
    }

    pub fn refused(e: impl fmt::Display) -> Self {
        Self {
            kind: Kind::Refused,
            msg: e.to_string(),
        }
    }

    pub fn timeout(msg: &str) -> Self {
        Self {
            kind: Kind::Timeout,
            msg: msg.to_owned(),
        }
    }

    /// A failure event from the service.
    pub fn event(resp: &Response) -> Self {
        Self {
            kind: if resp.status_code == status::UNROUTABLE {
                Kind::Unroutable
            } else {
                Kind::Refused
            },
            msg: format!(
                "{}: [{}] {}",
                resp.event, resp.status_code, resp.status_text
            ),
        }
    }

    /// Whether the credentials may be to blame, so are better fetched again.
    pub fn blames_credentials(&self) -> bool {
        matches!(self.kind, Kind::Refused | Kind::Unroutable)
    }
}

impl From<ApiError> for Failure {
    fn from(e: ApiError) -> Self {
        match e {
            ApiError::Http(_) => Self::network(e),
            _ => Self::refused(e),
        }
    }
}
