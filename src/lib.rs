//! Shared HTTP plumbing for all `erp-*` adapter crates.
//!
//! Three things used to be copy-pasted across the six adapter `client.rs` files:
//!
//! 1. **Client construction** – `reqwest::Client::builder()…build()` with per-adapter
//!    headers but no shared timeout or `User-Agent`.
//! 2. **Response handling** – the success / error-body split and JSON deserialisation.
//! 3. **Error mapping** – `From<reqwest::Error> for AdapterError` (401/403 → `Auth`,
//!    everything else → `Http`), replacing the old `reqwest_to_adapter` free function.

mod client;
mod error;
mod headers;
mod response;

pub use client::{build_client, build_client_with_builder};

pub use headers::{bearer_headers, custom_auth_headers};
pub use response::handle_response;