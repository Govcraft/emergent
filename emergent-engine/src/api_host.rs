//! Which host the HTTP API answers to.
//!
//! Binding `127.0.0.1` keeps other machines out, and sending no
//! `Access-Control-Allow-Origin` keeps other origins out. Neither stops DNS
//! rebinding: a page on `attacker.example` re-resolves its own name to
//! `127.0.0.1`, and from then on the browser treats this server as that page's
//! own origin and will hand it the reply. The one thing the attacker cannot
//! choose is the name the browser puts in the request, which stays
//! `attacker.example`. So a server with no authentication answers only to the
//! names it knows to be its own.
//!
//! Rebinding needs a name the attacker controls, so the rule is about names. An
//! IP literal is never one, because a browser sends the address it connected
//! to, and neither is `localhost`. Every other name is refused unless it is
//! listed in `[engine].api_allowed_hosts`, which is what a reverse proxy that
//! forwards its own public name needs.
//!
//! # Kept in step with the sinks
//!
//! `sse-sink` and `topology-viewer` in emergent-primitives ship the same rule
//! (Govcraft/emergent-primitives#16) and the table below mirrors the rows of
//! their `host_test.ts`, so the two implementations cannot drift apart without
//! a test saying so.
//!
//! Their `decideHost` takes the bound address as a third argument, because a
//! sink can be bound to a name and that name is then its own. The engine always
//! binds `127.0.0.1`, an IP literal the rule already accepts, so that argument
//! is dropped here. Their rows that only vary the bound address collapse onto
//! the `127.0.0.1` row, and their rows that bind a name do not apply.

use std::net::Ipv6Addr;
use std::sync::Arc;

use axum::response::IntoResponse;
use tracing::warn;

/// The key an operator adds a name to.
pub const ALLOWED_HOSTS_KEY: &str = "[engine].api_allowed_hosts";

/// What to do with one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostDecision {
    /// The request names only hosts this server knows to be its own.
    Accept,
    /// It does not, and gets a 421.
    Refuse,
}

impl HostDecision {
    /// Whether the request is answered.
    #[must_use]
    pub const fn is_accept(self) -> bool {
        matches!(self, Self::Accept)
    }
}

/// The body of a refusal: what happened, and what the operator can do.
pub const MISDIRECTED_BODY: &str = concat!(
    "Misdirected Request: this server does not answer to the host name in the ",
    "request. If the name is its own, list it in [engine].api_allowed_hosts.\n"
);

/// Lower case, and without the one trailing dot of a fully qualified name.
fn canonical_name(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    lower.strip_suffix('.').unwrap_or(&lower).to_owned()
}

/// A dotted quad, the only IPv4 spelling a browser puts in a request.
///
/// Deliberately not `Ipv4Addr::from_str`, which is the same rule, and
/// deliberately not the libc behavior that reads `127.1` or `0x7f.0.0.1` as an
/// address: a browser does not send those, so a request carrying one is not a
/// browser reaching its own machine.
fn is_ipv4(name: &str) -> bool {
    let mut parts = 0;
    for part in name.split('.') {
        parts += 1;
        let digits =
            !part.is_empty() && part.len() <= 3 && part.bytes().all(|byte| byte.is_ascii_digit());
        if !digits || part.parse::<u16>().is_ok_and(|value| value > 255) {
            return false;
        }
    }
    parts == 4
}

/// A bracketed IPv6 address, as a `Host` value and a URL authority both write
/// it.
fn is_bracketed_ipv6(name: &str) -> bool {
    let Some(inner) = name
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    else {
        return false;
    };
    inner
        .bytes()
        .all(|byte| byte.is_ascii_hexdigit() || byte == b':' || byte == b'.')
        && inner.parse::<Ipv6Addr>().is_ok()
}

/// The host of a request's authority, without its port.
///
/// `app.example:8080` gives `app.example` and `[::1]:8080` gives `[::1]`.
/// `None` when the value is not one host and an optional numeric port, which is
/// also how two `Host` headers joined by a comma are refused.
fn host_of(value: &str) -> Option<String> {
    let (host, port) = if let Some(end) = value.find(']') {
        // A bracketed address: the only host spelling that contains a colon.
        if !value.starts_with('[') {
            return None;
        }
        value.split_at(end + 1)
    } else if let Some(colon) = value.find(':') {
        value.split_at(colon)
    } else {
        (value, "")
    };

    if let Some(port) = port.strip_prefix(':') {
        // An empty port is allowed by the grammar; anything but digits is not.
        if !port.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
    } else if !port.is_empty() {
        return None;
    }

    let bracketed = host.starts_with('[');
    let plain_ok = !host.is_empty()
        && !host
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || matches!(byte, b',' | b'/' | b':'));
    if bracketed || plain_ok {
        Some(canonical_name(host))
    } else {
        None
    }
}

/// Accept or refuse one host a request named (pure function).
///
/// `host` is one authority: a `Host` header value, or the authority of the
/// request URI. `allowed` is the parsed `[engine].api_allowed_hosts` list.
#[must_use]
pub fn decide_host(host: &str, allowed: &[String]) -> HostDecision {
    let Some(host) = host_of(host) else {
        return HostDecision::Refuse;
    };
    let known = is_ipv4(&host)
        || is_bracketed_ipv6(&host)
        || host == "localhost"
        || allowed.contains(&host);
    if known {
        HostDecision::Accept
    } else {
        HostDecision::Refuse
    }
}

/// Accept or refuse a whole request by every host it names (pure function).
///
/// Measured against axum 0.8 on hyper 1.8, which is why both arguments exist
/// and why the count of headers matters:
///
/// - An HTTP/1.1 origin-form request carries a `Host` header and no authority.
/// - An absolute-form request line carries both, and hyper does not reconcile
///   them: `GET http://attacker.example/ HTTP/1.1` with `Host: localhost`
///   arrives with each value untouched, so both have to be checked.
/// - An HTTP/2 request carries an authority and **no** `Host` header at all,
///   and `axum::serve` speaks HTTP/2 over cleartext, so a rule that read only
///   the header would be bypassed by `curl --http2-prior-knowledge`.
/// - hyper does not refuse a duplicate `Host`; it hands over both values. A
///   request naming two hosts that way is refused here. The sinks refuse it
///   too, by a different route: their runtime joins duplicate headers with a
///   comma, which their host grammar rejects.
/// - A request naming no host at all is refused. hyper accepts an HTTP/1.1
///   request with no `Host`, and a server that cannot tell whose name it is
///   answering under should not answer.
#[must_use]
pub fn decide_request_host(
    host_headers: &[&str],
    authority: Option<&str>,
    allowed: &[String],
) -> HostDecision {
    if host_headers.len() > 1 {
        return HostDecision::Refuse;
    }
    let named = host_headers.iter().copied().chain(authority);
    let mut any = false;
    for host in named {
        any = true;
        if decide_host(host, allowed) == HostDecision::Refuse {
            return HostDecision::Refuse;
        }
    }
    if any {
        HostDecision::Accept
    } else {
        HostDecision::Refuse
    }
}

/// Parse every configured name (pure function).
///
/// A host name, with no scheme, port or path. It is stored the way a browser
/// sends it, lower case and in its ASCII form, so that it can match. IP
/// addresses never need listing, and there are no patterns: a `*` would be
/// listed and never match anything.
///
/// # Errors
///
/// Returns the one thing wrong with the first value that is not a host name.
pub fn parse_allowed_hosts(values: &[String]) -> Result<Vec<String>, String> {
    let mut hosts: Vec<String> = Vec::new();
    for value in values {
        let Some(host) = parse_host_name(value) else {
            return Err(format!(
                "Invalid {ALLOWED_HOSTS_KEY} entry \"{value}\": expected a host name with no scheme, port or path, such as app.example"
            ));
        };
        if !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    Ok(hosts)
}

fn parse_host_name(value: &str) -> Option<String> {
    if value.is_empty() || value.starts_with('-') {
        return None;
    }
    if value.bytes().any(|byte| {
        byte.is_ascii_whitespace()
            || matches!(
                byte,
                b'/' | b'?' | b'#' | b'@' | b':' | b'*' | b',' | b'\\' | b'[' | b']'
            )
    }) {
        return None;
    }
    // The same parser a browser uses, so a name is stored in the spelling the
    // browser will send: punycode for a non-ASCII name, lower case, no
    // trailing dot.
    let parsed = url::Url::parse(&format!("http://{value}/")).ok()?;
    let host = canonical_name(parsed.host_str()?);
    (!host.is_empty()).then_some(host)
}

/// The startup line saying which hosts are answered.
#[must_use]
pub fn describe_allowed_hosts(allowed: &[String]) -> String {
    let mut names = vec!["localhost".to_owned()];
    for host in allowed {
        if !names.contains(host) {
            names.push(host.clone());
        }
    }
    format!("any IP address, {}", names.join(", "))
}

// ============================================================================
// The one adapter: axum's request, and nothing else
// ============================================================================

/// Refuse an HTTP API request that names a host this server is not.
///
/// The decision is [`decide_request_host`], a pure function tested against the
/// same table the sinks use. This only pulls the two things hyper hands over
/// and turns a refusal into a 421.
///
/// Every host header value is checked, not just the first, and the request
/// URI's authority is checked as well: hyper keeps a duplicate `Host` and an
/// absolute-form request line intact, and an HTTP/2 request carries its
/// authority and no `Host` header at all.
pub async fn guard_host(
    allowed: Arc<Vec<String>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let headers: Vec<&str> = request
        .headers()
        .get_all(axum::http::header::HOST)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect();
    let authority = request
        .uri()
        .authority()
        .map(axum::http::uri::Authority::as_str);

    if decide_request_host(&headers, authority, &allowed).is_accept() {
        drop(headers);
        return next.run(request).await;
    }

    warn!(
        host = ?headers,
        authority = ?authority,
        "HTTP API refused a request naming a host that is not this server's. \
         If the name is its own, list it in [engine].api_allowed_hosts"
    );
    (
        axum::http::StatusCode::MISDIRECTED_REQUEST,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        MISDIRECTED_BODY,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two names `host_test.ts` lists, so the rows read the same.
    fn listed() -> Vec<String> {
        vec!["app.example".to_owned(), "viewer.internal".to_owned()]
    }

    #[test]
    fn decide_host_table() {
        use HostDecision::{Accept, Refuse};
        let listed = listed();
        let none: Vec<String> = Vec::new();

        // Every row of the sinks' decideHost table that applies to a server
        // bound to an IP literal. Their rows that only vary the bound address
        // collapse onto this one, and their rows that bind a name cannot
        // happen here.
        let cases: &[(&str, &[String], HostDecision)] = &[
            // The names that are always its own.
            ("localhost", &none, Accept),
            ("localhost:8080", &none, Accept),
            ("127.0.0.1:8080", &none, Accept),
            ("127.0.0.1", &none, Accept),
            ("[::1]:8080", &none, Accept),
            ("[::1]", &none, Accept),
            ("LOCALHOST:8080", &none, Accept),
            ("localhost.", &none, Accept),
            ("127.0.0.1.", &none, Accept),
            // The rebinding request: the attacker's name, whatever port.
            ("attacker.example", &none, Refuse),
            ("attacker.example:8080", &none, Refuse),
            ("attacker.example:8080", &listed, Refuse),
            // Names dressed up as the accepted ones.
            ("localhost.attacker.example", &none, Refuse),
            ("attacker.localhost", &none, Refuse),
            ("127.0.0.1.attacker.example", &none, Refuse),
            ("app.example.attacker.example", &listed, Refuse),
            ("xapp.example", &listed, Refuse),
            // Spellings of an address a browser does not send.
            ("127.1", &none, Refuse),
            ("256.0.0.1", &none, Refuse),
            ("1.2.3.4.5", &none, Refuse),
            ("0x7f.0.0.1", &none, Refuse),
            ("[::1", &none, Refuse),
            ("[attacker.example]", &none, Refuse),
            ("[zz::1]", &none, Refuse),
            // An address cannot be rebound, so any IP literal is accepted: a
            // port forward or a container publishes a loopback server under
            // the machine's own address.
            ("192.168.1.20:8080", &none, Accept),
            ("192.168.1.20:9000", &none, Accept),
            ("10.0.0.5", &none, Accept),
            ("[fe80::1]:8080", &none, Accept),
            ("[2001:db8::1]:8080", &none, Accept),
            ("[::ffff:192.168.1.20]:8080", &none, Accept),
            // A machine name is a name like any other, exposed or not.
            ("myhost.local:8080", &none, Refuse),
            ("myhost:8080", &none, Refuse),
            ("myhost.local:8080", &["myhost.local".to_owned()], Accept),
            // Listed names: what a reverse proxy forwarding its own name sends.
            ("app.example", &listed, Accept),
            ("app.example:443", &listed, Accept),
            ("viewer.internal:8080", &listed, Accept),
            ("APP.Example", &listed, Accept),
            ("app.example.", &listed, Accept),
            ("app.example.:8080", &listed, Accept),
            // Not one host and an optional port.
            ("", &none, Refuse),
            (":8080", &none, Refuse),
            ("localhost:80abc", &none, Refuse),
            ("localhost:8080:1", &none, Refuse),
            ("localhost, attacker.example", &none, Refuse),
            ("attacker.example, localhost", &none, Refuse),
            ("localhost attacker.example", &none, Refuse),
            ("localhost/attacker.example", &none, Refuse),
            ("attacker.example@localhost", &none, Refuse),
            ("[::1]x", &none, Refuse),
            // An empty port is allowed by the grammar.
            ("localhost:", &none, Accept),
        ];

        for (host, allowed, expected) in cases {
            assert_eq!(
                decide_host(host, allowed),
                *expected,
                "Host {host:?}, listed {allowed:?}"
            );
        }
    }

    #[test]
    fn decide_request_host_refuses_when_any_named_host_is_refused() {
        use HostDecision::{Accept, Refuse};
        let listed = listed();

        // One row per form the probe showed hyper can produce.
        let cases: &[(&[&str], Option<&str>, HostDecision)] = &[
            // HTTP/1.1 origin-form: the header alone.
            (&["localhost:8080"], None, Accept),
            (&["attacker.example"], None, Refuse),
            // Absolute-form: hyper keeps the header and the authority apart,
            // so a line that names one host and a header that names another
            // are both checked.
            (&["localhost:8080"], Some("localhost:8080"), Accept),
            (&["localhost:8080"], Some("attacker.example"), Refuse),
            (&["attacker.example"], Some("localhost:8080"), Refuse),
            (&["app.example"], Some("localhost:8080"), Accept),
            // HTTP/2: an authority and no header at all.
            (&[], Some("127.0.0.1:8891"), Accept),
            (&[], Some("attacker.example"), Refuse),
            (&[], Some("app.example"), Accept),
            // A request naming no host at all.
            (&[], None, Refuse),
            // Two Host headers, which hyper hands over unjoined.
            (&["localhost", "attacker.example"], None, Refuse),
            (&["attacker.example", "localhost"], None, Refuse),
            // Refused even when both would pass on their own: a request that
            // names its host twice is not one this server should answer.
            (&["localhost", "127.0.0.1"], None, Refuse),
        ];

        for (headers, authority, expected) in cases {
            assert_eq!(
                decide_request_host(headers, *authority, &listed),
                *expected,
                "headers {headers:?}, authority {authority:?}"
            );
        }
    }

    #[test]
    fn parse_allowed_hosts_accepts_table() {
        let cases: &[(&[&str], &[&str])] = &[
            (&[], &[]),
            (&["app.example"], &["app.example"]),
            (
                &["app.example", "viewer.internal"],
                &["app.example", "viewer.internal"],
            ),
            (&["myhost"], &["myhost"]),
            (&["my_host.local"], &["my_host.local"]),
            // Stored in the spelling a browser sends, so that it can match.
            (&["APP.Example"], &["app.example"]),
            (&["app.example."], &["app.example"]),
            (&["münchen.example"], &["xn--mnchen-3ya.example"]),
            // Listed twice, kept once.
            (&["app.example", "App.Example."], &["app.example"]),
            // Never needed, and harmless.
            (&["192.168.1.20"], &["192.168.1.20"]),
            (&["localhost"], &["localhost"]),
        ];

        for (values, expected) in cases {
            let values: Vec<String> = values.iter().map(|v| (*v).to_owned()).collect();
            assert_eq!(
                parse_allowed_hosts(&values).as_deref(),
                Ok(expected
                    .iter()
                    .map(|v| (*v).to_owned())
                    .collect::<Vec<_>>()
                    .as_slice()),
                "{values:?}"
            );
        }
    }

    #[test]
    fn parse_allowed_hosts_rejects_table() {
        let cases = [
            "",
            ".",
            "https://app.example",
            "app.example:8080",
            "app.example/",
            "app.example/events",
            "user@app.example",
            "app.example?x=1",
            "app.example#top",
            "app example",
            "app.example,viewer.internal",
            // There are no patterns: these would be listed and never match.
            "*",
            "*.app.example",
            "[::1]",
            "a\\b",
        ];

        for value in cases {
            let expected = format!(
                "Invalid {ALLOWED_HOSTS_KEY} entry \"{value}\": expected a host name with no scheme, port or path, such as app.example"
            );
            assert_eq!(
                parse_allowed_hosts(&[value.to_owned()]),
                Err(expected.clone()),
                "{value}"
            );
            // Named wherever it sits in the list, not only first.
            assert_eq!(
                parse_allowed_hosts(&["ok.example".to_owned(), value.to_owned()]),
                Err(expected),
                "{value}"
            );
        }
    }

    #[test]
    fn every_name_that_parses_is_one_decide_host_accepts() {
        // The point of storing a name in the browser's spelling: what an
        // operator writes and what arrives are not the same string.
        let written = ["APP.Example.", "münchen.example", "my_host"];
        let sent = ["app.example:8080", "xn--mnchen-3ya.example", "my_host:80"];

        for (written, sent) in written.iter().zip(sent) {
            let parsed = match parse_allowed_hosts(&[(*written).to_owned()]) {
                Ok(parsed) => parsed,
                Err(e) => panic!("{written} should parse: {e}"),
            };
            assert_eq!(
                decide_host(sent, &parsed),
                HostDecision::Accept,
                "{written}"
            );
            assert_eq!(decide_host(sent, &[]), HostDecision::Refuse, "{written}");
        }
    }

    #[test]
    fn the_refusal_says_what_to_do_about_it() {
        assert!(MISDIRECTED_BODY.contains("does not answer to the host name"));
        assert!(
            MISDIRECTED_BODY.contains(ALLOWED_HOSTS_KEY),
            "a refusal that does not name the key leaves the operator guessing"
        );
        assert!(MISDIRECTED_BODY.ends_with('\n'));
    }

    #[test]
    fn describe_allowed_hosts_says_which_hosts_are_answered() {
        assert_eq!(describe_allowed_hosts(&[]), "any IP address, localhost");
        assert_eq!(
            describe_allowed_hosts(&listed()),
            "any IP address, localhost, app.example, viewer.internal"
        );
        assert_eq!(
            describe_allowed_hosts(&["localhost".to_owned()]),
            "any IP address, localhost"
        );
    }
}
