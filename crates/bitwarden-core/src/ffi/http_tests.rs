//! Exercise the actual patched SDK transport in both Debug and Release builds.
use super::*;
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

fn loopback_client(origin: &str, opt_in: bool) -> Client {
    let builder = ClientBuilder::new().with_settings(ClientSettings {
        api_url: format!("{origin}/api"),
        identity_url: format!("{origin}/identity"),
        ..ClientSettings::default()
    });
    if opt_in {
        builder.with_loopback_http().build()
    } else {
        builder.build()
    }
}

#[tokio::test]
async fn opted_in_loopback_identity_and_api_keep_default_headers_and_reach_the_service() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/probe"))
        .respond_with(ResponseTemplate::new(200))
        .expect(2)
        .mount(&server)
        .await;
    let client = loopback_client(&server.uri(), true);
    let config = client.internal.get_api_configurations();
    for transport in [&config.identity_config.client, &config.api_config.client] {
        let response = transport
            .get(format!("{}/probe", server.uri()))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
    }
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    for request in requests {
        assert!(request.headers.contains_key("device-type"));
        assert!(request.headers.contains_key("user-agent"));
    }
}

#[tokio::test]
async fn loopback_transports_reject_requests_to_other_origins_before_network_io() {
    let service = MockServer::start().await;
    let other = MockServer::start().await;
    let client = loopback_client(&service.uri(), true);
    let config = client.internal.get_api_configurations();
    for transport in [&config.identity_config.client, &config.api_config.client] {
        for url in [
            format!("{}/probe", other.uri()),
            "http://example.invalid/probe".to_owned(),
            "https://example.invalid/probe".to_owned(),
        ] {
            let error = transport.get(url).send().await.unwrap_err();
            assert!(matches!(error, reqwest_middleware::Error::Middleware(_)));
        }
    }
    assert!(service.received_requests().await.unwrap().is_empty());
    assert!(other.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn loopback_identity_and_api_return_redirects_without_following_them() {
    let service = MockServer::start().await;
    let other = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/redirect"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", format!("{}/probe", other.uri())),
        )
        .expect(2)
        .mount(&service)
        .await;
    let client = loopback_client(&service.uri(), true);
    let config = client.internal.get_api_configurations();
    for transport in [&config.identity_config.client, &config.api_config.client] {
        let response = transport
            .get(format!("{}/redirect", service.uri()))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::FOUND);
    }
    assert!(other.received_requests().await.unwrap().is_empty());
}

#[test]
fn loopback_transports_bypass_configured_proxies() {
    // Isolate proxy variables in a child process; parallel tests keep their own environment.
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "ffi::http_tests::opted_in_loopback_identity_and_api_keep_default_headers_and_reach_the_service",
        ])
        .env("HTTP_PROXY", "http://127.0.0.1:1")
        .env("HTTPS_PROXY", "http://127.0.0.1:1")
        .env("ALL_PROXY", "http://127.0.0.1:1")
        .env("http_proxy", "http://127.0.0.1:1")
        .env("https_proxy", "http://127.0.0.1:1")
        .env("all_proxy", "http://127.0.0.1:1")
        .env("NO_PROXY", "")
        .env("no_proxy", "")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "proxy-isolated transport test failed"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

#[cfg(not(debug_assertions))]
#[tokio::test]
async fn release_sdk_without_opt_in_still_rejects_plain_http() {
    let service = MockServer::start().await;
    let client = loopback_client(&service.uri(), false);
    let config = client.internal.get_api_configurations();
    for transport in [&config.identity_config.client, &config.api_config.client] {
        assert!(transport
            .get(format!("{}/probe", service.uri()))
            .send()
            .await
            .is_err());
    }
    assert!(service.received_requests().await.unwrap().is_empty());
}
