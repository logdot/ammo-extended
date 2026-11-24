//! Per-ammo rocket thrust acceleration and burn duration.
//!
//! Rockets burn with a hardcoded `mass * 500` force along the body heading for
//! `1.5 s`, in `creatureLogic`'s logic 130 (all vanilla rockets) and logic 132
//! (the 1.163 300 mm rocket) blocks. Both numbers are independent of the rest
//! of the ammo record, and the body does not remember which ammo fired it:
//! `FireShell`'s rocket cases build the body inline, and several items share
//! logic 130.
//!
//! The mod therefore captures the settings where the `Ammo*` is still live.
//! Every rocket `FireShell` case stores `body->logic = ammo->logic` with the
//! ammo in `R15`; that store is replaced by a call that records
//! `body -> { acceleration, burn }`, then re-executes the store. The two
//! `creatureLogic` thrust sites ask that table, and the `+0x3d4` (`+0x3c4` in
//! 1.151) `1.5` stores write the recorded burn time instead of the literal.
//!
//! Nothing is written into the game's objects except the burn timer, which the
//! vanilla store writes anyway. A rocket body that never passes a `FireShell`
//! case — the 300 mm rocket launched as a ship part — has no entry and keeps
//! vanilla `500` / `1.5`; so does any ammo whose entry omits the fields.
//!
//! Sites:
//!
//! ```text
//! Steam 1.163                              Steam 1.151
//! 140030e90 MOV [RBX+0x3c4],EAX            140031444 MOV [RBX+0x3b4],EAX
//! 1400313c4 MOV [RSI+0x3c4],EAX            140031986 MOV [R14+0x3b4],EAX
//! 1400319d0 MOV [RBX+0x3c4],EAX
//! 140031c46 MOV [RSI+0x3c4],EAX
//! 140030e9d MOV [RBX+0x3d4],1.5            140031451 MOV [RBX+0x3c4],1.5
//! 1400313d1 MOV [RSI+0x3d4],1.5            140031995 MOV [R14+0x3c4],1.5
//! 140031c53 MOV [RSI+0x3d4],1.5
//! 14003538e MOV [RSI+0x3d4],1.5   (logic 132 state 0)
//! 1400348f2 MOVSS XMM7,[500]               140034230 MULSS XMM1,[500]
//! 140034c0c MOVSS XMM9,[500]
//! ```
//!
//! 1.151 multiplies the body mass at the thrust site itself (XMM1 already
//! holds it), so that trampoline multiplies the callback's acceleration by the
//! restored XMM1 before resuming. 1.163 loads the bare constant and multiplies
//! by mass a few instructions later.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use patchy::{PatchSession, ProcessModule, ReturnType, Trampoline};

#[cfg(feature = "1_151")]
use highfleet::v1_151::Body;
#[cfg(not(feature = "1_151"))]
use highfleet::v1_163::Body;

use crate::Ammo;

/// Vanilla burn force (`mass * this`) and burn duration.
pub const VANILLA_ACCELERATION: f32 = 500.0;
pub const VANILLA_BURN_SECONDS: f32 = 1.5;

/// Upper bound on remembered rocket bodies. Entries are overwritten at each
/// launch; this only stops an unbounded table when an ammo is configured for
/// burn time but not acceleration (no thrust callback clears entries).
const LIVE_LIMIT: usize = 512;

/// Optional per-ammo overrides. `None` keeps the vanilla value.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Settings {
    /// `creatureLogic` burn force in game units/s², applied as `mass * this`.
    pub acceleration: Option<f32>,
    /// Seconds of thrust (`Body::logic_f2`).
    pub burn_seconds: Option<f32>,
}

/// Which hook groups the configured values need.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Enabled {
    pub acceleration: bool,
    pub burn_seconds: bool,
}

impl Enabled {
    pub fn any(self) -> bool {
        self.acceleration || self.burn_seconds
    }
}

/// A resolved pair, recorded per rocket body at launch.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rocket {
    acceleration: f32,
    burn_seconds: f32,
}

#[derive(Default)]
struct State {
    /// `Ammo*` of every configured rocket ammo.
    ammo: HashMap<usize, Rocket>,
    /// `Body*` of rockets currently in flight.
    bodies: HashMap<usize, Rocket>,
}

static STATE: OnceLock<Mutex<State>> = OnceLock::new();

fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> Option<T> {
    STATE.get().map(|state| {
        let mut guard = state.lock().unwrap_or_else(|poison| poison.into_inner());
        f(&mut guard)
    })
}

/// Body field holding the remaining burn seconds (`0x3d4` / `0x3c4`).
const BURN_TIME: usize = std::mem::offset_of!(Body, logic_f2);

// `mov rcx, rbx` / `mov rcx, rsi` / `mov rcx, r14`.
const MOV_RCX_RBX: [u8; 3] = [0x48, 0x89, 0xd9];
const MOV_RCX_RSI: [u8; 3] = [0x48, 0x89, 0xf1];
#[cfg(feature = "1_151")]
const MOV_RCX_R14: [u8; 3] = [0x4c, 0x89, 0xf1];
/// `mov rdx, r15`: the selected `Ammo*` is in `R15` at every launch site.
const MOV_RDX_R15: [u8; 3] = [0x4c, 0x89, 0xfa];
/// `movaps xmm7, xmm0` / `movaps xmm9, xmm0` (1.163 thrust result).
#[cfg(not(feature = "1_151"))]
const MOVAPS_XMM7_XMM0: [u8; 3] = [0x0f, 0x28, 0xf8];
#[cfg(not(feature = "1_151"))]
const MOVAPS_XMM9_XMM0: [u8; 4] = [0x44, 0x0f, 0x28, 0xc8];
/// `mulss xmm1, xmm0`: 1.151's thrust site has already loaded the body mass
/// into XMM1, so the callback's acceleration is multiplied by it.
#[cfg(feature = "1_151")]
const MULSS_XMM1_XMM0: [u8; 4] = [0xf3, 0x0f, 0x59, 0xc8];

struct LaunchSite {
    rva: usize,
    store: &'static [u8],
    body_move: &'static [u8],
}

struct BurnSite {
    rva: usize,
    store: &'static [u8],
    body_move: &'static [u8],
}

struct ThrustSite {
    rva: usize,
    load: &'static [u8],
    /// Bytes emitted after the callback to put the result where vanilla did.
    result: &'static [u8],
}

#[cfg(feature = "1_151")]
const LAUNCH_SITES: &[LaunchSite] = &[
    LaunchSite {
        rva: 0x31444,
        store: &[0x89, 0x83, 0xb4, 0x03, 0x00, 0x00],
        body_move: &MOV_RCX_RBX,
    },
    LaunchSite {
        rva: 0x31986,
        store: &[0x41, 0x89, 0x86, 0xb4, 0x03, 0x00, 0x00],
        body_move: &MOV_RCX_R14,
    },
];

#[cfg(not(feature = "1_151"))]
const LAUNCH_SITES: &[LaunchSite] = &[
    LaunchSite {
        rva: 0x30e90,
        store: &[0x89, 0x83, 0xc4, 0x03, 0x00, 0x00],
        body_move: &MOV_RCX_RBX,
    },
    LaunchSite {
        rva: 0x313c4,
        store: &[0x89, 0x86, 0xc4, 0x03, 0x00, 0x00],
        body_move: &MOV_RCX_RSI,
    },
    LaunchSite {
        rva: 0x319d0,
        store: &[0x89, 0x83, 0xc4, 0x03, 0x00, 0x00],
        body_move: &MOV_RCX_RBX,
    },
    LaunchSite {
        rva: 0x31c46,
        store: &[0x89, 0x86, 0xc4, 0x03, 0x00, 0x00],
        body_move: &MOV_RCX_RSI,
    },
];

#[cfg(feature = "1_151")]
const BURN_SITES: &[BurnSite] = &[
    BurnSite {
        rva: 0x31451,
        store: &[0xc7, 0x83, 0xc4, 0x03, 0x00, 0x00, 0x00, 0x00, 0xc0, 0x3f],
        body_move: &MOV_RCX_RBX,
    },
    BurnSite {
        rva: 0x31995,
        store: &[
            0x41, 0xc7, 0x86, 0xc4, 0x03, 0x00, 0x00, 0x00, 0x00, 0xc0, 0x3f,
        ],
        body_move: &MOV_RCX_R14,
    },
];

#[cfg(not(feature = "1_151"))]
const BURN_SITES: &[BurnSite] = &[
    BurnSite {
        rva: 0x30e9d,
        store: &[0xc7, 0x83, 0xd4, 0x03, 0x00, 0x00, 0x00, 0x00, 0xc0, 0x3f],
        body_move: &MOV_RCX_RBX,
    },
    BurnSite {
        rva: 0x313d1,
        store: &[0xc7, 0x86, 0xd4, 0x03, 0x00, 0x00, 0x00, 0x00, 0xc0, 0x3f],
        body_move: &MOV_RCX_RSI,
    },
    BurnSite {
        rva: 0x31c53,
        store: &[0xc7, 0x86, 0xd4, 0x03, 0x00, 0x00, 0x00, 0x00, 0xc0, 0x3f],
        body_move: &MOV_RCX_RSI,
    },
    BurnSite {
        rva: 0x3538e,
        store: &[0xc7, 0x86, 0xd4, 0x03, 0x00, 0x00, 0x00, 0x00, 0xc0, 0x3f],
        body_move: &MOV_RCX_RSI,
    },
];

#[cfg(feature = "1_151")]
const THRUST_SITES: &[ThrustSite] = &[ThrustSite {
    rva: 0x34230,
    load: &[0xf3, 0x0f, 0x59, 0x0d, 0xbc, 0x21, 0x2f, 0x00],
    result: &MULSS_XMM1_XMM0,
}];

#[cfg(not(feature = "1_151"))]
const THRUST_SITES: &[ThrustSite] = &[
    ThrustSite {
        rva: 0x348f2,
        load: &[0xf3, 0x0f, 0x10, 0x3d, 0x3e, 0xdf, 0x31, 0x00],
        result: &MOVAPS_XMM7_XMM0,
    },
    ThrustSite {
        rva: 0x34c0c,
        load: &[0xf3, 0x44, 0x0f, 0x10, 0x0d, 0x23, 0xdc, 0x31, 0x00],
        result: &MOVAPS_XMM9_XMM0,
    },
];

/// Resolves the limits for every ammo that opts in. Called before the ammo
/// vector is handed to the game, so the stored `Ammo*` keys stay valid.
///
/// Values are taken as written; only a non-finite number falls back to vanilla.
pub fn configure(ammos: &[Ammo], settings: &[Settings]) -> Enabled {
    assert_eq!(ammos.len(), settings.len());
    let mut state = State::default();
    let mut enabled = Enabled::default();

    for (index, (ammo, settings)) in ammos.iter().zip(settings).enumerate() {
        let acceleration = sane(settings.acceleration, index, "rocket_acceleration");
        let burn_seconds = sane(settings.burn_seconds, index, "rocket_burn_seconds");
        if acceleration.is_none() && burn_seconds.is_none() {
            continue;
        }
        if ammo.logic != 130 && ammo.logic != 132 {
            log::warn!(
                "Ammo entry {index} ({}) sets rocket fields but its logic is {} (not 130/132); \
                 the values have no effect on a non-rocket",
                ammo.item_name.get_string(),
                ammo.logic
            );
            continue;
        }
        enabled.acceleration |= acceleration.is_some();
        enabled.burn_seconds |= burn_seconds.is_some();
        log::info!(
            "Ammo entry {index} ({}): rocket acceleration {}, burn time {} s",
            ammo.item_name.get_string(),
            acceleration.map_or("500 (vanilla)".to_owned(), |value| value.to_string()),
            burn_seconds.map_or("1.5 (vanilla)".to_owned(), |value| value.to_string()),
        );
        state.ammo.insert(
            ammo as *const Ammo as usize,
            Rocket {
                acceleration: acceleration.unwrap_or(VANILLA_ACCELERATION),
                burn_seconds: burn_seconds.unwrap_or(VANILLA_BURN_SECONDS),
            },
        );
    }

    if state.ammo.is_empty() {
        enabled = Enabled::default();
    }
    *STATE
        .get_or_init(|| Mutex::new(State::default()))
        .lock()
        .unwrap_or_else(|poison| poison.into_inner()) = state;
    enabled
}

fn sane(value: Option<f32>, index: usize, field: &str) -> Option<f32> {
    match value {
        Some(value) if value.is_finite() => Some(value),
        Some(_) => {
            log::warn!("Ammo entry {index}: {field} is not finite; keeping vanilla");
            None
        }
        None => None,
    }
}

fn launch_trampoline(site: &LaunchSite, address: usize) -> Trampoline {
    let mut argument_setup = site.store.to_vec();
    argument_setup.extend_from_slice(site.body_move);
    argument_setup.extend_from_slice(&MOV_RDX_R15);

    let mut trampoline = Trampoline::new();
    // `ReturnType::None`: the original store runs inside the setup, so RAX (the
    // ammo logic id) must survive the callback.
    trampoline.preserved_call(
        on_rocket_launched as *const (),
        &argument_setup,
        &[],
        ReturnType::None,
    );
    trampoline.relative_jump(address + site.store.len());
    trampoline
}

fn burn_trampoline(site: &BurnSite, address: usize) -> Trampoline {
    let mut trampoline = Trampoline::new();
    trampoline.preserved_call(
        on_burn_time_store as *const (),
        site.body_move,
        &[],
        ReturnType::None,
    );
    trampoline.relative_jump(address + site.store.len());
    trampoline
}

fn thrust_trampoline(site: &ThrustSite, address: usize) -> Trampoline {
    let mut trampoline = Trampoline::new();
    trampoline.preserved_call(
        on_burn_thrust as *const (),
        &MOV_RCX_RSI,
        &[],
        ReturnType::Xmm0,
    );
    // The preserved call has restored XMM1-5 and leaves the callback's f32 in
    // XMM0. Put it where the replaced constant load would have written.
    trampoline.bytes(site.result);
    trampoline.relative_jump(address + site.load.len());
    trampoline
}

/// Records the configured values for a freshly launched rocket body.
unsafe extern "C" fn on_rocket_launched(body: *mut u8, ammo: *const u8) {
    let body = body as usize;
    let ammo = ammo as usize;
    with_state(|state| {
        state.bodies.remove(&body);
        if let Some(rocket) = state.ammo.get(&ammo).copied() {
            if state.bodies.len() >= LIVE_LIMIT {
                state.bodies.clear();
            }
            state.bodies.insert(body, rocket);
        }
    });
}

/// Writes the configured burn time where vanilla stored `1.5`.
unsafe extern "C" fn on_burn_time_store(body: *mut u8) {
    let body = body as usize;
    let seconds = with_state(|state| state.bodies.get(&body).map(|rocket| rocket.burn_seconds))
        .flatten()
        .unwrap_or(VANILLA_BURN_SECONDS);
    // SAFETY: the replaced store targets the same live Body field.
    unsafe { ((body + BURN_TIME) as *mut f32).write_unaligned(seconds) };
}

/// Returns the configured burn force for this rocket body.
///
/// The entry is dropped once the burn timer is spent. 1.163 calls this before
/// decrementing the timer, so the last burning tick still sees a positive value
/// and keeps the configured force. 1.151 decrements first, so its final tick
/// sees `0` and gets vanilla for one frame before the table entry is dropped.
unsafe extern "C" fn on_burn_thrust(body: *mut u8) -> f32 {
    let body = body as usize;
    // SAFETY: the thrust site runs on a live rocket body.
    let remaining = unsafe { ((body + BURN_TIME) as *const f32).read_unaligned() };
    with_state(|state| {
        // NaN counts as spent, like the vanilla `COMISS`/`JBE` path.
        if remaining.is_nan() || remaining <= 0.0 {
            state.bodies.remove(&body);
            return VANILLA_ACCELERATION;
        }
        state
            .bodies
            .get(&body)
            .map(|rocket| rocket.acceleration)
            .unwrap_or(VANILLA_ACCELERATION)
    })
    .unwrap_or(VANILLA_ACCELERATION)
}

/// Prepares the hooks the configuration needs.
///
/// Every site is byte-verified first. If any differs from the vanilla
/// instruction the whole group is skipped and `Ok(false)` is returned, so a
/// mod that already owns those bytes (for example `hf_tuning`) disables rocket
/// tuning instead of failing this mod's other patches.
///
/// # Safety
///
/// Must run against the game version selected by the `1_151` feature, before
/// any rocket is launched.
pub unsafe fn patch(
    session: &mut PatchSession,
    module: ProcessModule,
    enabled: Enabled,
) -> patchy::Result<bool> {
    if !enabled.any() {
        return Ok(false);
    }

    let Some(launches) = resolve_launches(module)? else {
        return Ok(false);
    };
    let burns = if enabled.burn_seconds {
        match resolve_burns(module)? {
            Some(addresses) => addresses,
            None => return Ok(false),
        }
    } else {
        Vec::new()
    };
    let thrusts = if enabled.acceleration {
        match resolve_thrusts(module)? {
            Some(addresses) => addresses,
            None => return Ok(false),
        }
    } else {
        Vec::new()
    };

    for (site, address) in LAUNCH_SITES.iter().zip(&launches) {
        // SAFETY: the site was byte-verified by `resolve_launches`.
        unsafe {
            session.detour_trampoline(
                *address,
                site.store.len(),
                launch_trampoline(site, *address),
            )?
        };
        log::debug!("Prepared rocket launch hook at {address:#x}");
    }
    for (site, address) in BURN_SITES.iter().zip(&burns) {
        // SAFETY: the site was byte-verified by `resolve_burns`.
        unsafe {
            session.detour_trampoline(
                *address,
                site.store.len(),
                burn_trampoline(site, *address),
            )?
        };
        log::debug!("Prepared rocket burn-time hook at {address:#x}");
    }
    for (site, address) in THRUST_SITES.iter().zip(&thrusts) {
        // SAFETY: the site was byte-verified by `resolve_thrusts`.
        unsafe {
            session.detour_trampoline(
                *address,
                site.load.len(),
                thrust_trampoline(site, *address),
            )?
        };
        log::debug!("Prepared rocket thrust hook at {address:#x}");
    }
    Ok(true)
}

/// Resolved launch addresses, or `None` when a site is already patched.
fn resolve_launches(module: ProcessModule) -> patchy::Result<Option<Vec<usize>>> {
    let mut addresses = Vec::with_capacity(LAUNCH_SITES.len());
    for site in LAUNCH_SITES {
        let address = module.resolve_rva(site.rva)?;
        if !has_bytes(address, site.store) {
            log::warn!(
                "Rocket tuning: {address:#x} (launch site) is not the vanilla instruction; \
                 another mod may own it. Rocket acceleration/burn time stay vanilla."
            );
            return Ok(None);
        }
        addresses.push(address);
    }
    Ok(Some(addresses))
}

/// Resolved burn-time store addresses, or `None` when a site is already patched.
fn resolve_burns(module: ProcessModule) -> patchy::Result<Option<Vec<usize>>> {
    let mut addresses = Vec::with_capacity(BURN_SITES.len());
    for site in BURN_SITES {
        let address = module.resolve_rva(site.rva)?;
        if !has_bytes(address, site.store) {
            log::warn!(
                "Rocket tuning: {address:#x} (burn-time store) is not the vanilla instruction; \
                 another mod may own it. Rocket burn time stays vanilla."
            );
            return Ok(None);
        }
        addresses.push(address);
    }
    Ok(Some(addresses))
}

/// Resolved thrust addresses, or `None` when a site is already patched.
fn resolve_thrusts(module: ProcessModule) -> patchy::Result<Option<Vec<usize>>> {
    let mut addresses = Vec::with_capacity(THRUST_SITES.len());
    for site in THRUST_SITES {
        let address = module.resolve_rva(site.rva)?;
        if !has_bytes(address, site.load) {
            log::warn!(
                "Rocket tuning: {address:#x} (thrust site) is not the vanilla instruction; \
                 another mod may own it. Rocket acceleration stays vanilla."
            );
            return Ok(None);
        }
        addresses.push(address);
    }
    Ok(Some(addresses))
}

fn has_bytes(address: usize, expected: &[u8]) -> bool {
    // SAFETY: every site lies inside the main module's code and is readable
    // before gameplay starts.
    let current = unsafe { std::slice::from_raw_parts(address as *const u8, expected.len()) };
    current == expected
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::MutexGuard;

    /// Tests that touch the global table run one at a time.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock() -> MutexGuard<'static, ()> {
        TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn ammo(logic: i32) -> Ammo {
        let mut ammo: Ammo = unsafe { std::mem::zeroed() };
        ammo.logic = logic;
        ammo
    }

    #[test]
    fn configured_ammo_is_resolved_once_and_unknown_ammo_stays_vanilla() {
        let _guard = lock();
        let ammos = [ammo(130), ammo(132), ammo(100), ammo(130)];
        let settings = [
            Settings {
                acceleration: Some(900.0),
                burn_seconds: None,
            },
            Settings {
                acceleration: None,
                burn_seconds: Some(2.5),
            },
            Settings {
                acceleration: Some(4000.0),
                burn_seconds: Some(9.0),
            },
            Settings::default(),
        ];
        let enabled = configure(&ammos, &settings);
        assert_eq!(
            enabled,
            Enabled {
                acceleration: true,
                burn_seconds: true
            }
        );

        // A body launched by the first ammo gets its acceleration and the
        // vanilla burn; the second gets the opposite.
        let mut first = Box::new([0u8; 0x400]);
        unsafe {
            on_rocket_launched(first.as_mut_ptr(), &ammos[0] as *const Ammo as *const u8);
            on_burn_time_store(first.as_mut_ptr());
        }
        assert_eq!(
            f32::from_le_bytes(first[BURN_TIME..BURN_TIME + 4].try_into().unwrap()),
            VANILLA_BURN_SECONDS
        );
        // A positive timer keeps the configured force.
        first[BURN_TIME..BURN_TIME + 4].copy_from_slice(&1.0f32.to_le_bytes());
        assert_eq!(unsafe { on_burn_thrust(first.as_mut_ptr()) }, 900.0);
        // A spent timer drops the entry and falls back to vanilla.
        first[BURN_TIME..BURN_TIME + 4].copy_from_slice(&0.0f32.to_le_bytes());
        assert_eq!(
            unsafe { on_burn_thrust(first.as_mut_ptr()) },
            VANILLA_ACCELERATION
        );
        with_state(|state| assert!(!state.bodies.contains_key(&(first.as_mut_ptr() as usize))));

        let mut second = Box::new([0u8; 0x400]);
        unsafe {
            on_rocket_launched(second.as_mut_ptr(), &ammos[1] as *const Ammo as *const u8);
            on_burn_time_store(second.as_mut_ptr());
        }
        assert_eq!(
            f32::from_le_bytes(second[BURN_TIME..BURN_TIME + 4].try_into().unwrap()),
            2.5
        );
        assert_eq!(
            unsafe { on_burn_thrust(second.as_mut_ptr()) },
            VANILLA_ACCELERATION
        );
        // No entry: vanilla.
        let unknown = [0u8; 0x400];
        assert_eq!(
            unsafe { on_burn_thrust(unknown.as_ptr() as *mut u8) },
            VANILLA_ACCELERATION
        );
        // A non-rocket's fields are ignored.
        with_state(|state| {
            assert!(!state
                .ammo
                .contains_key(&(&ammos[2] as *const Ammo as usize)));
        });
        configure(&[], &[]);
    }

    #[test]
    fn non_finite_values_keep_vanilla_and_an_empty_config_enables_nothing() {
        let _guard = lock();
        let ammos = [ammo(130)];
        let enabled = configure(
            &ammos,
            &[Settings {
                acceleration: Some(f32::INFINITY),
                burn_seconds: Some(f32::NAN),
            }],
        );
        assert_eq!(enabled, Enabled::default());
        assert!(!enabled.any());
        with_state(|state| assert!(state.ammo.is_empty()));
    }

    #[test]
    fn launch_trampoline_replays_the_store_and_sets_both_arguments() {
        for site in LAUNCH_SITES {
            let address = 0x1_4000_0000usize + site.rva;
            let code = launch_trampoline(site, address).build(address).unwrap();
            assert!(
                code.windows(site.store.len())
                    .any(|bytes| bytes == site.store),
                "the original store is replayed"
            );
            assert!(
                code.windows(site.body_move.len())
                    .any(|bytes| bytes == site.body_move),
                "the body move is emitted"
            );
            assert!(code.windows(3).any(|bytes| bytes == MOV_RDX_R15));
            assert_eq!(code[code.len() - 5], 0xe9);
            let displacement = i32::from_le_bytes(code[code.len() - 4..].try_into().unwrap());
            assert_eq!(
                (address + code.len()) as i64 + displacement as i64,
                (address + site.store.len()) as i64
            );
        }
    }

    #[test]
    fn burn_trampoline_calls_with_the_body_and_resumes_after_the_store() {
        for site in BURN_SITES {
            let address = 0x1_4000_0000usize + site.rva;
            let code = burn_trampoline(site, address).build(address).unwrap();
            assert!(code
                .windows(site.body_move.len())
                .any(|bytes| bytes == site.body_move));
            assert!(
                !code
                    .windows(site.store.len())
                    .any(|bytes| bytes == site.store),
                "the vanilla store must not be replayed"
            );
            assert_eq!(code[code.len() - 5], 0xe9);
            let displacement = i32::from_le_bytes(code[code.len() - 4..].try_into().unwrap());
            assert_eq!(
                (address + code.len()) as i64 + displacement as i64,
                (address + site.store.len()) as i64
            );
        }
    }

    #[test]
    fn thrust_trampoline_puts_the_result_where_vanilla_wrote_it() {
        for site in THRUST_SITES {
            let address = 0x1_4000_0000usize + site.rva;
            let code = thrust_trampoline(site, address).build(address).unwrap();
            // The result move must come after the preserved call.
            let result_at = code
                .windows(site.result.len())
                .position(|bytes| bytes == site.result)
                .expect("the result move is emitted");
            let call_at = code
                .windows(6)
                .position(|bytes| bytes == [0xff, 0x15, 0x02, 0x00, 0x00, 0x00])
                .expect("the callback is called");
            assert!(result_at > call_at);
            assert_eq!(code[code.len() - 5], 0xe9);
        }
    }

    #[test]
    fn site_tables_match_the_two_versions() {
        #[cfg(feature = "1_151")]
        {
            assert_eq!(LAUNCH_SITES.len(), 2);
            assert_eq!(BURN_SITES.len(), 2);
            assert_eq!(THRUST_SITES.len(), 1);
            assert_eq!(THRUST_SITES[0].result, MULSS_XMM1_XMM0);
            assert_eq!(BURN_TIME, 0x3c4);
        }
        #[cfg(not(feature = "1_151"))]
        {
            assert_eq!(LAUNCH_SITES.len(), 4);
            assert_eq!(BURN_SITES.len(), 4);
            assert_eq!(THRUST_SITES.len(), 2);
            assert_eq!(THRUST_SITES[0].result, MOVAPS_XMM7_XMM0);
            assert_eq!(THRUST_SITES[1].result, MOVAPS_XMM9_XMM0);
            assert_eq!(BURN_TIME, 0x3d4);
        }
        // 1.151 multiplies by mass at the thrust site; 1.163 does not.
        #[cfg(feature = "1_151")]
        assert_eq!(MULSS_XMM1_XMM0, [0xf3, 0x0f, 0x59, 0xc8]);
        #[cfg(not(feature = "1_151"))]
        {
            assert_eq!(MOVAPS_XMM7_XMM0, [0x0f, 0x28, 0xf8]);
            assert_eq!(MOVAPS_XMM9_XMM0, [0x44, 0x0f, 0x28, 0xc8]);
        }
    }

    #[test]
    fn every_launch_site_has_a_matching_burn_site_in_1_163() {
        #[cfg(not(feature = "1_151"))]
        {
            // Case 54 sets its burn in creatureLogic state 0, not in FireShell.
            for launch in LAUNCH_SITES
                .iter()
                .take(2)
                .chain(LAUNCH_SITES.iter().skip(3))
            {
                assert!(
                    BURN_SITES
                        .iter()
                        .any(|burn| burn.rva > launch.rva && burn.rva - launch.rva < 0x100),
                    "no burn store after launch site {:#x}",
                    launch.rva
                );
            }
        }
    }
}
