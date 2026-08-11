use futures::StreamExt as _;
use reqwest::Response;
use serde::de::DeserializeOwned;
use tracing::{debug, error};

use contreforts_core::ConnectorError;

use crate::redact::redact_url_secrets;

/// How much of a response body this crate will hold in memory (#4).
///
/// `resp.json::<T>()` applies no cap and neither does `reqwest` by default, so
/// a hostile or merely broken endpoint could return a body large enough to
/// exhaust memory and no caller had a way to prevent it. Every consumer of this
/// crate inherited that, and connectors talk to third-party services by
/// definition.
///
/// **16 MiB**, chosen against the bodies this crate actually reads: connector
/// pages (an ERPNext or Forgejo list response), embedding vectors, and a Chroma
/// `/get` page. `contreforts-rag#9` bounds its *requests* to 16 texts per call;
/// at 4 096 f32 dimensions rendered as JSON that is a low-single-digit-megabyte
/// reply, so this leaves an order of magnitude of headroom over the largest
/// legitimate body known here.
///
/// Deliberately a constant and not a parameter. A per-call bound would be a
/// knob every caller has to reason about, and the failure it guards against is
/// one no caller is better placed to size than this crate; a consumer that
/// genuinely needs more should raise this and say why, where the reasoning is
/// visible to everyone else.
pub const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// How much of an **error** body reaches the message.
///
/// The error path reads the body to put it in `ConnectorError::Http`, and that
/// string is logged and propagated through `anyhow` context. A 16 MiB error
/// page in a log line is its own denial of service, so it is truncated far
/// harder than the success path — an HTML error page's first 2 KiB says which
/// proxy refused you, which is the whole diagnostic value it has.
const MAX_ERROR_BODY_BYTES: usize = 2 * 1024;

/// Read at most `limit` bytes of `resp`'s body, failing rather than truncating.
///
/// Streamed rather than `resp.bytes()` with a length check afterwards: by then
/// the body is already in memory, which is the thing being prevented.
/// `Content-Length` is not trusted either — it is absent under chunked transfer
/// encoding and it can simply be wrong.
///
/// **Fails rather than truncating**, because a truncated JSON body is not a
/// smaller answer, it is a parse error attributed to the wrong cause. The
/// caller is told the bound and the method, so "raise the cap" and "this
/// endpoint is broken" are distinguishable.
async fn read_bounded(
    method: &str,
    url: &str,
    resp: Response,
    limit: usize,
) -> Result<Vec<u8>, ConnectorError> {
    let mut body = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| ConnectorError::Http(e.to_string()))?;
        if body.len() + chunk.len() > limit {
            return Err(ConnectorError::Http(format!(
                "{method} {}: response body exceeds {limit} bytes and was not read. \
                 Raising the limit is a change to contreforts-http::MAX_RESPONSE_BYTES, \
                 deliberately not a per-call knob.",
                redact_url_secrets(url)
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Consume a [`Response`] and return either the deserialised body on success or a
/// mapped [`ConnectorError`] on failure.
///
/// The type parameter `T` must implement [`DeserializeOwned`]; the response body
/// is deserialised directly into `T` on 2xx, so callers no longer need to
/// re-deserialise a [`serde_json::Value`] themselves.
///
/// Structured `tracing` fields emitted on every call:
/// * **success** – `method`, `url`, `status` at `DEBUG`
/// * **auth error** – `method`, `url`, `status`, `body` at `ERROR`
/// * **not-found** – `method`, `url`, `status`, `body` at `ERROR`
/// * **other error** – `method`, `url`, `status`, `body` at `ERROR`
///
/// HTTP semantics:
///
/// | Status              | [`ConnectorError`] variant                                            |
/// |---------------------|---------------------------------------------------------------------|
/// | 2xx                 | *(success — returns the deserialised body)*                        |
/// | 401, 403            | [`ConnectorError::Auth`]                                             |
/// | 404                 | [`ConnectorError::NotFound`] `{ kind: method, id: url }`            |
/// | anything else ≥ 400 | [`ConnectorError::Http`] `("<method> <status> - <body> for URL: <url>")` |
pub async fn handle_response<T: DeserializeOwned>(
    method: &str,
    url: &str,
    resp: Response,
) -> Result<T, ConnectorError> {
    let status = resp.status();
    // #5: every `url` below is this, never the argument. A credential in a URL
    // is an ordinary way to reach an authenticated proxy, and this function
    // logs the URL on *every* call — success included — as well as splicing it
    // into the error it returns, which then travels through `anyhow` context
    // into whatever the caller logs.
    let url = redact_url_secrets(url);

    if status.is_success() {
        debug!(%method, %url, %status, "HTTP request successful");
        // #4: bounded, and the deserialisation happens after the read rather
        // than inside it, so the cap applies whatever `T` is.
        let body = read_bounded(method, &url, resp, MAX_RESPONSE_BYTES).await?;
        return serde_json::from_slice(&body).map_err(|e| {
            ConnectorError::Http(format!(
                "{method} {url}: response body is not valid JSON: {e}"
            ))
        });
    }

    let code = status.as_u16();
    // Truncated hard, and lossily on purpose: this becomes a log line and an
    // error message, not a value anything parses.
    let raw = read_bounded(method, &url, resp, MAX_ERROR_BODY_BYTES)
        .await
        .unwrap_or_else(|_| b"<error body exceeded the log bound and was not read>".to_vec());
    let body = String::from_utf8_lossy(&raw);

    if code == 401 || code == 403 {
        error!(%method, %url, %status, %body, "HTTP auth error");
        return Err(ConnectorError::Auth);
    }

    if code == 404 {
        error!(%method, %url, %status, %body, "HTTP not-found");
        return Err(ConnectorError::NotFound {
            kind: method.into(),
            // The redacted form here too: this `id` is rendered into
            // `ConnectorError`'s `Display` and reaches the same logs.
            id: url.into_owned(),
        });
    }

    error!(%method, %url, %status, %body, "HTTP error");
    Err(ConnectorError::Http(format!(
        "{method} {status} - {body} for URL: {url}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn handle_response_200_returns_json() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "id": 1 })))
            .mount(&server)
            .await;

        let resp = reqwest::Client::new()
            .get(format!("{}/test", server.uri()))
            .send()
            .await
            .unwrap();
        let val: Value = handle_response("GET", "/test", resp).await.unwrap();
        assert_eq!(val["id"], 1);
    }

    #[tokio::test]
    async fn handle_response_200_deserialises_into_struct() {
        #[derive(serde::Deserialize, Debug, PartialEq)]
        struct Payload {
            id: u32,
            name: String,
        }

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "id": 42, "name": "acme" })),
            )
            .mount(&server)
            .await;

        let resp = reqwest::Client::new()
            .get(format!("{}/test", server.uri()))
            .send()
            .await
            .unwrap();
        let p: Payload = handle_response("GET", "/test", resp).await.unwrap();
        assert_eq!(
            p,
            Payload {
                id: 42,
                name: "acme".into()
            }
        );
    }

    #[tokio::test]
    async fn handle_response_401_maps_to_auth() {
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
        match handle_response::<Value>("GET", "/test", resp).await {
            Err(ConnectorError::Auth) => {}
            other => panic!("expected Auth, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_response_403_maps_to_auth() {
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
        match handle_response::<Value>("GET", "/test", resp).await {
            Err(ConnectorError::Auth) => {}
            other => panic!("expected Auth, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_response_404_maps_to_not_found() {
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
        match handle_response::<Value>("GET", "/test", resp).await {
            Err(ConnectorError::NotFound { kind, id }) => {
                assert_eq!(kind, "GET");
                assert!(id.contains("/test"));
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    /// **A credential in the URL never reaches the error, on any path** (#5).
    ///
    /// Asserted over the rendered `ConnectorError` rather than by inspecting a
    /// log subscriber: the error is what travels through `anyhow` context into
    /// whatever the caller finally logs, so it is the surface that matters and
    /// the one a future edit is most likely to reintroduce the URL into.
    ///
    /// All four arms, because each builds its message differently — and the
    /// `404` arm is the one that would have been missed: it puts the URL in
    /// `NotFound { id }`, not in a format string.
    #[tokio::test]
    async fn no_arm_lets_a_credential_in_the_url_reach_the_error() {
        const SECRET: &str = "hunter2";

        for status in [200u16, 401, 404, 500] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(status).set_body_string("not json"))
                .mount(&server)
                .await;

            // The credential sits in the URL that is *reported*, not in the one
            // dialled -- a real proxy URL would carry both, and wiremock is not
            // one. This function takes the reported url as its own argument, so
            // that is exactly the surface under test.
            let reported = format!("https://user:{SECRET}@proxy/v1/embeddings");
            let resp = reqwest::Client::new()
                .get(format!("{}/test", server.uri()))
                .send()
                .await
                .unwrap();

            let err = handle_response::<Value>("GET", &reported, resp)
                .await
                .expect_err("a non-JSON body fails on every status here");
            let rendered = format!("{err}");
            assert!(
                !rendered.contains(SECRET),
                "status {status}: the credential reached the error: {rendered}"
            );
        }
    }

    /// **A body past the cap is refused, not truncated** (#4).
    ///
    /// Refused rather than truncated because a truncated JSON body is not a
    /// smaller answer, it is a parse error attributed to the wrong cause.
    #[tokio::test]
    async fn a_response_body_past_the_cap_is_refused() {
        let server = MockServer::start().await;
        // Comfortably past MAX_RESPONSE_BYTES, and valid JSON, so the refusal
        // cannot be mistaken for a deserialisation failure.
        let huge = format!("\"{}\"", "x".repeat(MAX_RESPONSE_BYTES + 1024));
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(huge))
            .mount(&server)
            .await;

        let resp = reqwest::Client::new()
            .get(format!("{}/test", server.uri()))
            .send()
            .await
            .unwrap();
        let err = handle_response::<Value>("GET", "/test", resp)
            .await
            .expect_err("a body past the cap must not be read");
        let rendered = format!("{err}");
        assert!(
            rendered.contains("exceeds") && rendered.contains(&MAX_RESPONSE_BYTES.to_string()),
            "the refusal has to name the bound so a caller can tell it from a broken \
             endpoint: {rendered}"
        );
    }

    /// A body just under the cap still goes through, so the guard is a cap and
    /// not a wall.
    ///
    /// The half that would be missing if only the refusal were tested: a
    /// `read_bounded` that refused everything would pass that test.
    #[tokio::test]
    async fn a_large_body_under_the_cap_is_still_read() {
        let server = MockServer::start().await;
        let big = "y".repeat(MAX_RESPONSE_BYTES / 2);
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(format!("\"{big}\"")))
            .mount(&server)
            .await;

        let resp = reqwest::Client::new()
            .get(format!("{}/test", server.uri()))
            .send()
            .await
            .unwrap();
        let value: Value = handle_response("GET", "/test", resp)
            .await
            .expect("under the cap");
        assert_eq!(value.as_str().map(str::len), Some(MAX_RESPONSE_BYTES / 2));
    }

    #[tokio::test]
    async fn handle_response_500_maps_to_http_error() {
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
        match handle_response::<Value>("POST", "/boom", resp).await {
            Err(ConnectorError::Http(msg)) => {
                assert!(msg.contains("500"));
                assert!(msg.contains("oops"));
            }
            other => panic!("expected Http, got {other:?}"),
        }
    }
}
