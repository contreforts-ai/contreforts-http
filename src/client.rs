//! [`reqwest::Client`] construction shared by all adapters.

use reqwest::{header, Client};

use erp_core::AdapterError;

const USER_AGENT: &str = concat!("erp-sync/", env!("CARGO_PKG_VERSION"));

/// Default request timeout (30 s). Individual adapters may override this by
/// supplying their own [`reqwest::ClientBuilder`] and calling
/// [`build_client_with_builder`] instead of [`build_client`].
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
/// let headers = erp_http::bearer_headers(&config.token)?;
/// let http = erp_http::build_client(headers)?;
/// ```
pub fn build_client(default_headers: header::HeaderMap) -> Result<Client, AdapterError> {
    build_client_with_builder(Client::builder(), default_headers)
}

/// Same as [`build_client`] but lets the caller start from a custom
/// [`reqwest::ClientBuilder`] (e.g. to set certificate pinning or a proxy).
pub fn build_client_with_builder(
    builder: reqwest::ClientBuilder,
    default_headers: header::HeaderMap,
) -> Result<Client, AdapterError> {
    builder
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS))
        .default_headers(default_headers)
        .build()
        .map_err(|e| AdapterError::Http(e.to_string()))
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
