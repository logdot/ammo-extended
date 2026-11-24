//! Makes gun-launched rockets use the ammo's `speed` as their initial velocity.
//!
//! Vanilla rocket cases copy the firing body's velocity into the new rocket and
//! then let the logic-130 thrust ramp the speed up from there. They never read
//! `ammo->speed`, unlike cannon shells (`Body_InitializeCannonShell`).
//!
//! Every affected case sets the rocket's angle to the firing direction with
//! `Body_SetAngle` immediately before the body starts ticking. Replacing those
//! calls lets the mod add `ammo->speed` along the body's nose (the cached
//! rotation at `Body + 0x8fc`, the same vector logic 130 applies its thrust
//! along) on top of the velocity the game already copied.
//!
//! Behaviors 51, 52 and 55 share two call sites per version (55 jumps into 52's
//! tail). Behavior 54 is not hooked: the 300 mm rocket uses logic 132, whose
//! state 0 overwrites the velocity with the master body's velocity on the
//! first tick, so an initial velocity written here would be discarded.

use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(feature = "1_151")]
use highfleet::v1_151::Body;
#[cfg(not(feature = "1_151"))]
use highfleet::v1_163::Body;
use patchy::{PatchError, PatchSession, ProcessModule, ReturnType, Trampoline};

use crate::Ammo;

/// Size of the replaced `CALL Body_SetAngle` instruction.
const REPLACED_SIZE: usize = 5;

/// `Body_SetAngle` entry; the rocket cases' calls must target it.
#[cfg(feature = "1_151")]
const BODY_SET_ANGLE_RVA: usize = 0x4f3c0;
#[cfg(not(feature = "1_151"))]
const BODY_SET_ANGLE_RVA: usize = 0x51850;

/// `Body_SetLinearVelocity` entry. Writing the Body fields alone does not
/// reach Box2D, so the new velocity must go through this function.
#[cfg(feature = "1_151")]
const BODY_SET_LINEAR_VELOCITY_RVA: usize = 0x4f900;
#[cfg(not(feature = "1_151"))]
const BODY_SET_LINEAR_VELOCITY_RVA: usize = 0x51e20;

/// `MOV RCX, <rocket body>` for each case.
const MOV_RCX_RBX: [u8; 3] = [0x48, 0x89, 0xd9];
#[cfg(not(feature = "1_151"))]
const MOV_RCX_RSI: [u8; 3] = [0x48, 0x89, 0xf1];
#[cfg(feature = "1_151")]
const MOV_RCX_R14: [u8; 3] = [0x4c, 0x89, 0xf1];
/// `MOV RDX, R15`: the selected `Ammo*` is in `R15` at every site.
const MOV_RDX_R15: [u8; 3] = [0x4c, 0x89, 0xfa];
/// `MOVAPS XMM2, XMM1`: pass the angle as the third float argument.
const MOVAPS_XMM2_XMM1: [u8; 3] = [0x0f, 0x28, 0xd1];

/// A `CALL Body_SetAngle` site inside `FireShell`.
struct Site {
    /// Ammo behaviors that reach this site; informational only.
    behavior: &'static str,
    rva: usize,
    /// Encoding of `MOV RCX, <rocket body register>`.
    load_body: [u8; 3],
}

#[cfg(feature = "1_151")]
const SITES: &[Site] = &[
    Site {
        behavior: "51",
        rva: 0x31517,
        load_body: MOV_RCX_RBX,
    },
    Site {
        behavior: "52, 55",
        rva: 0x31a22,
        load_body: MOV_RCX_R14,
    },
];

#[cfg(not(feature = "1_151"))]
const SITES: &[Site] = &[
    Site {
        behavior: "51",
        rva: 0x30f3b,
        load_body: MOV_RCX_RBX,
    },
    Site {
        behavior: "52, 55",
        rva: 0x3148a,
        load_body: MOV_RCX_RSI,
    },
];

/// Runtime address of `Body_SetAngle`, decoded from the replaced calls.
static BODY_SET_ANGLE: AtomicUsize = AtomicUsize::new(0);
/// Runtime address of `Body_SetLinearVelocity`.
static BODY_SET_LINEAR_VELOCITY: AtomicUsize = AtomicUsize::new(0);

/// Adds `speed` along a body angle to a velocity.
///
/// `Body_SetAngle` caches `(cos, -sin, sin, cos)` at `Body + 0x8fc`, so the
/// body's nose - the direction logic 130 thrusts - is `(sin, -cos)`.
fn launch_velocity(velocity: (f32, f32), angle: f32, speed: f32) -> (f32, f32) {
    let (sin, cos) = angle.sin_cos();
    (velocity.0 + speed * sin, velocity.1 - speed * cos)
}

/// Called in place of `CALL Body_SetAngle` in the rocket cases.
///
/// # Safety
///
/// `body` and `ammo` must point to a live rocket `Body` and its `Ammo` record.
/// `BODY_SET_ANGLE` must have been resolved by [`patch`].
unsafe extern "C" fn set_angle_and_speed(body: *mut Body, ammo: *const Ammo, angle: f32) {
    let set_angle: unsafe extern "C" fn(*mut Body, f32) =
        std::mem::transmute(BODY_SET_ANGLE.load(Ordering::Relaxed));
    set_angle(body, angle);

    let speed = (*ammo).speed;
    // Reading the Body fields is enough: the game's earlier
    // Body_SetLinearVelocity kept them in sync with Box2D. Writing them back
    // directly would not move the physics body, so update both through the
    // game's own setter.
    let (new_vx, new_vy) = launch_velocity(((*body).velocity_x, (*body).velocity_y), angle, speed);
    if speed == 0.0 {
        return;
    }
    let set_velocity: unsafe extern "C" fn(*mut Body, f32, f32) =
        std::mem::transmute(BODY_SET_LINEAR_VELOCITY.load(Ordering::Relaxed));
    set_velocity(body, new_vx, new_vy);
}

/// Prepares a launch-speed hook at every rocket case's angle store.
///
/// # Safety
///
/// Must run against the game version selected by the `1_151` feature.
pub unsafe fn patch(session: &mut PatchSession, module: ProcessModule) -> patchy::Result {
    let expected_set_angle = module.resolve_rva(BODY_SET_ANGLE_RVA)?;
    BODY_SET_LINEAR_VELOCITY.store(
        module.resolve_rva(BODY_SET_LINEAR_VELOCITY_RVA)?,
        Ordering::Relaxed,
    );

    for site in SITES {
        let address = module.resolve_rva(site.rva)?;
        let original = std::slice::from_raw_parts(address as *const u8, REPLACED_SIZE);
        if original[0] != 0xe8 {
            return Err(PatchError::SourceChanged { address });
        }

        let displacement = i32::from_le_bytes(original[1..5].try_into().unwrap());
        let target = (address as isize + REPLACED_SIZE as isize + displacement as isize) as usize;
        if target != expected_set_angle {
            return Err(PatchError::SourceChanged { address });
        }
        BODY_SET_ANGLE.store(target, Ordering::Relaxed);

        let mut argument_setup = site.load_body.to_vec();
        argument_setup.extend_from_slice(&MOV_RDX_R15);
        argument_setup.extend_from_slice(&MOVAPS_XMM2_XMM1);

        let mut trampoline = Trampoline::new();
        trampoline.preserved_call(
            set_angle_and_speed as *const (),
            &argument_setup,
            &[],
            ReturnType::None,
        );
        trampoline.relative_jump(address + REPLACED_SIZE);
        session.detour_trampoline(address, REPLACED_SIZE, trampoline)?;
        log::debug!(
            "Prepared rocket launch-speed hook for behavior {} at {address:#x}",
            site.behavior
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_angle_launches_along_the_negative_y_axis() {
        assert_eq!(launch_velocity((0.0, 0.0), 0.0, 150.0), (0.0, -150.0));
    }

    #[test]
    fn quarter_turns_follow_the_cached_rotation() {
        let (vx, vy) = launch_velocity((0.0, 0.0), std::f32::consts::FRAC_PI_2, 150.0);
        assert!((vx - 150.0).abs() < 1e-4);
        assert!(vy.abs() < 1e-4);

        let (vx, vy) = launch_velocity((0.0, 0.0), -std::f32::consts::FRAC_PI_2, 150.0);
        assert!((vx + 150.0).abs() < 1e-4);
        assert!(vy.abs() < 1e-4);

        let (vx, vy) = launch_velocity((0.0, 0.0), std::f32::consts::PI, 150.0);
        assert!(vx.abs() < 1e-4);
        assert!((vy - 150.0).abs() < 1e-4);
    }

    #[test]
    fn the_firing_bodys_velocity_is_kept() {
        assert_eq!(launch_velocity((20.0, -30.0), 0.0, 150.0), (20.0, -180.0));
    }

    #[test]
    fn a_negative_speed_reverses_the_launch() {
        assert_eq!(launch_velocity((0.0, 0.0), 0.0, -150.0), (0.0, 150.0));
    }

    #[test]
    fn a_zero_speed_is_the_vanilla_velocity() {
        assert_eq!(launch_velocity((20.0, -30.0), 0.7, 0.0), (20.0, -30.0));
    }

    #[test]
    fn site_table_covers_the_expected_behaviors() {
        let behaviors: Vec<&str> = SITES.iter().map(|site| site.behavior).collect();
        assert_eq!(behaviors, ["51", "52, 55"]);
    }

    #[test]
    fn velocity_setter_matches_the_body_layout_version() {
        #[cfg(feature = "1_151")]
        assert_eq!(BODY_SET_LINEAR_VELOCITY_RVA, 0x4f900);
        #[cfg(not(feature = "1_151"))]
        assert_eq!(BODY_SET_LINEAR_VELOCITY_RVA, 0x51e20);
    }

    #[test]
    fn register_moves_encode_the_intended_instructions() {
        // MOV RCX, RBX / RSI / R14 and MOV RDX, R15 (REX.W 89 /r, mod=11).
        assert_eq!(MOV_RCX_RBX, [0x48, 0x89, 0xd9]);
        #[cfg(not(feature = "1_151"))]
        assert_eq!(MOV_RCX_RSI, [0x48, 0x89, 0xf1]);
        #[cfg(feature = "1_151")]
        assert_eq!(MOV_RCX_R14, [0x4c, 0x89, 0xf1]);
        assert_eq!(MOV_RDX_R15, [0x4c, 0x89, 0xfa]);
        // MOVAPS XMM2, XMM1.
        assert_eq!(MOVAPS_XMM2_XMM1, [0x0f, 0x28, 0xd1]);
    }

    #[test]
    fn sites_load_the_registers_verified_in_the_binary() {
        #[cfg(feature = "1_151")]
        assert_eq!(SITES[0].load_body, MOV_RCX_RBX);
        #[cfg(not(feature = "1_151"))]
        assert_eq!(SITES[0].load_body, MOV_RCX_RBX);
        #[cfg(feature = "1_151")]
        assert_eq!(SITES[1].load_body, MOV_RCX_R14);
        #[cfg(not(feature = "1_151"))]
        assert_eq!(SITES[1].load_body, MOV_RCX_RSI);
    }
}
