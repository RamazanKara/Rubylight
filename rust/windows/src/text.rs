//! UTF-16 strings for Win32 calls.

/// `text` as a NUL-terminated UTF-16 buffer for a `PCWSTR` argument.
pub(crate) fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// [`to_wide`] of a fixed name, such as an AMF property set on every frame,
/// converted once: each name is kept for the life of the process, so pass
/// only names from a fixed set, never text built at run time.
pub(crate) fn static_wide(name: &'static str) -> &'static [u16] {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static NAMES: OnceLock<Mutex<HashMap<&'static str, &'static [u16]>>> = OnceLock::new();
    let mut names = NAMES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    names
        .entry(name)
        .or_insert_with(|| Vec::leak(to_wide(name)))
}

/// The string in a fixed-size UTF-16 buffer, up to its first NUL.
/// Unpaired surrogates become U+FFFD.
pub(crate) fn from_wide(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_a_fixed_buffer() {
        let mut buffer = [0u16; 16];
        let wide = to_wide("Radeon");
        assert_eq!(wide.last(), Some(&0));
        buffer[..wide.len()].copy_from_slice(&wide);
        assert_eq!(from_wide(&buffer), "Radeon");
        assert_eq!(from_wide(&wide[..3]), "Rad");
    }

    #[test]
    fn a_fixed_name_is_converted_once() {
        let first = static_wide("HevcOutputDataType");
        assert_eq!(first, &to_wide("HevcOutputDataType")[..]);
        assert!(std::ptr::eq(first, static_wide("HevcOutputDataType")));
        assert_eq!(
            static_wide("OutputDataType"),
            &to_wide("OutputDataType")[..]
        );
    }
}
