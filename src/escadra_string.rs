use std::fmt;

/// An union that stores either a raw 16 char string or a pointer to a raw char string.
union CharPointer {
    /// A raw 16 char string.
    /// Must be null terminated.
    chars: [u8; 16],
    /// A pointer to a raw char string.
    /// Must be null terminated.
    pointer: *mut u8,
}

/// An escadra string is a variable length string.
/// If the max-length of the escadra string (without the null terminator) exceeds 15 the string is stored as a pointer to memory.
/// Otherwise, the string is stored within the struct itself.
///
/// The string should always be null terminated.
/// The `max_length` is 15 by default.
#[repr(C)]
pub struct EscadraString {
    /// The \[u8;16\] char or the pointer to the char, depending on if max_length is either 15 or more.
    string: CharPointer,
    /// The length of the currently stored string
    length: u64,
    /// The maximum length of the string.
    /// By default it's 15 (15 for the chars + 1 for the null pointer completely filling up the default 16 char buffer).
    max_length: u64,
}

impl fmt::Debug for EscadraString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let string = self.get_string();

        f.debug_struct("EscadraString")
            .field("string", &string)
            .field("length", &self.length)
            .field("max_length", &self.max_length)
            .finish()
    }
}

impl EscadraString {
    /// Creates an empty Escadra string.
    #[must_use]
    pub fn new() -> Self {
        Self {
            string: CharPointer { chars: [b'\0'; 16] },
            length: 0,
            max_length: 15,
        }
    }

    /// Writes the given string into the `EscadraString`.
    pub fn set_string(&mut self, string: &mut str) {
        if string.len() > 15 || self.max_length > 15 {
            self.string.pointer = string.as_mut_ptr();
            self.max_length = string.len() as _;
        } else {
            let mut temp = [0u8; 16];
            temp[..string.len()].copy_from_slice(string.as_bytes());
            self.string.chars = temp;
        }

        self.length = string.len() as _;
    }

    /// Returns the string inside of the `EscadraString`.
    #[must_use]
    pub fn get_string(&self) -> &str {
        if self.max_length > 15 {
            unsafe {
                let buf: &[u8] = core::slice::from_raw_parts(self.string.pointer, self.length as _);
                return std::str::from_utf8(buf).unwrap();
            }
        }

        unsafe {
            return std::str::from_utf8(&self.string.chars[0..self.length as _]).unwrap();
        }
    }
}

impl Default for EscadraString {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_string_then_read_below_16_chars() {
        let mut es = EscadraString::new();

        let mut string = "Banana".to_string();

        es.set_string(string.as_mut_str());

        let result = es.get_string();

        assert_eq!(string, result);
    }

    #[test]
    fn set_string_below_16_chars_twice() {
        let mut es = EscadraString::new();

        let mut string = "Banana".to_string();

        es.set_string(string.as_mut_str());
        let result = es.get_string();
        assert_eq!(string, result);

        es.set_string(string.as_mut_str());
        let result = es.get_string();
        assert_eq!(string, result);
    }

    #[test]
    fn set_string_then_read_above_16_chars() {
        let mut es = EscadraString::new();

        let mut string = "Banana Banana Banana Banana".to_string();

        es.set_string(&mut string);

        let result = es.get_string();

        assert_eq!(string, result);
    }

    #[test]
    fn set_string_above_16_chars_twice() {
        let mut es = EscadraString::new();

        let mut string = "Banana Banana Banana Banana".to_string();
        es.set_string(&mut string);
        let result = es.get_string();
        assert_eq!(string, result);

        es.set_string(&mut string);
        let result = es.get_string();
        assert_eq!(string, result);
    }

    #[test]
    fn set_large_then_set_small_then_read() {
        let mut es = EscadraString::new();

        let mut long_string = "Banana Banana Banana Banana".to_string();
        let mut short_string = "Banana".to_string();

        es.set_string(&mut long_string);
        es.set_string(&mut short_string);

        let result = es.get_string();

        assert_eq!(short_string, result);
    }
}
