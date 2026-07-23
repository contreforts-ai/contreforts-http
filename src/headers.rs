//! Convenience constructors for common [`reqwest::header::HeaderMap`] patterns.

use reqwest::header;

use contreforts_core::AdapterError;

/// Build a [`header::HeaderMap`] with:
/// * `Authorization: Bearer <token>`
/// * `Accept: application/json`
/// * `Content-Type: application/json`
///
/// Used by Pennylane, ERPNext, Forgejo, and GitLab — adapters whose only
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

/// Build a [`header::HeaderMap`] with a custom auth header name, plus the
/// standard `Accept` and `Content-Type` JSON headers.
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
