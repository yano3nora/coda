//! Clipboard transport helpers.
//!
//! Internal clipboard state lives in the event loop. This module only formats
//! terminal clipboard writes (OSC 52) and implements the tiny RFC 4648 base64
//! subset needed for that transport, avoiding a new dependency for MVP scope.

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Maximum payload sent through OSC 52. Larger copies still update the internal
/// clipboard, but are not sent to the terminal to avoid freezing or truncation
/// on terminals with strict clipboard limits.
pub const OSC52_MAX_BYTES: usize = 1024 * 1024;

/// Encodes bytes using standard RFC 4648 base64 with padding.
pub fn base64_encode(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);

    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);

        encoded.push(BASE64[(first >> 2) as usize] as char);
        encoded.push(BASE64[(((first & 0b0000_0011) << 4) | (second >> 4)) as usize] as char);

        if chunk.len() > 1 {
            encoded.push(BASE64[(((second & 0b0000_1111) << 2) | (third >> 6)) as usize] as char);
        } else {
            encoded.push('=');
        }

        if chunk.len() > 2 {
            encoded.push(BASE64[(third & 0b0011_1111) as usize] as char);
        } else {
            encoded.push('=');
        }
    }

    encoded
}

/// Formats an OSC 52 clipboard write for the default clipboard selection.
pub fn osc52_copy_sequence(text: &str) -> Option<Vec<u8>> {
    if text.len() > OSC52_MAX_BYTES {
        return None;
    }
    Some(format!("\x1b]52;c;{}\x07", base64_encode(text.as_bytes())).into_bytes())
}

#[cfg(test)]
mod tests {
    use super::{base64_encode, osc52_copy_sequence};

    #[test]
    fn base64_rfc4648_vectors_and_utf8_bytes() {
        let cases = [
            (b"".as_slice(), ""),
            (b"f".as_slice(), "Zg=="),
            (b"fo".as_slice(), "Zm8="),
            (b"foo".as_slice(), "Zm9v"),
            ("日本語".as_bytes(), "5pel5pys6Kqe"),
        ];

        for (input, expected) in cases {
            assert_eq!(base64_encode(input), expected);
        }
    }

    #[test]
    fn osc52_output_format_wraps_base64_payload() {
        assert_eq!(osc52_copy_sequence("foo").unwrap(), b"\x1b]52;c;Zm9v\x07");
    }
}
