//! Pure synthetic state tests: no GPUI windows, core requests, or network calls.
use super::*;

fn authenticator_options() -> TwoFactorOptions {
    TwoFactorOptions::new(Region::Us, [true, false, false])
}

fn device_then_two_factor() -> LoginChallenge {
    let mut flow = LoginChallenge::default();
    flow.device();
    let mut code = Zeroizing::new("synthetic-device-otp".to_owned());
    let device_code = flow.device_code(&mut code);
    assert!(code.is_empty());
    flow.two_factor(authenticator_options(), device_code);
    flow
}

#[test]
fn dismissing_an_inflight_unlock_cancels_and_rejects_late_decrypted_results() {
    assert!(unlock_dismissal_requires_cancellation(
        Some(Form::Unlock),
        true
    ));
    assert!(!unlock_dismissal_requires_cancellation(
        Some(Form::Unlock),
        false
    ));
    assert!(!unlock_dismissal_requires_cancellation(
        Some(Form::SignIn),
        true
    ));
    assert!(!unlock_result_still_current(None, false));
    assert!(!unlock_result_still_current(Some(Form::SignIn), false));
    assert!(!unlock_result_still_current(Some(Form::Unlock), true));
    assert!(unlock_result_still_current(Some(Form::Unlock), false));
}

#[test]
fn device_otp_survives_two_factor_transition_and_is_supplied_separately_from_token() {
    let mut flow = device_then_two_factor();
    assert!(matches!(flow.challenge, Some(Challenge::TwoFactor(_))));
    assert_eq!(
        flow.pending_device_otp.as_deref().map(String::as_str),
        Some("synthetic-device-otp")
    );

    let mut token = Zeroizing::new("synthetic-authenticator-token".to_owned());
    let device_code = flow.device_code(&mut token);
    assert_eq!(device_code.as_str(), "synthetic-device-otp");
    assert_eq!(token.as_str(), "synthetic-authenticator-token");
    // Ownership moves to the worker; the UI retains no duplicate OTP buffer.
    assert!(flow.pending_device_otp.is_none());

    // A repeated server challenge retains the same device OTP for the next try.
    flow.two_factor(authenticator_options(), device_code);
    let device_code = flow.device_code(&mut token);
    assert_eq!(device_code.as_str(), "synthetic-device-otp");
}

#[test]
fn base_sign_in_never_sends_an_unrelated_code_as_a_device_otp() {
    let mut flow = LoginChallenge::default();
    let mut token = Zeroizing::new("synthetic-unused-code".to_owned());
    assert!(flow.device_code(&mut token).is_empty());
    assert_eq!(token.as_str(), "synthetic-unused-code");
}

#[test]
fn reset_clears_pending_challenge_and_invalidates_replies() {
    // close_form uses this reset on success, cancel, restart, and account cleanup.
    let mut flow = device_then_two_factor();
    let generation = flow.generation;
    assert!(flow.is_current(generation));
    flow.reset();
    assert!(flow.challenge.is_none());
    assert!(flow.pending_device_otp.is_none());
    assert!(!flow.is_current(generation));
}

#[test]
fn cancellation_during_worker_owned_otp_invalidates_the_late_reply() {
    let mut flow = device_then_two_factor();
    let generation = flow.generation;
    let mut token = Zeroizing::new("synthetic-token".to_owned());
    let worker_device_code = flow.device_code(&mut token);
    flow.reset();
    assert!(!flow.is_current(generation));
    assert!(flow.challenge.is_none());
    assert!(flow.pending_device_otp.is_none());
    // Like the guarded finish callback, discard the worker's zeroizing result.
    drop(worker_device_code);
}

#[test]
fn terminal_failures_return_to_base_form_instead_of_hiding_a_cleared_password() {
    for status in [
        Some(AuthStatus::Failed),
        Some(AuthStatus::Cancelled),
        Some(AuthStatus::AlreadySignedIn),
        Some(AuthStatus::AccountLimit),
        None, // Core argument/transport errors.
    ] {
        let mut flow = device_then_two_factor();
        assert!(flow.failure(status, false)); // Caller clears password and restarts sign-in.
        assert!(flow.challenge.is_none()); // render_form shows password on base sign-in.
        assert!(flow.pending_device_otp.is_none());
    }
}

#[test]
fn failed_final_auth_discards_device_otp_and_restarts_chain_for_retry() {
    for status in [
        AuthStatus::InvalidTwoFactor,
        AuthStatus::TimedOut,
        AuthStatus::RateLimited,
        AuthStatus::Failed,
        AuthStatus::Cancelled,
    ] {
        let mut flow = device_then_two_factor();
        let mut token = Zeroizing::new("synthetic-token".to_owned());
        let worker_device_code = flow.device_code(&mut token);
        assert!(flow.failure(Some(status), !worker_device_code.is_empty()));
        assert!(flow.challenge.is_none());
        assert!(flow.pending_device_otp.is_none());
    }
}

#[test]
fn failed_email_request_also_discards_pending_device_otp_and_restarts_chain() {
    let mut flow = device_then_two_factor();
    // Sending email does not take the pending OTP out of UI state.
    assert!(flow.failure(Some(AuthStatus::TimedOut), false));
    assert!(flow.challenge.is_none());
    assert!(flow.pending_device_otp.is_none());
}

#[test]
fn invalid_device_code_returns_to_device_challenge_and_discards_old_otp() {
    let mut flow = device_then_two_factor();
    assert!(!flow.failure(Some(AuthStatus::InvalidDeviceCode), true));
    assert!(matches!(flow.challenge, Some(Challenge::Device)));
    assert!(flow.pending_device_otp.is_none());
    let mut replacement = Zeroizing::new("synthetic-replacement-otp".to_owned());
    assert_eq!(
        flow.device_code(&mut replacement).as_str(),
        "synthetic-replacement-otp"
    );
    assert!(replacement.is_empty());
}

#[test]
fn ordinary_two_factor_retries_keep_challenge_when_no_device_otp_is_needed() {
    for status in [
        AuthStatus::InvalidTwoFactor,
        AuthStatus::TimedOut,
        AuthStatus::RateLimited,
    ] {
        let mut flow = LoginChallenge::default();
        flow.two_factor(authenticator_options(), Zeroizing::default());
        assert!(!flow.failure(Some(status), false));
        assert!(matches!(flow.challenge, Some(Challenge::TwoFactor(_))));
        assert!(flow.pending_device_otp.is_none());
    }
}

#[test]
fn renewed_device_challenge_discards_previously_accepted_otp() {
    let mut flow = device_then_two_factor();
    flow.device();
    assert!(matches!(flow.challenge, Some(Challenge::Device)));
    assert!(flow.pending_device_otp.is_none());
}

#[test]
fn manual_authenticator_matches_winui_for_every_supported_provider_projection() {
    // Empty metadata and masks containing only unknown/unsupported providers
    // both project to [false; 3]; no raw-mask constructor or core FFI is needed.
    for region in [Region::Us, Region::Eu, Region::Custom] {
        for bits in 0..8 {
            let supported = [bits & 1 != 0, bits & 2 != 0, bits & 4 != 0];
            let options = TwoFactorOptions::new(region, supported);
            let manual = region == Region::Custom && bits == 0;
            assert_eq!(options.manual_authenticator, manual);
            for (index, (provider, _)) in PROVIDERS.into_iter().enumerate() {
                assert_eq!(
                    options.offers(provider),
                    supported[index] || (provider == Provider::Authenticator && manual)
                );
            }
        }
    }
}

#[test]
fn manual_fallback_replaces_stale_provider_with_authenticator_and_remains_a_challenge() {
    let options = TwoFactorOptions::new(Region::Custom, [false; 3]);
    assert_eq!(
        options.selected(Provider::Email),
        Some(Provider::Authenticator)
    );
    let mut flow = LoginChallenge::default();
    flow.two_factor(options, Zeroizing::default());
    // Offering manual input never changes session/authentication status.
    assert!(matches!(flow.challenge, Some(Challenge::TwoFactor(_))));
    assert!(flow.pending_device_otp.is_none());
}

#[test]
fn advertised_provider_selection_preserves_previous_choice_and_cloud_has_no_fallback() {
    let options = TwoFactorOptions::new(Region::Custom, [true, true, false]);
    assert!(!options.manual_authenticator);
    assert_eq!(options.selected(Provider::Email), Some(Provider::Email));
    assert_eq!(
        options.selected(Provider::YubiKey),
        Some(Provider::Authenticator)
    );
    for region in [Region::Us, Region::Eu] {
        let unsupported = TwoFactorOptions::new(region, [false; 3]);
        assert!(!unsupported.manual_authenticator);
        assert_eq!(unsupported.selected(Provider::Authenticator), None);
    }
}
