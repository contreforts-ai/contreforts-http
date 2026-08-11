# contreforts-http

The HTTP client policy every adapter shares: one `reqwest::Client` shape, the common auth
header forms, and one response reader that maps status codes to `ConnectorError`.

```rust
let headers = contreforts_http::bearer_headers(&config.token)?;
let http = contreforts_http::build_client(headers)?;
let resp = http.get(&url).send().await?;
let page: Page = contreforts_http::handle_response("GET", &url, resp).await?;
```

Small on purpose. An adapter that needs something else builds its own client from
`build_client_with_builder`; what lives here is what every adapter would otherwise
re-implement slightly differently.

## What `handle_response` guarantees

| status | result |
|---|---|
| 2xx | the body, deserialised into `T` |
| 401, 403 | `ConnectorError::Auth` |
| 404 | `ConnectorError::NotFound { kind: method, id: url }` |
| anything else ≥ 400 | `ConnectorError::Http` with the status and the body |

Two of those guarantees are security properties rather than conveniences, and both were
defects before they were guarantees.

**The body is bounded.** `MAX_RESPONSE_BYTES` is 16 MiB, applied by streaming the body rather
than buffering it and checking afterwards — by then the memory is already spent. It **fails**
rather than truncating: a truncated JSON body is not a smaller answer, it is a parse error
attributed to the wrong cause. Error bodies are capped far harder (2 KiB), because they become
log lines. `#4`.

**Credentials in the URL never reach a log.** `handle_response` logs the URL on *every* call
and splices it into the error it returns, and `https://user:pw@proxy/v1` is an ordinary way to
reach an authenticated proxy. Every one of those sites goes through `redact_url_secrets`,
including `NotFound { id }` — the one an audit misses, because the URL sits in a struct field
rather than a format string. `#5`.

## `redact_url_secrets`

Public, because it is worth reusing and `contreforts-vecdb` does (it dropped its own copy in
that crate's `#47`). Two passes:

- **userinfo** — everything after the first `:` up to the last `@`. The *last*, deliberately: a
  password may contain `@`, and Azure Postgres usernames do.
- **query** — the value of `password`, `sslpassword` or `ssl-password`, matched exactly and
  case-insensitively. Exactly, because `password_file=/etc/pgpass` names a path a maintainer
  needs to see.

Where the last `@` is not in the authority, the **whole remainder** is withheld rather than
spliced back into something that still reads as a URL naming the wrong host. Over-redaction
loses debuggability; under-redaction loses the password, and a plausible-but-wrong redaction
loses both.

A URL with no credential is returned untouched — the property a leak-only test would not catch,
and one `src/redact.rs` asserts alongside a sweep of 600+ URL shapes.

## Known limit

`build_client_with_builder` applies its own `.timeout()` **after** the caller's builder, so a
timeout set on the builder handed in is replaced. Use `RequestBuilder::timeout` for a
per-request override. Making the client-level one configurable is a real change with a real
question behind it and has not been made.
