use std::ffi::{c_char, CStr, CString};

use bitwarden_generators::{
    GeneratorClientsExt, PassphraseGeneratorRequest, PasswordGeneratorRequest,
};
use bw_sdk_core::Client;

fn generate_password(
    length: u32,
    options: u32,
    min_numbers: u32,
    min_special: u32,
) -> Option<String> {
    let min_numbers = u8::try_from(min_numbers).ok()?;
    let min_special = u8::try_from(min_special).ok()?;
    if min_numbers > 9 || min_special > 9 {
        return None;
    }

    let request = PasswordGeneratorRequest {
        uppercase: options & 1 != 0,
        lowercase: options & 2 != 0,
        numbers: options & 4 != 0,
        special: options & 8 != 0,
        length: length.clamp(5, 128) as u8,
        avoid_ambiguous: options & 16 != 0,
        min_number: Some(min_numbers),
        min_special: Some(min_special),
        ..Default::default()
    };

    Client::new(None).generator().password(request).ok()
}

fn generate_passphrase(
    num_words: u32,
    word_separator: &str,
    capitalize: bool,
    include_number: bool,
) -> Option<String> {
    let request = PassphraseGeneratorRequest {
        num_words: u8::try_from(num_words).ok()?,
        word_separator: word_separator.to_owned(),
        capitalize,
        include_number,
    };

    Client::new(None).generator().passphrase(request).ok()
}

fn into_raw_string(value: String) -> *mut c_char {
    CString::new(value)
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn bw_core_version() -> *const c_char {
    b"0.2.0\0".as_ptr().cast()
}

#[no_mangle]
pub extern "C" fn bw_generate_password(
    length: u32,
    options: u32,
    min_numbers: u32,
    min_special: u32,
) -> *mut c_char {
    generate_password(length, options, min_numbers, min_special)
        .map(into_raw_string)
        .unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "C" fn bw_generate_passphrase(
    num_words: u32,
    word_separator: *const c_char,
    capitalize: u8,
    include_number: u8,
) -> *mut c_char {
    if word_separator.is_null() {
        return std::ptr::null_mut();
    }

    let Ok(word_separator) = (unsafe { CStr::from_ptr(word_separator) }).to_str() else {
        return std::ptr::null_mut();
    };

    generate_passphrase(
        num_words,
        word_separator,
        capitalize != 0,
        include_number != 0,
    )
    .map(into_raw_string)
    .unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "C" fn bw_string_free(value: *mut c_char) {
    if value.is_null() {
        return;
    }

    let owned = unsafe { CString::from_raw(value) };
    let length = owned.as_bytes_with_nul().len();
    unsafe { std::ptr::write_bytes(owned.as_ptr().cast_mut(), 0, length) };
    drop(owned);
}

#[cfg(test)]
mod tests {
    use super::{generate_passphrase, generate_password};
    use bitwarden_generators::{GeneratorClientsExt, PasswordGeneratorRequest};
    use bw_sdk_core::Client;

    #[test]
    fn generated_password_obeys_length_and_character_options() {
        let password = generate_password(24, 1 | 2 | 4 | 8, 2, 2).expect("valid options");
        assert_eq!(password.len(), 24);
        assert!(password.bytes().any(|byte| byte.is_ascii_lowercase()));
        assert!(password.bytes().any(|byte| byte.is_ascii_uppercase()));
        assert!(password.bytes().filter(u8::is_ascii_digit).count() >= 2);
        assert!(
            password
                .bytes()
                .filter(|byte| b"!@#$%^&*".contains(byte))
                .count()
                >= 2
        );
    }

    #[test]
    fn password_generator_clamps_length_and_respects_ambiguous_setting() {
        let password = generate_password(1, 1 | 2 | 16, 0, 0).expect("valid options");
        assert_eq!(password.len(), 5);
        assert!(!password.bytes().any(|byte| b"IOl01".contains(&byte)));
        assert_eq!(
            generate_password(1000, 1, 0, 0)
                .expect("valid options")
                .len(),
            128
        );
    }

    #[test]
    fn impossible_password_settings_are_rejected() {
        assert!(generate_password(5, 1 | 2 | 4, 4, 0).is_none());
        assert!(generate_password(20, 0, 0, 0).is_none());
        assert!(generate_password(20, 4, 10, 0).is_none());
    }

    #[test]
    fn passphrase_options_match_desktop_defaults_and_bounds() {
        let passphrase = generate_passphrase(6, "-", false, false).expect("valid options");
        assert_eq!(passphrase.split('-').count(), 6);
        assert!(generate_passphrase(2, "-", false, false).is_none());
        assert!(generate_passphrase(21, "-", false, false).is_none());
    }

    #[test]
    fn passphrase_can_capitalize_and_include_a_number() {
        let passphrase = generate_passphrase(6, "-", true, true).expect("valid options");
        let words: Vec<_> = passphrase.split('-').collect();
        assert_eq!(words.len(), 6);
        assert!(words
            .iter()
            .all(|word| word.chars().next().unwrap().is_uppercase()));
        assert_eq!(passphrase.chars().filter(char::is_ascii_digit).count(), 1);
    }

    #[test]
    fn sdk_generator_is_available_without_a_vault_session() {
        let generated = Client::new(None)
            .generator()
            .password(PasswordGeneratorRequest::default())
            .expect("generator does not require a vault session");
        assert_eq!(generated.len(), 16);
    }
}
