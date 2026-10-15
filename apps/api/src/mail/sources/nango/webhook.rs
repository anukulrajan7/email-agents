use super::*;

/// `X-Nango-Hmac-Sha256` = hex(HMAC-SHA256(signing key, raw body)). `secrets` may hold
/// several comma-separated keys to cover Nango's key-rotation window.
pub fn verify_webhook(secrets: &str, headers: &HeaderMap, body: &[u8]) -> bool {
    let Some(given) = headers.get("x-nango-hmac-sha256").and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let Some(given) = decode_hex(given.trim()) else {
        return false;
    };
    secrets.split(',').map(str::trim).filter(|s| !s.is_empty()).any(|secret| {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
        mac.update(body);
        mac.finalize().into_bytes().ct_eq(&given).into()
    })
}

pub fn sign_webhook(secret: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(body);
    mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn webhook_signature_with_rotation() {
        let body = br#"{"type":"sync"}"#;
        let mut h = HeaderMap::new();
        h.insert("x-nango-hmac-sha256", HeaderValue::from_str(&sign_webhook("new", body)).unwrap());
        assert!(verify_webhook("old, new", &h, body));
        assert!(!verify_webhook("old", &h, body));
        assert!(!verify_webhook("new", &h, b"{}"));
        assert!(!verify_webhook("new", &HeaderMap::new(), body));
    }
}
