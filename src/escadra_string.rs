use std::{mem::ManuallyDrop, str::Utf8Error};

union CharPointer {
    chars: [u8; 16],
    pointer: ManuallyDrop<Box<str>>,
}

#[repr(C)]
pub struct EscadraString {
    string: CharPointer,
    length: u64,
    max_length: u64,
}

impl EscadraString {
    pub fn new() -> Self {
        Self {
            string: CharPointer { chars: [b'\0'; 16] },
            length: 0,
            max_length: 15,
        }
    }

    pub fn set_string(&mut self, string: &str) {
        if string.len() > 15 {
            self.string.pointer = ManuallyDrop::new((*string).into());
            self.max_length = string.len() as _;
        } else {
            let mut temp = [0u8; 16];
            temp[..string.len()].copy_from_slice(string.as_bytes());
            self.string.chars = temp;
            // self.string.chars = string.as_bytes().try_into().unwrap_or_else(|x| {
            //     println!("{x}");
            //     [b'\0'; 16]
            // });
        }

        self.length = string.len() as _;
    }

    pub fn get_string(&self) -> Result<&str, Utf8Error> {
        if self.max_length > 15 {
            unsafe {
                return std::str::from_utf8(
                    &self.string.pointer.as_bytes()[0..self.length.try_into().unwrap()],
                );
            }
        }

        unsafe {
            return std::str::from_utf8(&self.string.chars[0..self.length.try_into().unwrap()]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_string_then_read_below_16_chars() {
        let mut es = EscadraString::new();

        let string = "Banana";

        es.set_string(string);

        let result = es.get_string().unwrap();

        assert_eq!(string, result);
    }

    #[test]
    fn set_string_then_read_above_16_chars() {
        let mut es = EscadraString::new();

        let string = "Banana Banana Banana Banana";

        es.set_string(string);

        let result = es.get_string().unwrap();

        assert_eq!(string, result);
    }
}
