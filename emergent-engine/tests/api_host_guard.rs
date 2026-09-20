//! The host guard through a real axum router, over a real socket.
//!
//! The table in `api_host.rs` decides; this proves the decision reaches a
//! request. It is written against raw bytes and `curl` rather than a test
//! client, because what it has to pin down is exactly what hyper does with a
//! request form no polite client produces: a duplicate `Host`, an absolute
//! form request line, a missing `Host`, and HTTP/2 over cleartext, which
//! `axum::serve` speaks and which carries no `Host` header at all.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{Router, routing::get};
use emergent_engine::api_host::guard_host;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// The same shape `main.rs` builds: one route, the guard in front of it.
fn app(allowed: Vec<String>) -> Router {
    let allowed = Arc::new(allowed);
    Router::new()
        .route("/api/topology", get(|| async { "topology" }))
        .layer(axum::middleware::from_fn(move |request, next| {
            guard_host(allowed.clone(), request, next)
        }))
}

async fn serve(allowed: Vec<String>) -> SocketAddr {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(e) => panic!("could not bind a test listener: {e}"),
    };
    let addr = match listener.local_addr() {
        Ok(addr) => addr,
        Err(e) => panic!("could not read the test listener address: {e}"),
    };
    let app = app(allowed);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    // The listener is already bound, so the accept loop only has to be polled.
    tokio::task::yield_now().await;
    addr
}

/// Write one request and read the whole reply.
///
/// `Connection: close` is added so the read ends when the reply does. Without
/// it every case waits out a keep-alive timeout, which is the difference
/// between this file taking a second and taking a minute.
async fn raw(addr: SocketAddr, request: &str) -> String {
    let request = match request.strip_suffix("\r\n\r\n") {
        Some(head) => format!("{head}\r\nConnection: close\r\n\r\n"),
        None => panic!("a test request ends with a blank line: {request:?}"),
    };
    let mut stream = match TcpStream::connect(addr).await {
        Ok(stream) => stream,
        Err(e) => panic!("could not connect to the test server: {e}"),
    };
    if let Err(e) = stream.write_all(request.as_bytes()).await {
        panic!("could not write the test request: {e}");
    }
    let mut out = Vec::new();
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        stream.read_to_end(&mut out),
    )
    .await;
    String::from_utf8_lossy(&out).into_owned()
}

fn status_of(response: &str) -> &str {
    response.lines().next().unwrap_or("<no response>")
}

#[tokio::test(flavor = "multi_thread")]
async fn the_guard_answers_its_own_names_and_refuses_the_rest() {
    let addr = serve(vec!["app.example".to_owned()]).await;
    let port = addr.port();

    // (what the request says, what hyper makes of it, expected status)
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "origin-form naming localhost",
            format!("GET /api/topology HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n"),
            "HTTP/1.1 200 OK",
        ),
        (
            "origin-form naming the address it connected to",
            format!("GET /api/topology HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
            "HTTP/1.1 200 OK",
        ),
        (
            "origin-form naming a listed host",
            "GET /api/topology HTTP/1.1\r\nHost: app.example\r\n\r\n".to_owned(),
            "HTTP/1.1 200 OK",
        ),
        (
            "the rebinding request",
            "GET /api/topology HTTP/1.1\r\nHost: attacker.example\r\n\r\n".to_owned(),
            "HTTP/1.1 421 Misdirected Request",
        ),
        (
            "absolute-form request line, refused there",
            format!(
                "GET http://attacker.example/api/topology HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n"
            ),
            "HTTP/1.1 421 Misdirected Request",
        ),
        (
            "absolute-form request line, accepted in both places",
            format!(
                "GET http://localhost:{port}/api/topology HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n"
            ),
            "HTTP/1.1 200 OK",
        ),
        (
            "two Host headers, which hyper keeps apart",
            "GET /api/topology HTTP/1.1\r\nHost: localhost\r\nHost: attacker.example\r\n\r\n"
                .to_owned(),
            "HTTP/1.1 421 Misdirected Request",
        ),
        (
            "two Host headers that would both pass alone",
            "GET /api/topology HTTP/1.1\r\nHost: localhost\r\nHost: 127.0.0.1\r\n\r\n".to_owned(),
            "HTTP/1.1 421 Misdirected Request",
        ),
        (
            "no Host at all, which hyper allows through",
            "GET /api/topology HTTP/1.1\r\n\r\n".to_owned(),
            "HTTP/1.1 421 Misdirected Request",
        ),
        (
            "one Host header carrying two names",
            "GET /api/topology HTTP/1.1\r\nHost: localhost, attacker.example\r\n\r\n".to_owned(),
            "HTTP/1.1 421 Misdirected Request",
        ),
        (
            "a name that only looks like localhost",
            "GET /api/topology HTTP/1.1\r\nHost: localhost.attacker.example\r\n\r\n".to_owned(),
            "HTTP/1.1 421 Misdirected Request",
        ),
    ];

    for (label, request, expected) in cases {
        let response = raw(addr, &request).await;
        assert_eq!(status_of(&response), expected, "{label}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_says_what_to_do_and_echoes_nothing_back() {
    let addr = serve(Vec::new()).await;
    let response = raw(
        addr,
        "GET /api/topology HTTP/1.1\r\nHost: attacker.example\r\n\r\n",
    )
    .await;

    assert_eq!(status_of(&response), "HTTP/1.1 421 Misdirected Request");
    assert!(
        response.contains("[engine].api_allowed_hosts"),
        "the refusal should name the key an operator would add to: {response}"
    );
    assert!(
        !response.contains("attacker.example"),
        "the refusal should not echo the name back: {response}"
    );
    assert!(
        !response.contains("topology"),
        "the refusal should carry no part of the answer: {response}"
    );
}

/// `axum::serve` speaks HTTP/2 over cleartext, where there is no `Host` header
/// and the authority is a pseudo-header, so a guard that read only the header
/// would be bypassed by `curl --http2-prior-knowledge`. curl is the only HTTP/2
/// client on the machine, so the test asks for it and says so if it is absent.
#[tokio::test(flavor = "multi_thread")]
async fn http2_carries_its_authority_instead_of_a_host_header() {
    let addr = serve(vec!["app.example".to_owned()]).await;
    let port = addr.port();

    let cases: &[(&str, &str, &str)] = &[
        ("its own address", "127.0.0.1", "200"),
        ("the attacker's name", "attacker.example", "421"),
        ("a listed name", "app.example", "200"),
        ("localhost", "localhost", "200"),
    ];

    for (label, authority, expected) in cases {
        let output = tokio::process::Command::new("curl")
            .args([
                "-s",
                "-o",
                "/dev/null",
                "--max-time",
                "5",
                "--http2-prior-knowledge",
                "-w",
                "%{http_code}",
                "--resolve",
                &format!("{authority}:{port}:127.0.0.1"),
                &format!("http://{authority}:{port}/api/topology"),
            ])
            .output()
            .await;

        match output {
            Ok(output) => assert_eq!(
                String::from_utf8_lossy(&output.stdout).trim(),
                *expected,
                "HTTP/2 naming {label}"
            ),
            Err(e) => panic!("curl is needed to drive an HTTP/2 request: {e}"),
        }
    }
}
