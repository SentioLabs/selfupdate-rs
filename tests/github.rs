//! GitHub API integration against a local HTTP server.
mod common;
use common::{Reply, Server};
use selfupdate::*;
use std::time::{Duration, Instant};

fn github(server: &Server) -> GitHubSource {
    GitHubSource::builder("acme", "tool")
        .api_base(&server.url)
        .client(
            reqwest::blocking::Client::builder()
                .no_proxy()
                .build()
                .unwrap(),
        )
        .build()
        .unwrap()
}

#[test]
fn latest_headers_token_and_enterprise_base() {
    let server = Server::new(vec![Reply::ok(r#"{"tag_name":"v1.2.3"}"#)]);
    let source = GitHubSource::builder("acme", "tool")
        .api_base(format!("{}/api/v3/", server.url))
        .token("secret")
        .client(
            reqwest::blocking::Client::builder()
                .no_proxy()
                .user_agent("consumer")
                .build()
                .unwrap(),
        )
        .build()
        .unwrap();
    assert_eq!(
        source.latest(&CancellationToken::new()).unwrap(),
        Release {
            tag: "v1.2.3".into(),
            prerelease: false
        }
    );
    let request = server.requests()[0].to_lowercase();
    assert!(request.starts_with("get /api/v3/repos/acme/tool/releases/latest http/1.1"));
    assert!(request.contains("accept: application/vnd.github+json\r\n"));
    assert!(request.contains("authorization: bearer secret\r\n"));
    assert!(request.contains(&format!(
        "user-agent: selfupdate/{}\r\n",
        env!("CARGO_PKG_VERSION")
    )));
}

#[test]
fn page_limits_drafts_and_order() {
    let json = r#"[{"tag_name":"v1.3.0-rc.1","prerelease":true},{"tag_name":"v9.0.0","draft":true},{"tag_name":"v1.2.3"}]"#;
    let server = Server::new((0..4).map(|_| Reply::ok(json)).collect());
    let source = github(&server);
    for limit in [50, 0, 101, 100] {
        let list = source.list(&CancellationToken::new(), limit).unwrap();
        assert_eq!(
            list,
            [
                Release {
                    tag: "v1.3.0-rc.1".into(),
                    prerelease: true
                },
                Release {
                    tag: "v1.2.3".into(),
                    prerelease: false
                }
            ]
        );
    }
    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    for (request, expected) in requests.iter().zip([50, 100, 100, 100]) {
        assert!(request.starts_with(&format!(
            "GET /repos/acme/tool/releases?per_page={expected} HTTP/1.1"
        )));
        assert!(!request.to_lowercase().contains("authorization:"));
    }
}

#[test]
fn no_match_fallback_is_requested_through_http() {
    let server = Server::new(vec![
        Reply::ok(r#"[{"tag_name":"v2.0.0-beta.1","prerelease":true}]"#),
        Reply::ok(r#"{"tag_name":"v1.2.3"}"#),
    ]);
    assert_eq!(
        resolve(
            &CancellationToken::new(),
            &github(&server),
            &Channel::RC,
            None
        )
        .unwrap(),
        "v1.2.3"
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].contains("per_page=100"));
    assert!(requests[1].contains("/releases/latest"));
}

#[test]
fn fallback_failure_is_not_hidden() {
    let mut denied = Reply::ok("denied");
    denied.status = 403;
    let server = Server::new(vec![Reply::ok("[]"), denied]);
    assert!(matches!(
        resolve(
            &CancellationToken::new(),
            &github(&server),
            &Channel::RC,
            None
        ),
        Err(Error::HttpStatus { status: 403, .. })
    ));
}

#[test]
fn malformed_json_http_errors_and_draft_latest() {
    for status in [201, 204, 401, 403, 404, 429, 500] {
        let mut reply = Reply::ok("");
        reply.status = status;
        let server = Server::new(vec![reply]);
        assert!(
            matches!(github(&server).latest(&CancellationToken::new()), Err(Error::HttpStatus { status: actual, .. }) if actual == status)
        );
    }
    for json in [
        "not json",
        r#"{"tag_name":12}"#,
        r#"{"prerelease":"true"}"#,
        "[]",
    ] {
        let server = Server::new(vec![Reply::ok(json)]);
        let error = github(&server)
            .latest(&CancellationToken::new())
            .unwrap_err();
        assert!(matches!(error, Error::Decode { .. }));
        assert!(std::error::Error::source(&error).is_some());
    }
    let server = Server::new(vec![Reply::ok(r#"{"tag_name":"v9.0.0","draft":true}"#)]);
    assert!(
        github(&server)
            .latest(&CancellationToken::new())
            .unwrap()
            .tag
            .is_empty()
    );
}

#[test]
fn timeout_bounds_headers_and_body_even_with_custom_client() {
    for body_delay in [false, true] {
        let mut reply = Reply::ok(r#"{"tag_name":"v1.0.0"}"#);
        if body_delay {
            reply.before_body = Duration::from_millis(300);
        } else {
            reply.before_headers = Duration::from_millis(300);
        }
        let server = Server::new(vec![reply]);
        let source = GitHubSource::builder("acme", "tool")
            .api_base(&server.url)
            .timeout(Duration::from_millis(75))
            .client(
                reqwest::blocking::Client::builder()
                    .no_proxy()
                    .timeout(Duration::from_secs(10))
                    .build()
                    .unwrap(),
            )
            .build()
            .unwrap();
        let start = Instant::now();
        let error = source.latest(&CancellationToken::new()).unwrap_err();
        assert!(
            matches!(error, Error::Http(ref e) if e.is_timeout()),
            "{error}"
        );
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}

#[test]
fn cancellation_before_request_and_after_headers_or_body() {
    let server = Server::new(vec![]);
    let token = CancellationToken::new();
    token.clone().cancel();
    assert!(github(&server).latest(&token).unwrap_err().is_cancelled());
    assert!(server.requests().is_empty());
    for after_headers in [false, true] {
        let token = CancellationToken::new();
        let cancel = token.clone();
        let mut reply = Reply::ok(r#"{"tag_name":"v1.0.0"}"#);
        // Cancel before the server releases the headers or body. Fixed sleeps
        // let a delayed cancellation thread lose the race on busy CI runners.
        if after_headers {
            reply.on_headers = Some(Box::new(move || cancel.cancel()));
        } else {
            reply.on_request = Some(Box::new(move || cancel.cancel()));
        }
        let server = Server::new(vec![reply]);
        assert!(github(&server).latest(&token).unwrap_err().is_cancelled());
        assert_eq!(server.requests().len(), 1);
    }
}

#[test]
fn configuration_and_transport_errors_retain_causes() {
    assert!(
        GitHubSource::builder("acme", "tool")
            .api_base("not a URL")
            .build()
            .is_err()
    );
    assert!(
        GitHubSource::builder("acme", "tool")
            .timeout(Duration::ZERO)
            .build()
            .is_err()
    );
    assert!(GitHubSource::new("", "tool").is_err());
    for base in [
        "file:///tmp/api",
        "https://example.com/api?query=1",
        "https://example.com/#fragment",
    ] {
        assert!(
            GitHubSource::builder("acme", "tool")
                .api_base(base)
                .build()
                .is_err()
        );
    }
    // A bound but non-listening port reliably rejects connections without external networking.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let source = GitHubSource::builder("acme", "tool")
        .api_base(format!("http://{addr}"))
        .timeout(Duration::from_millis(100))
        .build()
        .unwrap();
    let error = source.latest(&CancellationToken::new()).unwrap_err();
    assert!(matches!(error, Error::Http(_)));
    assert!(std::error::Error::source(&error).is_some());
}
