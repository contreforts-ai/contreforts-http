//! Error-mapping helpers for `reqwest` → [`erp_core::AdapterError`].
//!
//! The canonical conversion now lives in `erp-core` as
//! `impl From<reqwest::Error> for AdapterError` (enabled via the `reqwest`
//! feature flag on that crate). The free function below is kept for
//! backward-compatibility with any call sites not yet migrated to `?`.

use erp_core::AdapterError;

/// Map a [`reqwest::Error`] to an [`AdapterError`].
///
/// Prefer using `?` directly at call sites — `From<reqwest::Error> for
/// AdapterError` is implemented in `erp-core` (feature `reqwest`) and makes
/// this function redundant. It is kept as a thin shim for gradual migration.
///
/// `401` / `403` status codes become [`AdapterError::Auth`];
/// every other error becomes [`AdapterError::Http`].
pub fn reqwest_to_adapter(e: reqwest::Error) -> AdapterError {
    AdapterError::from(e)
}