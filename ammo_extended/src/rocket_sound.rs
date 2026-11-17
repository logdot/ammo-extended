//! Routes `FireShell`'s hardcoded rocket launch sound through the ammo's
//! configured `shell_out` / `shell_enemy` / `shell_far` sound sets.
//!
//! Cannon shells get their firing sound from `Body_InitializeCannonShell`,
//! which picks `shell_out` for the player's own close shot, `shell_far` when
//! zoomed out and `shell_enemy` for an enemy shot. The rocket `FireShell` cases
//! bypass that helper and build one of two literal names, `missile_launch` or
//! `missile_launch_far`.
//!
//! The two shared sound blocks merge their close/far branches before calling
//! `Sound_BuildName`, so the hook replaces those calls. `R15` holds the
//! selected `Ammo*` from the dispatch until each block overwrites it for a
//! particle argument; small stash hooks just before each overwrite record it
//! in [`CURRENT_AMMO`].
//!
//! `shell_in` is not handled here: the game already reads it when a round is
//! loaded into a magazine, and rockets use that same path.

use std::sync::atomic::{AtomicUsize, Ordering};

use highfleet::general::EscadraString;
use patchy::{PatchError, PatchSession, ProcessModule, ReturnType, Trampoline};

use crate::Ammo;

/// Game function that formats `<number><infix><sound set>` into its global
/// string and returns the `char*` the caller hands to `playSFXSound_`.
#[cfg(feature = "1_151")]
const BUILD_NAME_RVA: usize = 0x1a2080;
#[cfg(not(feature = "1_151"))]
const BUILD_NAME_RVA: usize = 0x1b1fb0;

/// Global zoom level compared against 3 by the sound blocks.
#[cfg(feature = "1_151")]
const ZOOM_LEVEL_RVA: usize = 0x39115c;
#[cfg(not(feature = "1_151"))]
const ZOOM_LEVEL_RVA: usize = 0x3c11cc;

/// Variant count used for the vanilla `missile_launch_01..02` sounds when no
/// ammo has been recorded for the shot.
const VANILLA_ROCKET_VARIANTS: u32 = 2;
/// `shell_out` sets have three variants in vanilla.
const SHELL_OUT_VARIANTS: u32 = 3;
/// `shell_far` / `shell_enemy` sets have four variants in vanilla.
const SHELL_DISTANT_VARIANTS: u32 = 4;

/// A `CALL Sound_BuildName` site inside `FireShell`.
struct SoundSite {
    rva: usize,
    original: [u8; 5],
}

/// A `MOVSS [RSP + 0x38], XMM0` site immediately before `R15` is overwritten.
struct StashSite {
    rva: usize,
    original: [u8; 6],
}

#[cfg(feature = "1_151")]
const SOUND_SITES: &[SoundSite] = &[
    SoundSite {
        rva: 0x3179e,
        original: [0xe8, 0xdd, 0x08, 0x17, 0x00],
    },
    SoundSite {
        rva: 0x31d7a,
        original: [0xe8, 0x01, 0x03, 0x17, 0x00],
    },
];

#[cfg(not(feature = "1_151"))]
const SOUND_SITES: &[SoundSite] = &[
    SoundSite {
        rva: 0x311bb,
        original: [0xe8, 0xf0, 0x0d, 0x18, 0x00],
    },
    SoundSite {
        rva: 0x317d4,
        original: [0xe8, 0xd7, 0x07, 0x18, 0x00],
    },
];

#[cfg(feature = "1_151")]
const STASH_SITES: &[StashSite] = &[
    StashSite {
        rva: 0x31623,
        original: [0xf3, 0x0f, 0x11, 0x44, 0x24, 0x38],
    },
    StashSite {
        rva: 0x31c35,
        original: [0xf3, 0x0f, 0x11, 0x44, 0x24, 0x38],
    },
];

#[cfg(not(feature = "1_151"))]
const STASH_SITES: &[StashSite] = &[
    StashSite {
        rva: 0x3104a,
        original: [0xf3, 0x0f, 0x11, 0x44, 0x24, 0x38],
    },
    StashSite {
        rva: 0x3168f,
        original: [0xf3, 0x0f, 0x11, 0x44, 0x24, 0x38],
    },
];

/// `MOV RCX, R15`: pass the selected ammo to [`record_ammo`].
const STASH_ARGUMENT_SETUP: [u8; 3] = [0x4c, 0x89, 0xf9];
/// `MOVZX EDX, R12B`: pass the player-ship flag to the sound callback.
const SOUND_ARGUMENT_SETUP: [u8; 4] = [0x41, 0x0f, 0xb6, 0xd4];

/// Runtime address of the game's name builder, resolved during installation.
static BUILD_NAME: AtomicUsize = AtomicUsize::new(0);
/// Runtime address of the zoom-level global, resolved during installation.
static ZOOM_LEVEL: AtomicUsize = AtomicUsize::new(0);
/// `Ammo*` selected by the current `FireShell` rocket case.
static CURRENT_AMMO: AtomicUsize = AtomicUsize::new(0);

type BuildName = unsafe extern "C" fn(*const u8, u32, u32, *const u8) -> *mut u8;

/// Picks the sound set for a rocket shot using the same split as cannon
/// shells: player close, distant and enemy.
fn select(ammo: &Ammo, player: bool, far: bool) -> (&EscadraString, u32) {
    match (player, far) {
        (true, false) => (&ammo.shell_out, SHELL_OUT_VARIANTS),
        // 1.151 has no shell_enemy, so distant/non-player shots use shell_far.
        #[cfg(feature = "1_151")]
        (false, _) | (true, true) => (&ammo.shell_far, SHELL_DISTANT_VARIANTS),
        #[cfg(not(feature = "1_151"))]
        (false, false) => (&ammo.shell_enemy, SHELL_DISTANT_VARIANTS),
        #[cfg(not(feature = "1_151"))]
        (true, true) | (false, true) => (&ammo.shell_far, SHELL_DISTANT_VARIANTS),
    }
}

/// Records the `Ammo*` `FireShell` is about to stop keeping in `R15`.
///
/// # Safety
///
/// `ammo` must be the live `Ammo` record selected by the current `FireShell`.
unsafe extern "C" fn record_ammo(ammo: *const u8) {
    CURRENT_AMMO.store(ammo as usize, Ordering::Relaxed);
}

/// Called in place of `Sound_BuildName` at the two rocket sound blocks.
///
/// # Safety
///
/// `base`/`infix` must be NUL-terminated strings and `CURRENT_AMMO` must refer
/// to a live ammo record. `BUILD_NAME`/`ZOOM_LEVEL` must have been resolved by
/// [`patch`].
unsafe extern "C" fn build_rocket_sound(
    base: *const u8,
    player: u32,
    _unused: u32,
    infix: *const u8,
) -> *mut u8 {
    let build: BuildName = std::mem::transmute(BUILD_NAME.load(Ordering::Relaxed));

    let ammo = CURRENT_AMMO.load(Ordering::Relaxed);
    if ammo == 0 || base.is_null() || infix.is_null() {
        return build(base, 1, VANILLA_ROCKET_VARIANTS, infix);
    }

    let far = *(ZOOM_LEVEL.load(Ordering::Relaxed) as *const i32) > 3;
    let ammo = &*(ammo as *const Ammo);
    let (field, variants) = select(ammo, player != 0, far);
    build(field.get_string().as_ptr(), 1, variants, infix)
}

/// Redirects the FireShell rocket sound-name builders to the ammo's fields.
///
/// # Safety
///
/// Must run against the game version selected by the `1_151` feature.
pub unsafe fn patch(session: &mut PatchSession, module: ProcessModule) -> patchy::Result {
    BUILD_NAME.store(
        module.resolve_rva(BUILD_NAME_RVA)? as usize,
        Ordering::Relaxed,
    );
    ZOOM_LEVEL.store(
        module.resolve_rva(ZOOM_LEVEL_RVA)? as usize,
        Ordering::Relaxed,
    );

    for site in STASH_SITES {
        let address = module.resolve_rva(site.rva)?;
        let actual = std::slice::from_raw_parts(address as *const u8, site.original.len());
        if actual != site.original {
            return Err(PatchError::SourceChanged { address });
        }

        let mut trampoline = Trampoline::new();
        trampoline.preserved_call(
            record_ammo as *const (),
            &STASH_ARGUMENT_SETUP,
            &[],
            ReturnType::None,
        );
        trampoline.bytes(&site.original);
        trampoline.relative_jump(address + site.original.len());
        session.detour_trampoline(address, site.original.len(), trampoline)?;
        log::debug!("Prepared rocket ammo stash at {address:#x}");
    }

    for site in SOUND_SITES {
        let address = module.resolve_rva(site.rva)?;
        let actual = std::slice::from_raw_parts(address as *const u8, site.original.len());
        if actual != site.original {
            return Err(PatchError::SourceChanged { address });
        }

        let mut trampoline = Trampoline::new();
        trampoline.preserved_call(
            build_rocket_sound as *const (),
            &SOUND_ARGUMENT_SETUP,
            &[],
            ReturnType::Rax,
        );
        trampoline.relative_jump(address + site.original.len());
        session.detour_trampoline(address, site.original.len(), trampoline)?;
        log::debug!("Prepared rocket firing sound hook at {address:#x}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ammo() -> Ammo {
        // All sound strings start out empty and SSO; dropping the record frees
        // nothing for those fields and frees only the long strings set below.
        let mut ammo: Ammo = unsafe { std::mem::zeroed() };
        ammo.shell_in = EscadraString::from("shell_in_med");
        ammo.shell_out = EscadraString::from("shell_out_med");
        ammo.shell_far = EscadraString::from("shell_out_med_far");
        #[cfg(not(feature = "1_151"))]
        {
            ammo.shell_enemy = EscadraString::from("shell_out_enemy_med");
        }
        ammo
    }

    #[test]
    fn player_close_shot_uses_shell_out_with_three_variants() {
        let ammo = ammo();
        let (field, variants) = select(&ammo, true, false);
        assert_eq!(field.get_string(), "shell_out_med");
        assert_eq!(variants, 3);
    }

    #[test]
    fn player_distant_shot_uses_shell_far_with_four_variants() {
        let ammo = ammo();
        let (field, variants) = select(&ammo, true, true);
        assert_eq!(field.get_string(), "shell_out_med_far");
        assert_eq!(variants, 4);
    }

    #[cfg(not(feature = "1_151"))]
    #[test]
    fn enemy_close_shot_uses_shell_enemy() {
        let ammo = ammo();
        let (field, variants) = select(&ammo, false, false);
        assert_eq!(field.get_string(), "shell_out_enemy_med");
        assert_eq!(variants, 4);
    }

    #[cfg(feature = "1_151")]
    #[test]
    fn enemy_close_shot_falls_back_to_shell_far() {
        let ammo = ammo();
        let (field, variants) = select(&ammo, false, false);
        assert_eq!(field.get_string(), "shell_out_med_far");
        assert_eq!(variants, 4);
    }

    #[test]
    fn enemy_distant_shot_uses_shell_far() {
        let ammo = ammo();
        let (field, variants) = select(&ammo, false, true);
        assert_eq!(field.get_string(), "shell_out_med_far");
        assert_eq!(variants, 4);
    }

    #[test]
    fn empty_sound_field_is_selected_like_any_other_value() {
        let mut ammo: Ammo = unsafe { std::mem::zeroed() };
        ammo.shell_out = EscadraString::from("");
        let (field, variants) = select(&ammo, true, false);
        assert_eq!(field.get_string(), "");
        assert_eq!(variants, 3);
    }

    #[test]
    fn long_sound_names_are_read_through_the_heap_pointer() {
        let mut ammo: Ammo = unsafe { std::mem::zeroed() };
        let name = "shell_out_enemy_very_long_name";
        ammo.shell_out = EscadraString::from(name);
        let (field, _) = select(&ammo, true, false);
        assert_eq!(field.get_string(), name);
    }

    #[test]
    fn shell_in_is_not_part_of_the_firing_selection() {
        let ammo = ammo();
        for (player, far) in [(true, false), (true, true), (false, false), (false, true)] {
            assert_ne!(select(&ammo, player, far).0.get_string(), "shell_in_med");
        }
    }

    #[test]
    fn register_moves_encode_the_intended_instructions() {
        // MOVZX EDX, R12B (REX.B 0F B6 /r).
        assert_eq!(SOUND_ARGUMENT_SETUP, [0x41, 0x0f, 0xb6, 0xd4]);
        // MOV RCX, R15 (REX.WRB 89 /r).
        assert_eq!(STASH_ARGUMENT_SETUP, [0x4c, 0x89, 0xf9]);
    }

    #[test]
    fn site_tables_match_the_expected_sizes_and_opcodes() {
        assert_eq!(SOUND_SITES.len(), 2);
        assert!(SOUND_SITES.iter().all(|site| site.original[0] == 0xe8));
        assert_eq!(STASH_SITES.len(), 2);
        assert!(STASH_SITES
            .iter()
            .all(|site| { site.original == [0xf3, 0x0f, 0x11, 0x44, 0x24, 0x38] }));
    }
}
