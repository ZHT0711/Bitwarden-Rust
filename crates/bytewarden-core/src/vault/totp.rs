use bitwarden_vault::{generate_totp, Totp, TotpAlgorithm};
use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use serde::Serialize;
use zeroize::Zeroizing;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TotpCode {
    pub(super) code: String,
    period: u32,
    valid_from: i64,
    valid_until: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase", tag = "status")]
pub(crate) enum VaultTotp {
    Ready { value: TotpCode },
    PremiumRequired,
    Invalid,
}

impl TotpCode {
    pub(super) fn generate(key: &str, now: DateTime<Utc>) -> Option<Self> {
        let key = key.trim();
        let steam = key
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("steam://"));
        let base32 = |secret: &str| {
            secret.bytes().all(|byte| {
                byte.is_ascii_alphabetic()
                    || byte.is_ascii_whitespace()
                    || matches!(byte, b'2'..=b'7' | b'-' | b'=')
            })
        };
        if steam {
            if !base32(&key[8..]) {
                return None;
            }
        } else if key.contains("://") {
            let uri = url::Url::parse(key).ok()?;
            if uri.scheme() != "otpauth"
                || uri.host_str() != Some("totp")
                || !uri
                    .query_pairs()
                    .find(|(name, _)| name.eq_ignore_ascii_case("secret"))
                    .is_some_and(|(_, secret)| base32(&secret))
            {
                return None;
            }
        } else if !base32(key) {
            return None;
        }
        let settings: Totp = key.parse().ok()?;
        let secret = Zeroizing::new(settings.secret);
        if secret.is_empty() || !(1..=10).contains(&settings.digits) {
            return None;
        }
        let period = i64::from(settings.period);
        let valid_from = now.timestamp().div_euclid(period) * period;
        let code = if settings.digits == 10 {
            // The SDK's u32 modulus overflows at 10 digits. The RFC's truncated
            // 31-bit value already fits in ten digits, so only padding is needed.
            ten_digit_code(
                settings.algorithm,
                &secret,
                now.timestamp().div_euclid(period),
            )?
        } else {
            generate_totp(key.to_owned(), Some(now)).ok()?.code
        };
        Some(Self {
            code,
            period: settings.period,
            valid_from,
            valid_until: valid_from + period,
        })
    }
}

fn ten_digit_code(algorithm: TotpAlgorithm, secret: &[u8], counter: i64) -> Option<String> {
    let counter = counter.to_be_bytes();
    fn digest<T: Mac>(mac: T, counter: &[u8]) -> Vec<u8> {
        mac.chain_update(counter).finalize().into_bytes().to_vec()
    }
    let hash = Zeroizing::new(match algorithm {
        TotpAlgorithm::Sha1 => digest(Hmac::<sha1::Sha1>::new_from_slice(secret).ok()?, &counter),
        TotpAlgorithm::Sha256 => {
            digest(Hmac::<sha2::Sha256>::new_from_slice(secret).ok()?, &counter)
        }
        TotpAlgorithm::Sha512 => {
            digest(Hmac::<sha2::Sha512>::new_from_slice(secret).ok()?, &counter)
        }
        TotpAlgorithm::Steam => return None,
    });
    let offset = usize::from(hash.last()? & 0x0f);
    let truncated =
        u32::from_be_bytes(hash.get(offset..offset + 4)?.try_into().ok()?) & 0x7fff_ffff;
    Some(format!("{truncated:010}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reproduces_rfc_codes_and_preserves_the_rotation_boundary() {
        let now = DateTime::from_timestamp(59, 0).unwrap();
        let key = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
        let plain = TotpCode::generate(key, now).unwrap();
        assert_eq!(plain.code, "287082");
        assert_eq!(
            (plain.period, plain.valid_from, plain.valid_until),
            (30, 30, 60)
        );
        let uri = format!("otpauth://totp/Simulation?secret={key}&digits=8");
        assert_eq!(TotpCode::generate(&uri, now).unwrap().code, "94287082");
        assert_eq!(
            TotpCode::generate(&uri.replace("digits=8", "digits=10"), now)
                .unwrap()
                .code,
            "1094287082"
        );
        let next = TotpCode::generate(key, DateTime::from_timestamp(60, 0).unwrap()).unwrap();
        assert_ne!(next.code, plain.code);
        let steam = TotpCode::generate(&format!("steam://{key}"), now).unwrap();
        assert_eq!(steam.code.len(), 5);
        assert!(TotpCode::generate("otpauth://totp/example?period=90", now).is_none());
        assert!(TotpCode::generate("otpauth://hotp/example?secret=ABCD213", now).is_none());
        assert!(TotpCode::generate("====", now).is_none());
        assert!(TotpCode::generate("ABC891!", now).is_none());
    }
}
