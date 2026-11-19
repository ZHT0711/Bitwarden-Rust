use std::ffi::{c_char, CString};

use rand::{rngs::OsRng, seq::SliceRandom, Rng};

const LOWERCASE: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
const UPPERCASE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const NUMBERS: &[u8] = b"0123456789";
const SYMBOLS: &[u8] = b"!@#$%^&*";
const AMBIGUOUS: &[u8] = b"iIlLoO01";

fn generate_password(
    length: usize,
    options: u32,
    min_numbers: usize,
    min_special: usize,
) -> Option<String> {
    let length = length.clamp(5, 128);
    let include_uppercase = options & 1 != 0;
    let include_lowercase = options & 2 != 0;
    let include_numbers = options & 4 != 0;
    let include_special = options & 8 != 0;
    let avoid_ambiguous = options & 16 != 0;

    let mut classes: Vec<(&[u8], usize)> = Vec::with_capacity(4);
    if include_uppercase {
        classes.push((UPPERCASE, 1));
    }
    if include_lowercase {
        classes.push((LOWERCASE, 1));
    }
    if include_numbers {
        classes.push((NUMBERS, min_numbers));
    }
    if include_special {
        classes.push((SYMBOLS, min_special));
    }
    if classes.is_empty() {
        return None;
    }

    let filter_ambiguous = |class: &[u8]| -> Vec<u8> {
        class
            .iter()
            .copied()
            .filter(|character| !avoid_ambiguous || !AMBIGUOUS.contains(character))
            .collect()
    };
    let filtered_classes: Vec<(Vec<u8>, usize)> = classes
        .iter()
        .map(|(class, minimum)| (filter_ambiguous(class), *minimum))
        .collect();
    if filtered_classes.iter().any(|(class, _)| class.is_empty()) {
        return None;
    }

    let alphabet: Vec<u8> = filtered_classes
        .iter()
        .flat_map(|(class, _)| class.iter().copied())
        .collect();
    let required_count: usize = filtered_classes.iter().map(|(_, minimum)| minimum).sum();
    if required_count > length {
        return None;
    }

    let mut rng = OsRng;
    let mut result = Vec::with_capacity(length);

    for (class, required_count) in &filtered_classes {
        for _ in 0..*required_count {
            result.push(class[rng.gen_range(0..class.len())]);
        }
    }
    while result.len() < length {
        result.push(alphabet[rng.gen_range(0..alphabet.len())]);
    }
    result.shuffle(&mut rng);
    Some(String::from_utf8(result).expect("password alphabet is valid UTF-8"))
}

#[no_mangle]
pub extern "C" fn bw_core_version() -> *const c_char {
    b"0.1.0\0".as_ptr().cast()
}

#[no_mangle]
pub extern "C" fn bw_generate_password(
    length: u32,
    options: u32,
    min_numbers: u32,
    min_special: u32,
) -> *mut c_char {
    let Some(password) = generate_password(
        length as usize,
        options,
        min_numbers as usize,
        min_special as usize,
    ) else {
        return std::ptr::null_mut();
    };
    CString::new(password)
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "C" fn bw_string_free(value: *mut c_char) {
    if !value.is_null() {
        unsafe { drop(CString::from_raw(value)) };
    }
}

#[cfg(test)]
mod tests {
    use super::{generate_password, AMBIGUOUS, LOWERCASE, NUMBERS, SYMBOLS, UPPERCASE};

    #[test]
    fn generated_password_obeys_length_and_character_options() {
        let password = generate_password(24, 1 | 2 | 4 | 8, 2, 2).expect("valid options");
        assert_eq!(password.len(), 24);
        assert!(password.bytes().any(|byte| LOWERCASE.contains(&byte)));
        assert!(password.bytes().any(|byte| UPPERCASE.contains(&byte)));
        assert!(
            password
                .bytes()
                .filter(|byte| NUMBERS.contains(byte))
                .count()
                >= 2
        );
        assert!(
            password
                .bytes()
                .filter(|byte| SYMBOLS.contains(byte))
                .count()
                >= 2
        );
    }

    #[test]
    fn generated_password_clamps_lengths_and_respects_ambiguous_setting() {
        let password = generate_password(1, 1 | 2 | 16, 0, 0).expect("valid options");
        assert_eq!(password.len(), 5);
        assert!(!password.bytes().any(|byte| AMBIGUOUS.contains(&byte)));
        assert_eq!(
            generate_password(1000, 1, 0, 0)
                .expect("valid options")
                .len(),
            128
        );
    }

    #[test]
    fn impossible_generator_settings_are_rejected() {
        assert!(generate_password(5, 1 | 2 | 4, 4, 0).is_none());
        assert!(generate_password(20, 0, 0, 0).is_none());
    }
}
