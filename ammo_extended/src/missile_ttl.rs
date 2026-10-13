//! In-place replacement of FireShell's three missile lifetime assignments.

pub struct TtlPatch {
    pub rva: usize,
    pub original: [u8; 30],
    pub replacement: [u8; 30],
}

pub fn patch(steam_1_151: bool) -> TtlPatch {
    let (rva, ammo_ttl_offset, body_ttl_offset) = if steam_1_151 {
        // 1.151 has no native ammo TTL; config loading stores it in padding_164h.
        (0x3135a, 0x164u32, 0x2b0u32)
    } else {
        (0x30d82, 0x16cu32, 0x2c0u32)
    };
    let offsets = [body_ttl_offset, 0xf4, 0xf0];
    let mut original = [0; 30];
    for (instruction, offset) in original.chunks_exact_mut(10).zip(offsets) {
        instruction[..2].copy_from_slice(&[0xc7, 0x83]); // MOV [RBX + offset], 10.0
        instruction[2..6].copy_from_slice(&offset.to_le_bytes());
        instruction[6..].copy_from_slice(&10.0f32.to_bits().to_le_bytes());
    }

    // R15 is the selected Ammo*, RBX is the newly created missile Body*.
    // EAX is dead here and is overwritten by subsequent calls before being read.
    let mut replacement = [0x90; 30];
    replacement[..3].copy_from_slice(&[0x41, 0x8b, 0x87]); // MOV EAX, [R15 + ttl]
    replacement[3..7].copy_from_slice(&ammo_ttl_offset.to_le_bytes());
    for (instruction, offset) in replacement[7..25].chunks_exact_mut(6).zip(offsets) {
        instruction[..2].copy_from_slice(&[0x89, 0x83]); // MOV [RBX + offset], EAX
        instruction[2..].copy_from_slice(&offset.to_le_bytes());
    }
    TtlPatch {
        rva,
        original,
        replacement,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_bytes_match_saved_listings() {
        for (version, ttl_offset) in [(true, 0x2b0u32), (false, 0x2c0u32)] {
            let patch = patch(version);
            let mut expected = vec![];
            for offset in [ttl_offset, 0xf4, 0xf0] {
                expected.extend([0xc7, 0x83]);
                expected.extend(offset.to_le_bytes());
                expected.extend([0x00, 0x00, 0x20, 0x41]);
            }
            assert_eq!(patch.original.as_slice(), expected);
        }
        assert_eq!(patch(true).rva, 0x3135a);
        assert_eq!(patch(false).rva, 0x30d82);
    }

    #[test]
    fn replacements_copy_configured_ttl_to_only_the_three_lifetime_fields() {
        for (version, ammo_offset, body_offset) in
            [(true, 0x164usize, 0x2b0usize), (false, 0x16c, 0x2c0)]
        {
            for ttl in [0.0f32, 2.5, 10.0, 25.0] {
                let code = patch(version).replacement;
                let mut ammo = [0u8; 0x188];
                ammo[ammo_offset..ammo_offset + 4].copy_from_slice(&ttl.to_le_bytes());
                let mut body = [0xa5u8; 0x400];
                let mut expected = body;
                for offset in [body_offset, 0xf4, 0xf0] {
                    expected[offset..offset + 4].copy_from_slice(&ttl.to_le_bytes());
                }
                // Decode the restricted MOV sequence and emulate its memory writes.
                assert_eq!(&code[..3], &[0x41, 0x8b, 0x87]);
                let source = u32::from_le_bytes(code[3..7].try_into().unwrap()) as usize;
                let eax = &ammo[source..source + 4];
                for instruction in code[7..25].chunks_exact(6) {
                    assert_eq!(&instruction[..2], &[0x89, 0x83]);
                    let target = u32::from_le_bytes(instruction[2..].try_into().unwrap()) as usize;
                    body[target..target + 4].copy_from_slice(eax);
                }
                assert_eq!(body, expected);
                assert_eq!(&code[25..], &[0x90; 5]);
            }
        }
    }
}
