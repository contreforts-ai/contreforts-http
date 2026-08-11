//! [`reqwest::Client`] construction shared by all adapters.

use reqwest::{header, Client};

use contreforts_core::ConnectorError;

const USER_AGENT: &str = concat!("erp-sync/", env!("CARGO_PKG_VERSION"));

/// Request timeout applied to every client this module builds (30 s).
///
/// **Not overridable through [`build_client_with_builder`]**, and this comment
/// used to say it was (`contreforts-http#4`): that function applies `.timeout()`
/// *after* the caller's builder, so a `.timeout()` set on the builder handed in
/// is silently replaced. Nothing has broken on it because the callers that set
/// a timeout do it per-request with `RequestBuilder::timeout`, which this does
/// not touch — but the comment told the next reader something false about the
/// `ClientBuilder` path, which is the kind of thing that is only discovered
/// after a request that should have had ten minutes was cut off at thirty
/// seconds.
///
/// Use `RequestBuilder::timeout` for a per-request override. Making the
/// client-level one configurable is a real change with a real question behind
/// it — whether a shared default should be overridable at all — and is not
/// this fix.
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Build a [`reqwest::Client`] with:
/// * the shared `User-Agent` (`erp-sync/<version>`),
/// * a 30-second timeout,
/// * whatever `default_headers` the caller supplies (e.g. `Authorization`, `Accept`).
///
/// All adapter clients call this instead of rolling their own builder.
///
/// ```rust,ignore
/// // In an adapter's client.rs:
/// let headers = contreforts_http::bearer_headers(&config.token)?;
/// let http = contreforts_http::build_client(headers)?;
/// ```
pub fn build_client(default_headers: header::HeaderMap) -> Result<Client, ConnectorError> {
    build_client_with_builder(Client::builder(), default_headers)
}

/// Same as [`build_client`] but lets the caller start from a custom
/// [`reqwest::ClientBuilder`] (e.g. to set certificate pinning or a proxy).
pub fn build_client_with_builder(
    builder: reqwest::ClientBuilder,
    default_headers: header::HeaderMap,
) -> Result<Client, ConnectorError> {
    builder
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS))
        .default_headers(default_headers)
        .build()
        .map_err(|e| ConnectorError::Http(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headers::bearer_headers;

    #[test]
    fn build_client_succeeds_with_empty_headers() {
        assert!(build_client(header::HeaderMap::new()).is_ok());
    }

    #[test]
    fn build_client_succeeds_with_auth_header() {
        let headers = bearer_headers("tok-abc123").unwrap();
        assert!(build_client(headers).is_ok());
    }
}
