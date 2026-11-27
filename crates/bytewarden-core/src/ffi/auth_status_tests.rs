use super::*;

fn identity_error(error: &str, description: &str, message: &str) -> LoginError {
    let json = serde_json::json!({
        "error": error,
        "error_description": description,
        "ErrorModel": { "Message": message, "Object": "synthetic" }
    });
    LoginError::IdentityFail(serde_json::from_value(json).expect("synthetic identity error"))
}

fn response_error(status: u16) -> bw_sdk_core::ApiError {
    bw_sdk_core::ApiError::Response(bitwarden_api_base::ResponseContent {
        status: status.try_into().expect("valid synthetic HTTP status"),
        message: "synthetic response body containing sensitive text".into(),
    })
}

#[test]
fn typed_login_failures_are_classified_without_interpreting_response_bodies() {
    assert_eq!(
        login_error_status(&LoginError::Api(response_error(429))),
        AUTH_RATE_LIMITED
    );
    for status in [400, 401, 403, 404, 500, 502] {
        assert_eq!(
            login_error_status(&LoginError::Api(response_error(status))),
            AUTH_SERVER_ERROR
        );
    }
    assert_eq!(
        login_error_status(&LoginError::Api(bw_sdk_core::ApiError::Serde(
            serde_json::from_str::<serde_json::Value>("{").unwrap_err(),
        ))),
        AUTH_SERVER_ERROR
    );
    assert_eq!(
        login_error_status(&LoginError::Api(bw_sdk_core::ApiError::Io(
            std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "synthetic"),
        ))),
        AUTH_NETWORK_ERROR
    );
    assert_eq!(
        login_error_status(&LoginError::Api(bw_sdk_core::ApiError::Io(
            std::io::Error::new(std::io::ErrorKind::TimedOut, "synthetic"),
        ))),
        AUTH_TIMED_OUT
    );
    assert_eq!(
        login_error_status(&identity_error("invalid_grant", "", "synthetic body")),
        AUTH_REJECTED
    );
    assert_eq!(
        login_error_status(&identity_error("invalid_request", "synthetic body", "")),
        AUTH_SERVER_ERROR
    );
    assert_eq!(
        login_error_status(&identity_error(
            "invalid_request",
            "",
            "Two factor token is invalid"
        )),
        INVALID_TWO_FACTOR_CODE
    );
    assert_eq!(
        login_error_status(&LoginError::AuthenticationFailed),
        AUTH_REJECTED
    );
}

#[test]
fn nested_prelogin_failures_preserve_transport_and_protocol_classification() {
    assert_eq!(
        auth_prelogin_error_status(&PreloginError::Api(response_error(503))),
        AUTH_SERVER_ERROR
    );
    assert_eq!(
        auth_prelogin_error_status(&PreloginError::Api(response_error(429))),
        AUTH_RATE_LIMITED
    );
    assert_eq!(
        auth_prelogin_error_status(&PreloginError::Api(bw_sdk_core::ApiError::Io(
            std::io::Error::new(std::io::ErrorKind::ConnectionReset, "synthetic"),
        ))),
        AUTH_NETWORK_ERROR
    );
    assert_eq!(
        auth_prelogin_error_status(&PreloginError::MissingField(
            bw_sdk_core::MissingFieldError("kdf")
        )),
        AUTH_SERVER_ERROR
    );
    assert_eq!(
        AUTH_TIMED_OUT, 3,
        "the existing 45-second request timeout ABI remains unchanged"
    );
}

#[test]
fn appended_login_statuses_do_not_change_existing_values_or_provider_mask() {
    assert_eq!(AUTH_FAILED, 0);
    assert_eq!(AUTH_RATE_LIMITED, 10);
    assert_eq!(AUTH_NETWORK_ERROR, 11);
    assert_eq!(AUTH_SERVER_ERROR, 12);
    assert_eq!(AUTH_REJECTED, 13);
    assert_eq!(TWO_FACTOR_MASK_SHIFT, 8);
}
