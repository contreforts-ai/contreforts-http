//! Shared HTTP plumbing for all `erp-*` adapter crates.
//!
//! Three things used to be copy-pasted across the six adapter `client.rs` files:
//!
//! 1. **Client construction** – `reqwest::Client::builder()…build()` with per-adapter
//!    headers but no shared timeout or `User-Agent`.
//! 2. **Response handling** – the success / error-body split and JSON deserialisation.
//! 3. **Error mapping** – a `reqwest::Error → AdapterError` function that maps
//!    `401 / 403` to `AdapterError::Auth` and everything else to `AdapterError::Http`.
//!
//! This crate provides one canonical implementation of each.

use reqwest::{header, Client, Response, StatusCode};
use serde_json::Value;
use tracing::{debug, error};

use erp_core::AdapterError;

const USER_AGENT: &str = concat!("erp-sync/", env!("CARGO_PKG_VERSION"));

/// Default request timeout (30 s).  Individual adapters may override this by
/// supplying their own [`reqwest::ClientBuilder`] and calling
/// [`build_client_with_builder`] instead of [`build_client`].
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Build a [`reqwest::Client`] with:
/// * the shared `User-Agent` (`erp-sync/<version>`),
/// * a 30-second timeout,
/// * whatever `default_headers` the caller supplies (e.g. `Authorization`, `Accept`).
///
/// All six adapter clients call this instead of rolling their own builder.
///
/// ```rust,ignore
/// // In an adapter's client.rs:
/// let mut headers = header::HeaderMap::new();
/// headers.insert(header::AUTHORIZATION, HeaderValue::from_str(&token)?);
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

/// Consume a [`Response`] and return either the JSON body on success or a
/// mapped [`AdapterError`] on failure.
///
/// Structured `tracing` fields emitted on every call:
/// * **success** – `url`, `status` at `DEBUG`
/// * **auth error** – `method`, `url`, `status`, `body` at `ERROR`
/// * **other error** – `method`, `url`, `status`, `body` at `ERROR`
///
/// HTTP semantics:
/// | Status | `AdapterError` variant |
/// |--------|------------------------|
/// | 401, 403 | `Auth` |
/// | 404 | `NotFound { kind: method, id: url }` |
/// | anything else ≥ 400 | `Http("<method> <status> - <body> for URL: <url>")` |
pub async fn handle_response(
    method: &str,
    url: &str,
    resp: Response,
) -> Result<Value, AdapterError> {
    let status = resp.status();

    if status.is_success() {
        debug!(%method, %url, %status, "HTTP request successful");
        return resp.json().await.map_err(reqwest_to_adapter);
    }

    let code = status.as_u16();
    let body = resp.text().await.unwrap_or_default();

    if code == 401 || code == 403 {
        error!(%method, %url, %status, %body, "HTTP auth error");
        return Err(AdapterError::Auth);
    }

    if code == 404 {
        error!(%method, %url, %status, %body, "HTTP not-found");
        return Err(AdapterError::NotFound {
            kind: method.into(),
            id: url.into(),
        });
    }

    error!(%method, %url, %status, %body, "HTTP error");
    Err(AdapterError::Http(format!(
        "{method} {status} - {body} for URL: {url}"
    )))
}

/// Map a [`reqwest::Error`] to an [`AdapterError`].
///
/// `401` / `403` responses embedded in the error (e.g. when `reqwest` itself
/// surfaces a status) become `AdapterError::Auth`; all other errors become
/// `AdapterError::Http`.
///
/// This replaces the six identical `reqwest_err` free functions that previously
/// lived in each adapter's `client.rs`.
pub fn reqwest_to_adapter(e: reqwest::Error) -> AdapterError {
    if e.status()
        .map(|s| s.as_u16() == 401 || s.as_u16() == 403)
        .unwrap_or(false)
    {
        AdapterError::Auth
    } else {
        AdapterError::Http(e.to_string())
    }
}

/// Convenience: build a [`header::HeaderMap`] with `Authorization: Bearer <token>`,
/// `Accept: application/json`, and `Content-Type: application/json`.
///
/// Used by Pennylane, ERPNext, Forgejo, and GitLab – adapters whose only
/// per-request auth is a static bearer / token header.
pub fn bearer_headers(token: &str) -> Result<header::HeaderMap, AdapterError> {
    let mut headers = header::HeaderMap::new();
    let auth_value = format!("Bearer {token}");
    headers.insert(
        header::AUTHORIZATION,
        header::HeaderValue::from_str(&auth_value)
            .map_err(|e| AdapterError::Config(e.to_string()))?,
    );
    headers.insert(
        header::ACCEPT,
        header::HeaderValue::from_static("application/json"),
    );
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    Ok(headers)
}

/// Convenience: build a [`header::HeaderMap`] with a custom auth header name.
///
/// Used by GitLab (`PRIVATE-TOKEN`) and Forgejo (`Authorization: token …`).
pub fn custom_auth_headers(
    auth_header_name: &str,
    auth_header_value: &str,
) -> Result<header::HeaderMap, AdapterError> {
    let mut headers = header::HeaderMap::new();
    headers.insert(
        header::HeaderName::from_bytes(auth_header_name.as_bytes())
            .map_err(|e| AdapterError::Config(e.to_string()))?,
        header::HeaderValue::from_str(auth_header_value)
            .map_err(|e| AdapterError::Config(e.to_string()))?,
    );
    headers.insert(
        header::ACCEPT,
        header::HeaderValue::from_static("application/json"),
    );
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    Ok(headers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_client_succeeds_with_empty_headers() {
        let client = build_client(header::HeaderMap::new());
        assert!(client.is_ok());
    }

    #[test]
    fn build_client_succeeds_with_auth_header() {
        let headers = bearer_headers("tok-abc123").unwrap();
        let client = build_client(headers);
        assert!(client.is_ok());
    }

    // We can't easily fabricate a `reqwest::Error` with a known status without
    // an actual HTTP round-trip, so these tests use handle_response directly via
    // the wiremock tests below.  The unit test here simply verifies the happy
    // path of the function signature.

    #[tokio::test]
    async fn handle_response_200_returns_json() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "id": 1 })),
            )
            .mount(&server)
            .await;

        let resp = reqwest::Client::new()
            .get(format!("{}/test", server.uri()))
            .send()
            .await
            .unwrap();
        let val = handle_response("GET", "/test", resp).await.unwrap();
        assert_eq!(val["id"], 1);
    }

    #[tokio::test]
    async fn handle_response_401_maps_to_auth() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let resp = reqwest::Client::new()
            .get(format!("{}/test", server.uri()))
            .send()
            .await
            .unwrap();
        match handle_response("GET", "/test", resp).await {
            Err(AdapterError::Auth) => {}
            other => panic!("expected Auth, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_response_403_maps_to_auth() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;

        let resp = reqwest::Client::new()
            .get(format!("{}/test", server.uri()))
            .send()
            .await
            .unwrap();
        match handle_response("GET", "/test", resp).await {
            Err(AdapterError::Auth) => {}
            other => panic!("expected Auth, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_response_404_maps_to_not_found() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let resp = reqwest::Client::new()
            .get(format!("{}/test", server.uri()))
            .send()
            .await
            .unwrap();
        match handle_response("GET", "/test", resp).await {
            Err(AdapterError::NotFound { kind, id }) => {
                assert_eq!(kind, "GET");
                assert!(id.contains("/test"));
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_response_500_maps_to_http_error() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500).set_body_string("oops"))
            .mount(&server)
            .await;

        let resp = reqwest::Client::new()
            .post(format!("{}/boom", server.uri()))
            .send()
            .await
            .unwrap();
        match handle_response("POST", "/boom", resp).await {
            Err(AdapterError::Http(msg)) => {
                assert!(msg.contains("500"));
                assert!(msg.contains("oops"));
            }
            other => panic!("expected Http, got {other:?}"),
        }
    }

    #[test]
    fn bearer_headers_sets_authorization() {
        let h = bearer_headers("my-secret").unwrap();
        assert_eq!(
            h.get(header::AUTHORIZATION).unwrap().to_str().unwrap(),
            "Bearer my-secret"
        );
        assert!(h.contains_key(header::ACCEPT));
        assert!(h.contains_key(header::CONTENT_TYPE));
    }

    #[test]
    fn custom_auth_headers_sets_named_header() {
        let h = custom_auth_headers("PRIVATE-TOKEN", "glpat-xyz").unwrap();
        assert_eq!(
            h.get("private-token").unwrap().to_str().unwrap(),
            "glpat-xyz"
        );
    }
}