use std::fmt;

union CharPointer {
    chars: [u8; 16],
    pointer: *mut u8,
}

#[repr(C)]
pub struct EscadraString {
    string: CharPointer,
    length: u64,
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
    pub fn new() -> Self {
        Self {
            string: CharPointer { chars: [b'\0'; 16] },
            length: 0,
            max_length: 15,
        }
    }

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
