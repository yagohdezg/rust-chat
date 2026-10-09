/// Decode UTF-8 or UTF-16 text, returning `None` for binary content.
pub fn decode_text(bytes: &[u8]) -> Option<String> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return std::str::from_utf8(&bytes[3..]).ok().map(str::to_string);
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return decode_utf16(&bytes[2..], true);
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return decode_utf16(&bytes[2..], false);
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return Some(text.to_string());
    }
    // No BOM and not valid UTF-8: detect NUL-heavy UTF-16 (common for Windows
    // logs) by the position of the NUL bytes, then decode it.
    if bytes.len() >= 2 && bytes.len() % 2 == 0 {
        let nuls_even = bytes.iter().step_by(2).filter(|b| **b == 0).count();
        let nuls_odd = bytes.iter().skip(1).step_by(2).filter(|b| **b == 0).count();
        let pairs = bytes.len() / 2;
        if nuls_even * 4 > pairs * 3 && nuls_even > nuls_odd {
            return decode_utf16(bytes, true);
        }
        if nuls_odd * 4 > pairs * 3 && nuls_odd > nuls_even {
            return decode_utf16(bytes, false);
        }
    }
    None
}

/// Decode a UTF-16 byte stream (little-endian when `le`) into a `String`.
fn decode_utf16(bytes: &[u8], le: bool) -> Option<String> {
    if bytes.len() % 2 != 0 {
        return None;
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| {
            if le {
                u16::from_le_bytes([pair[0], pair[1]])
            } else {
                u16::from_be_bytes([pair[0], pair[1]])
            }
        })
        .collect();
    String::from_utf16(&units).ok()
}
