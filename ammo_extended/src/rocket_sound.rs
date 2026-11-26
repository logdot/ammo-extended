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
use patchy::{
    CallBridge, DetourSite, FlagPolicy, KeepReturn, PatchSession, ProcessModule, StackAlignment,
    Trampoline,
};

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

/// Length of the `CALL Sound_BuildName` replaced inside `FireShell`.
const SOUND_CALL_LEN: usize = 5;

/// Length of the `MOVSS [RSP + 0x38], XMM0` replayed before `R15` is overwritten.
const STASH_INSTRUCTION_LEN: usize = 6;

#[cfg(feature = "1_151")]
const SOUND_SITES: [usize; 2] = [0x3179e, 0x31d7a];

#[cfg(not(feature = "1_151"))]
const SOUND_SITES: [usize; 2] = [0x311bb, 0x317d4];

#[cfg(feature = "1_151")]
const STASH_SITES: [usize; 2] = [0x31623, 0x31c35];

#[cfg(not(feature = "1_151"))]
const STASH_SITES: [usize; 2] = [0x3104a, 0x3168f];

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

/// Records the ammo, then runs the displaced `MOVSS` before resuming.
///
/// # Safety
/// The displaced instructions must be replayable verbatim at the trampoline.
unsafe fn stash_trampoline(detour: &DetourSite) -> Trampoline {
    let mut trampoline = Trampoline::new();
    trampoline.call(CallBridge {
        callback: record_ammo as *const (),
        incoming_stack: StackAlignment::Aligned16,
        flags: FlagPolicy::KeepProduced,
        keep_return: KeepReturn::Neither,
        before_call: &STASH_ARGUMENT_SETUP,
        after_call: &[],
    });
    detour.replay_verbatim(&mut trampoline);
    detour.resume(&mut trampoline);
    trampoline
}

/// Redirects the FireShell rocket sound-name builders to the ammo's fields.
///
/// # Safety
///
/// Must run against the game version selected by the `1_151` feature.
pub unsafe fn patch(session: &mut PatchSession, module: ProcessModule) -> patchy::Result {
    BUILD_NAME.store(module.resolve_rva(BUILD_NAME_RVA)?, Ordering::Relaxed);
    ZOOM_LEVEL.store(module.resolve_rva(ZOOM_LEVEL_RVA)?, Ordering::Relaxed);

    for rva in STASH_SITES {
        let address = module.resolve_rva(rva)?;
        // `MOVSS [RSP+0x38],XMM0` has no RIP-relative operand.
        session.detour_trampoline(address, STASH_INSTRUCTION_LEN, |detour| {
            Ok(stash_trampoline(&detour))
        })?;
        log::debug!("Prepared rocket ammo stash at {address:#x}");
    }

    for rva in SOUND_SITES {
        let address = module.resolve_rva(rva)?;

        session.patch_call(
            address,
            SOUND_CALL_LEN,
            CallBridge {
                callback: build_rocket_sound as *const (),
                incoming_stack: StackAlignment::Aligned16,
                flags: FlagPolicy::KeepProduced,
                keep_return: KeepReturn::Rax,
                before_call: &SOUND_ARGUMENT_SETUP,
                after_call: &[],
            },
        )?;
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
    fn stash_records_the_ammo_before_replaying_the_installer_snapshot() {
        for rva in STASH_SITES {
            let address = 0x140000000 + rva;
            let trampoline_address = address + 0x10000;
            let snapshot = [0xcc; STASH_INSTRUCTION_LEN];
            let bytes = unsafe { stash_trampoline(&DetourSite::new(STASH_INSTRUCTION_LEN)) }
                .into_plan()
                .unwrap()
                .build_for_site(trampoline_address, &snapshot, address + snapshot.len())
                .unwrap();

            let (call, rest) = bytes.split_at(bytes.len() - snapshot.len() - 5);
            let callback = (record_ammo as *const () as usize).to_le_bytes();
            assert!(call
                .windows(callback.len())
                .any(|window| window == callback));
            assert_eq!(&rest[..snapshot.len()], snapshot);
            assert_eq!(rest[snapshot.len()], 0xe9);
            let displacement = i32::from_le_bytes(rest[snapshot.len() + 1..].try_into().unwrap());
            assert_eq!(
                (trampoline_address + bytes.len()) as i64 + displacement as i64,
                (address + snapshot.len()) as i64
            );
        }
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
}
