//! Redacting credentials out of a URL before it is logged.
//!
//! `contreforts-http#5`: `handle_response` logs the request URL on **every**
//! call and splices it into the error it returns. A credential in a URL is an
//! ordinary way to reach an authenticated proxy —
//! `https://user:pw@proxy/v1/embeddings`, which in `contreforts-rag` comes
//! straight out of the `EMBEDDING_URL` environment variable — and nothing
//! between that variable and the log line removed it.
//!
//! # Ported from `contreforts-vecdb::secret`, and it belongs here
//!
//! That crate wrote this first, with its edge cases measured and pinned, and
//! kept it `pub(crate)`. Re-deriving the rules here would be the second
//! independent implementation of a security control, which is
//! `contreforts-vecdb#45`'s complaint about `VectorStoreColumnType` in another
//! costume: two copies, no crate seeing both, nothing able to check they agree.
//!
//! So it moves **upstream** rather than sideways. `contreforts-vecdb` already
//! depends on this crate (it added the dependency for the Chroma backend,
//! `contreforts-vecdb#7`), so it can drop its private copy and call this —
//! filed as `contreforts-vecdb#47`. The dependency runs the right way round for
//! that; it would not have, the other way.
//!
//! The rules and their reasoning below are that crate's, carried over
//! deliberately unchanged so the two cannot diverge in the window before it
//! adopts this.

use std::borrow::Cow;

/// Placeholder substituted for a secret in every formatted representation.
pub const REDACTED: &str = "<redacted>";

/// Query-string keys whose value is a password.
///
/// `password` is documented libpq URI syntax and `sqlx-postgres` honours it
/// (`"password" => options = options.password(&value)` in its query-pair loop),
/// so `…/db?password=hunter2` is a working connection string carrying a live
/// credential. `sslpassword` is the libpq passphrase for the client TLS key;
/// sqlx 0.8 does not read it, but it is a secret in the same string and the
/// next sqlx release may well pick it up. The dashed spelling mirrors the
/// aliases sqlx already accepts for its other `ssl*` parameters.
///
/// Matching is exact on the key and ASCII-case-insensitive: over-redacting a
/// spelling the server would ignore costs one log line, under-redacting costs
/// the password. Exact, because a key that merely *contains* `password` —
/// `password_file=/etc/pgpass` — names a path, not a secret, and that path is
/// what a maintainer needs to see.
const QUERY_PASSWORD_KEYS: &[&str] = &["password", "sslpassword", "ssl-password"];

/// Rewrites a connection URL so that the passwords in it are replaced by
/// [`REDACTED`], leaving everything a maintainer needs in order to debug a
/// connection failure — scheme, username, host, port, path, the other query
/// parameters — intact.
///
/// A PostgreSQL URL carries a password in two places, and both are redacted:
/// the userinfo (`postgres://user:pw@host`) and the query string
/// (`postgres://user@host/db?password=pw`). Anything else — a `service=` name
/// resolved from a file, a parameter this crate has never heard of — is not a
/// password as far as this scan is concerned and survives verbatim.
///
/// The input is an arbitrary caller-supplied string, not necessarily a valid
/// URL, so the scan is purely lexical, allocates only when it has something to
/// redact, and cannot panic: every index comes from `find`/`rfind` on an ASCII
/// needle or from `str::split`, so it is always a `char` boundary.
///
/// # The userinfo pass
///
/// * the scheme is the text before the **first** `"://"`, and only when that
///   text is a syntactically valid scheme (`ALPHA *( ALPHA / DIGIT / "+" / "-"
///   / "." )`). Taking the first `"://"` unconditionally, as this used to, let
///   `appuser:secret@db.internal/db?cb=https://x` through in clear: the scan
///   jumped past the `'@'` to the *callback's* scheme separator and then found
///   no userinfo to redact. The validity check is strictly stronger than
///   requiring the separator to precede the last `'@'` — a valid scheme cannot
///   contain `'@'`, so a `"://"` that follows the last `'@'` has that `'@'` in
///   its prefix and is rejected by construction;
/// * a string with no scheme is scanned in full, so a bare `user:password@host`
///   is redacted too;
/// * the userinfo is taken to end at the **last** `'@'` of the remainder.
///   Deliberately the last, not the last one before the first `'/'`: a password
///   may contain `'@'` or an unescaped `'/'`, and bounding the scan at the
///   authority would let such a password through unredacted. It is also simply
///   right for Azure Postgres, whose username contains an `'@'`
///   (`postgres://adminuser@myserver:pw@host/db`);
/// * inside the userinfo, everything after the first `':'` is the password and
///   is replaced wholesale, so a password containing `':'` cannot leak a
///   fragment of itself;
/// * a userinfo with no `':'` is a bare username, carries no password, and is
///   kept as it is — whatever else is going on in the string.
///
/// # Over-redaction, and why it is shaped the way it is
///
/// Taking the last `'@'` means a URL with no userinfo but an `'@'` further on
/// (an address in a query parameter, say) is over-redacted. The direction of
/// that trade is chosen: over-redaction loses debuggability, under-redaction
/// loses the password.
///
/// What is *not* acceptable is over-redacting into something that still reads
/// as a URL. Redacting `postgres://db.internal:5432/contreforts?user=admin@myserver`
/// as `postgres://db.internal:<redacted>@myserver` yields a well-formed
/// redacted URL whose host, port and database are all wrong, and someone
/// debugging a connection failure has no way to tell. So when the candidate
/// userinfo contains a `'/'`, `'?'` or `'#'` — i.e. the last `'@'` is not in
/// the authority, and the string cannot be read as `userinfo@host` at all —
/// the whole remainder is replaced by [`REDACTED`], giving
/// `postgres://<redacted>`. The output then looks withheld rather than
/// plausible, which is the honest rendering of "this could be a slashy
/// password or an `'@'` in a path and there is no telling which".
///
/// # The query pass
///
/// Runs after the userinfo pass, on its result. The query is the text between
/// the first `'?'` and the first `'#'`; it is split on `'&'`, and the value of
/// any pair whose key is in [`QUERY_PASSWORD_KEYS`] is replaced wholesale —
/// unconditionally, so an empty value does not need reasoning about separately.
/// Nothing after a `'#'` is sent to the server, so a fragment is not a
/// credential and is left readable.
pub fn redact_url_secrets(url: &str) -> Cow<'_, str> {
    match redact_userinfo(url) {
        // The userinfo pass rewrote the string; run the query pass on *that*,
        // and keep its result if it found anything.
        Some(rewritten) => Cow::Owned(redact_query_passwords(&rewritten).unwrap_or(rewritten)),
        None => match redact_query_passwords(url) {
            Some(rewritten) => Cow::Owned(rewritten),
            // Nothing to redact anywhere: hand back the borrow.
            None => Cow::Borrowed(url),
        },
    }
}

/// Byte offset just past the `"://"` that separates a valid scheme from the
/// rest, or `None` when the string does not start with a scheme.
fn scheme_end(url: &str) -> Option<usize> {
    const SCHEME_SEP: &str = "://";

    let sep = url.find(SCHEME_SEP)?;
    let mut scheme = url[..sep].chars();
    // A scheme is at least one character and starts with a letter, which is
    // what rules out the empty scheme of a bare `"://"`.
    if !scheme.next()?.is_ascii_alphabetic() {
        return None;
    }
    if !scheme.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        return None;
    }
    Some(sep + SCHEME_SEP.len())
}

/// The userinfo pass. `None` when there is nothing to redact.
fn redact_userinfo(url: &str) -> Option<String> {
    let at = url.rfind('@')?;
    // `scheme_end` cannot point past `at`: the prefix it validated would then
    // contain the `'@'`, which is not a scheme character.
    let after_scheme = scheme_end(url).unwrap_or(0);
    let userinfo = &url[after_scheme..at];
    // No `':'` means no password, whatever else the string is doing.
    let colon = userinfo.find(':')?;

    let mut redacted = String::with_capacity(url.len() + REDACTED.len());
    redacted.push_str(&url[..after_scheme]);
    if userinfo.contains(['/', '?', '#']) {
        // The last `'@'` is not in the authority. Withhold the remainder whole
        // rather than name a host we would be guessing at.
        redacted.push_str(REDACTED);
    } else {
        redacted.push_str(&userinfo[..=colon]); // username and the `:`
        redacted.push_str(REDACTED);
        redacted.push_str(&url[at..]); // `@` and everything after it
    }
    Some(redacted)
}

/// The query pass. `None` when the query holds no password parameter.
fn redact_query_passwords(url: &str) -> Option<String> {
    let fragment = url.find('#').unwrap_or(url.len());
    // A `'?'` inside the fragment does not open a query.
    let query_start = url[..fragment].find('?')? + 1;
    let query = &url[query_start..fragment];

    let is_password_key = |key: &str| {
        QUERY_PASSWORD_KEYS
            .iter()
            .any(|candidate| key.eq_ignore_ascii_case(candidate))
    };
    let password_value = |pair: &str| match pair.find('=') {
        Some(eq) => is_password_key(&pair[..eq]).then_some(eq),
        None => None, // a key with no `=` carries no value
    };

    // Checked before anything is allocated, so a query without a password
    // parameter still hands the caller its borrow back.
    if !query.split('&').any(|pair| password_value(pair).is_some()) {
        return None;
    }

    let mut redacted = String::with_capacity(url.len() + REDACTED.len());
    redacted.push_str(&url[..query_start]);
    for (n, pair) in query.split('&').enumerate() {
        if n > 0 {
            redacted.push('&');
        }
        match password_value(pair) {
            Some(eq) => {
                redacted.push_str(&pair[..=eq]); // key and the `=`
                redacted.push_str(REDACTED);
            }
            None => redacted.push_str(pair),
        }
    }
    redacted.push_str(&url[fragment..]); // the `#` and the fragment, if any
    Some(redacted)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A URL with no credential is not mangled: everything a maintainer needs
    /// in order to debug a failure is still there in full.
    ///
    /// The half people forget. Over-redaction is the safer direction but it is
    /// not free, and a function that redacted everything would pass a
    /// leak-only test.
    #[test]
    fn a_url_with_no_credential_survives_verbatim() {
        for url in [
            "https://api.example.com/v1/embeddings",
            "postgres://db.internal:5432/contreforts?sslmode=require",
            "https://user@proxy/v1",
            "https://host/p?password_file=/etc/pgpass",
            "https://host/p#password=notaquery",
        ] {
            assert_eq!(
                redact_url_secrets(url),
                url,
                "{url:?} carries no password and must not be touched"
            );
        }
    }

    /// The two places a credential actually travels.
    #[test]
    fn a_password_is_replaced_wherever_it_sits() {
        assert_eq!(
            redact_url_secrets("https://user:hunter2@proxy/v1/embeddings"),
            format!("https://user:{REDACTED}@proxy/v1/embeddings")
        );
        assert_eq!(
            redact_url_secrets("postgres://h/db?password=hunter2&sslmode=require"),
            format!("postgres://h/db?password={REDACTED}&sslmode=require")
        );
        // Case-insensitive on the key, exact on the whole key.
        assert_eq!(
            redact_url_secrets("postgres://h/db?SSLPassword=hunter2"),
            format!("postgres://h/db?SSLPassword={REDACTED}")
        );
    }

    /// When the last `'@'` is not in the authority, the whole remainder is
    /// withheld rather than spliced back into something that still reads as a
    /// URL naming the wrong host.
    #[test]
    fn an_ambiguous_url_is_withheld_rather_than_rendered_plausible() {
        let rendered = redact_url_secrets("postgres://user:sl/ash-pw@host/db").into_owned();
        assert_eq!(rendered, format!("postgres://{REDACTED}"));
        assert!(
            !rendered.contains("host"),
            "a redaction that keeps a host it cannot vouch for is worse than one that \
             withholds: {rendered}"
        );
    }

    /// A table of hand-picked cases only covers the shapes someone thought of.
    /// This sweeps the cross-product of the parts a URL is made of and asserts
    /// the one property that matters: wherever the password sits, it does not
    /// reach the log.
    ///
    /// Carried over from `contreforts-vecdb`, where it is the regression net
    /// for the class of bug its own table missed twice — a `"://"` that is not
    /// a scheme separator, and an `'@'` that is not the end of a userinfo.
    #[test]
    fn a_sweep_of_url_shapes_never_leaks_the_password() {
        const MARKER: &str = "sweep-must-never-be-logged";

        // Only shapes where the marker really is a password are generated: a
        // marker sitting in a username or a path is not a secret and must
        // *not* be redacted, so asserting its absence there would be wrong.
        let prefixes = ["", "https://", "pg+v2://", "://", "weird stuff://"];
        let users = ["", "appuser", "ad@min", "üser", "u/v", "a:b"];
        let hosts = ["host", "db.internal:5432", "[::1]:5432", "dé.internal"];
        let paths = [
            "",
            "/db",
            "/db?sslmode=require",
            "/db?cb=https://x",
            "/db?u=a@b",
            "/db#fragment",
        ];

        let mut checked = 0usize;
        for prefix in prefixes {
            for user in users {
                for host in hosts {
                    for path in paths {
                        let mut urls = vec![format!("{prefix}{user}:{MARKER}@{host}{path}")];
                        // A `password=` parameter, but only where appending one
                        // yields a query the server would actually read: after
                        // a `#` it is a fragment, and a second `?` lands inside
                        // another parameter's value.
                        if !path.contains('#') {
                            let sep = if path.contains('?') { '&' } else { '?' };
                            urls.push(format!("{prefix}{user}@{host}{path}{sep}password={MARKER}"));
                        }
                        for url in urls {
                            let rendered = redact_url_secrets(&url);
                            assert!(
                                !rendered.contains(MARKER),
                                "url {url:?} leaked its password: {rendered}"
                            );
                            checked += 1;
                        }
                    }
                }
            }
        }
        assert!(checked > 500, "the sweep degenerated to {checked} cases");
    }
}
