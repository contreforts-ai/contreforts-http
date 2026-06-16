use reqwest::Response;
use serde_json::Value;
use tracing::{debug, error};

use erp_core::AdapterError;

/// Consume a [`Response`] and return either the JSON body on success or a
/// mapped [`AdapterError`] on failure.
///
/// Structured `tracing` fields emitted on every call:
/// * **success** – `method`, `url`, `status` at `DEBUG`
/// * **auth error** – `method`, `url`, `status`, `body` at `ERROR`
/// * **not-found** – `method`, `url`, `status`, `body` at `ERROR`
/// * **other error** – `method`, `url`, `status`, `body` at `ERROR`
///
/// HTTP semantics:
///
/// | Status        | [`AdapterError`] variant                         |
/// |---------------|--------------------------------------------------|
/// | 2xx           | *(success — returns the JSON body)*              |
/// | 401, 403      | [`AdapterError::Auth`]                           |
/// | 404           | [`AdapterError::NotFound`] `{ kind: method, id: url }` |
/// | anything else ≥ 400 | [`AdapterError::Http`] `("<method> <status> - <body> for URL: <url>")` |
pub async fn handle_response(
    method: &str,
    url: &str,
    resp: Response,
) -> Result<Value, AdapterError> {
    let status = resp.status();

    if status.is_success() {
        debug!(%method, %url, %status, "HTTP request successful");
        return Ok(resp.json().await?);
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

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn handle_response_200_returns_json() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "id": 1 })),
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
}