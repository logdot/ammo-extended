//! Replaces FireShell's hardcoded rocket lifetimes with the ammo's configured lifetime.
//!
//! Cases 51, 52, 54 and 55 each assign a constant to three fields of the newly
//! created missile `Body` with three consecutive `MOV [reg + offset], imm32`
//! instructions. Each block is replaced with a call to [`set_missile_ttl`].

use patchy::{PatchSession, ProcessModule, ReturnType, Trampoline};

/// Size of the three consecutive `MOV dword ptr [reg + disp32], imm32` instructions.
const REPLACED_SIZE: usize = 30;

/// A single hardcoded lifetime assignment block in `FireShell`.
struct Site {
    /// Ammo behavior this block belongs to; informational only.
    behavior: u32,
    rva: usize,
    /// Encoding of `MOV RCX, <missile body register>`.
    load_body: [u8; 3],
}

const MOV_RCX_RBX: [u8; 3] = [0x48, 0x89, 0xd9];
const MOV_RCX_RSI: [u8; 3] = [0x48, 0x89, 0xf1];
const MOV_RCX_R14: [u8; 3] = [0x4c, 0x89, 0xf1];
/// `R15` holds the selected `Ammo*` throughout cases 51-55 in both versions.
const MOV_RDX_R15: [u8; 3] = [0x4c, 0x89, 0xfa];

#[cfg(feature = "1_151")]
const SITES: &[Site] = &[
    Site {
        behavior: 51,
        rva: 0x3135a,
        load_body: MOV_RCX_RBX,
    },
    Site {
        behavior: 52,
        rva: 0x31895,
        load_body: MOV_RCX_R14,
    },
    Site {
        behavior: 55,
        rva: 0x31e68,
        load_body: MOV_RCX_R14,
    },
];

#[cfg(not(feature = "1_151"))]
const SITES: &[Site] = &[
    Site {
        behavior: 51,
        rva: 0x30d82,
        load_body: MOV_RCX_RBX,
    },
    Site {
        behavior: 52,
        rva: 0x312b2,
        load_body: MOV_RCX_RSI,
    },
    Site {
        behavior: 54,
        rva: 0x318c2,
        load_body: MOV_RCX_RBX,
    },
    Site {
        behavior: 55,
        rva: 0x31b34,
        load_body: MOV_RCX_RSI,
    },
];

/// Offset of the body's time-to-live field.
#[cfg(feature = "1_151")]
const BODY_TTL: usize = 0x2b0;
#[cfg(not(feature = "1_151"))]
const BODY_TTL: usize = 0x2c0;
/// Two further fields vanilla sets to the same value as the body's TTL.
const BODY_FIELD_F4: usize = 0xf4;
const BODY_DESOWNING_TIME: usize = 0xf0;

/// 1.151 has no native ammo lifespan; config loading stores it in `padding_164h`.
#[cfg(feature = "1_151")]
const AMMO_TTL: usize = 0x164;
#[cfg(not(feature = "1_151"))]
const AMMO_TTL: usize = 0x16c;

/// Called in place of the vanilla constant assignments.
///
/// # Safety
///
/// `body` and `ammo` must point to a live `Body` and `Ammo` respectively.
unsafe extern "C" fn set_missile_ttl(body: *mut u8, ammo: *const u8) {
    let ttl = ammo.add(AMMO_TTL).cast::<f32>().read_unaligned();
    for offset in [BODY_TTL, BODY_FIELD_F4, BODY_DESOWNING_TIME] {
        body.add(offset).cast::<f32>().write_unaligned(ttl);
    }
}

/// Prepares a lifetime hook at every missile site.
///
/// # Safety
///
/// Must run against the game version selected by the `1_151` feature.
pub unsafe fn patch(session: &mut PatchSession, module: ProcessModule) -> patchy::Result {
    for site in SITES {
        let address = module.resolve_rva(site.rva)?;
        let mut argument_setup = site.load_body.to_vec();
        argument_setup.extend_from_slice(&MOV_RDX_R15);

        let mut trampoline = Trampoline::new();
        trampoline.preserved_call(
            set_missile_ttl as *const (),
            &argument_setup,
            &[],
            ReturnType::None,
        );
        trampoline.relative_jump(address + REPLACED_SIZE);
        session.detour_trampoline(address, REPLACED_SIZE, trampoline)?;
        log::debug!("Prepared missile TTL hook for behavior {}", site.behavior);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_copies_the_ammo_lifetime_to_exactly_three_body_fields() {
        for ttl in [0.0f32, 2.5, 10.0, 25.0] {
            let mut ammo = [0u8; 0x188];
            ammo[AMMO_TTL..AMMO_TTL + 4].copy_from_slice(&ttl.to_le_bytes());
            let mut body = [0xa5u8; 0x400];
            let mut expected = body;
            for offset in [BODY_TTL, BODY_FIELD_F4, BODY_DESOWNING_TIME] {
                expected[offset..offset + 4].copy_from_slice(&ttl.to_le_bytes());
            }
            unsafe { set_missile_ttl(body.as_mut_ptr(), ammo.as_ptr()) };
            assert_eq!(body, expected);
        }
    }

    #[test]
    fn site_table_covers_the_expected_behaviors() {
        let behaviors: Vec<u32> = SITES.iter().map(|site| site.behavior).collect();
        #[cfg(feature = "1_151")]
        assert_eq!(behaviors, [51, 52, 55]);
        #[cfg(not(feature = "1_151"))]
        assert_eq!(behaviors, [51, 52, 54, 55]);
    }

    #[test]
    fn register_moves_encode_the_intended_instructions() {
        // MOV RCX, RBX / RSI / R14 and MOV RDX, R15 (REX.W 89 /r, mod=11).
        assert_eq!(MOV_RCX_RBX, [0x48, 0x89, 0xd9]);
        assert_eq!(MOV_RCX_RSI, [0x48, 0x89, 0xf1]);
        assert_eq!(MOV_RCX_R14, [0x4c, 0x89, 0xf1]);
        assert_eq!(MOV_RDX_R15, [0x4c, 0x89, 0xfa]);
    }
}
