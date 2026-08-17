use serde::de::DeserializeOwned;

const UTF8_BOM: &[u8] = &[0xef, 0xbb, 0xbf];

pub fn strip_utf8_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(UTF8_BOM).unwrap_or(bytes)
}

#[cfg(test)]
pub fn strip_utf8_bom_str(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

pub fn from_slice<T: DeserializeOwned>(bytes: &[u8]) -> serde_json::Result<T> {
    serde_json::from_slice(strip_utf8_bom(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    #[test]
    fn parses_json_with_or_without_a_single_utf8_bom() {
        let plain: Value = from_slice(br#"{"ok":true}"#).unwrap();
        let with_bom: Value = from_slice(b"\xef\xbb\xbf{\"ok\":true}").unwrap();
        let string_with_bom: Value =
            serde_json::from_str(strip_utf8_bom_str("\u{feff}{\"ok\":true}")).unwrap();

        assert_eq!(plain, json!({"ok": true}));
        assert_eq!(with_bom, plain);
        assert_eq!(string_with_bom, plain);
    }

    #[test]
    fn keeps_invalid_json_invalid_after_bom_normalization() {
        assert!(from_slice::<Value>(b"\xef\xbb\xbf{invalid").is_err());
        assert!(serde_json::from_str::<Value>(strip_utf8_bom_str("\u{feff}{invalid")).is_err());
    }
}
