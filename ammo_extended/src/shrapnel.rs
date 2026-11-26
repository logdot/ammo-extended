//! Per-ammo overrides for logic 150. Metadata follows the parent Body's lifetime;
//! no native Ammo or Body fields are repurposed.

use crate::Ammo;
#[cfg(feature = "1_151")]
use highfleet::v1_151::Body;
#[cfg(not(feature = "1_151"))]
use highfleet::v1_163::Body;
use patchy::{
    CallBridge, DetourSite, FlagPolicy, KeepReturn, PatchSession, ProcessModule, StackAlignment,
    Trampoline,
};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Settings {
    pub damage: Option<f32>,
    pub count: Option<u32>,
    pub fuse_time: Option<f32>,
}

#[derive(Default)]
struct State {
    ammo: HashMap<usize, Settings>,
    bodies: HashMap<usize, Settings>,
}

impl State {
    fn capture(&mut self, body: usize, ammo: usize) {
        self.bodies.remove(&body);
        if let Some(settings) = self.ammo.get(&ammo) {
            self.bodies.insert(body, *settings);
        }
    }

    fn forget(&mut self, body: usize) {
        self.bodies.remove(&body);
    }

    fn count(&self, body: usize, vanilla_count: u32) -> u32 {
        self.bodies
            .get(&body)
            .and_then(|settings| settings.count)
            .unwrap_or(vanilla_count)
    }
}

static STATE: OnceLock<Mutex<State>> = OnceLock::new();

/// Called before installing the hooks. The ammo vector's allocation must remain
/// at the same address for as long as the game uses it. Returns whether any
/// logic-150 ammo needs the hooks.
pub fn configure(ammos: &[Ammo], settings: &[Settings]) -> bool {
    assert_eq!(ammos.len(), settings.len());
    let mut state = State::default();
    for (index, (ammo, settings)) in ammos.iter().zip(settings).enumerate() {
        if ammo.logic == 150 && *settings != Settings::default() {
            if settings.count == Some(0) {
                log::warn!(
                    "Ammo entry {index} (caliber {}) has shrapnel_count=0. This almost certainly will freeze or crash the game. Zero does not disable fragments.",
                    ammo.caliber
                );
            }
            state.ammo.insert(ammo as *const Ammo as usize, *settings);
        }
    }
    let enabled = !state.ammo.is_empty();
    *STATE
        .get_or_init(|| Mutex::new(State::default()))
        .lock()
        .unwrap_or_else(|poison| poison.into_inner()) = state;
    enabled
}

fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> Option<T> {
    STATE
        .get()
        .map(|state| f(&mut state.lock().unwrap_or_else(|poison| poison.into_inner())))
}

unsafe extern "C" fn capture(body: *mut Body, ammo: *const Ammo) {
    with_state(|state| state.capture(body as usize, ammo as usize));
}

unsafe extern "C" fn forget(body: *mut Body) {
    with_state(|state| state.forget(body as usize));
}

unsafe fn apply_damage(fragment: *mut Body, parent: *const Body, settings: Settings) {
    if (*parent).logic == 150 {
        if let Some(damage) = settings.damage {
            (*fragment).explosive_power = damage;
        }
    }
}

unsafe fn apply_fuse(parent: *mut Body, settings: Settings) {
    if (*parent).logic == 150 {
        if let Some(time) = settings.fuse_time {
            (*parent).logic_f1 = time;
        }
    }
}

unsafe extern "C" fn set_damage(fragment: *mut Body, parent: *const Body) {
    let settings = with_state(|state| state.bodies.get(&(parent as usize)).copied()).flatten();
    if let Some(settings) = settings {
        apply_damage(fragment, parent, settings);
    }
}

unsafe extern "C" fn set_fuse(parent: *mut Body) {
    let settings = with_state(|state| state.bodies.get(&(parent as usize)).copied()).flatten();
    if let Some(settings) = settings {
        apply_fuse(parent, settings);
    }
}

unsafe extern "C" fn get_count(parent: *const Body, vanilla_count: u32) -> u32 {
    with_state(|state| state.count(parent as usize, vanilla_count)).unwrap_or(vanilla_count)
}

#[cfg(feature = "1_151")]
const COUNT_ARGUMENTS: &[u8] = &[0x48, 0x89, 0xf1, 0x44, 0x89, 0xfa]; // RCX=RSI; EDX=R15D.
#[cfg(not(feature = "1_151"))]
const COUNT_ARGUMENTS: &[u8] = &[0x48, 0x89, 0xf1, 0x44, 0x89, 0xea]; // RCX=RSI; EDX=R13D.
#[cfg(feature = "1_151")]
const STORE_COUNT: &[u8] = &[0x41, 0x89, 0xc7]; // MOV R15D,EAX.
#[cfg(not(feature = "1_151"))]
const STORE_COUNT: &[u8] = &[0x41, 0x89, 0xc5]; // MOV R13D,EAX.

struct Site {
    rva: usize,
    /// Whole relocation-free instructions displaced by the detour.
    overwrite_len: usize,
    callback: *const (),
    arguments: &'static [u8],
    /// At a Windows-x64 function entry RSP is 8 mod 16; inside these functions it is 0.
    entry: bool,
    /// Assign the callback result to the fragment counter, preserving the original RAX.
    count_result: bool,
}

fn sites() -> [Site; 5] {
    #[cfg(feature = "1_151")]
    let (capture_rva, destroy_rva, damage_rva, fuse_rva, count_rva) =
        (0x503db, 0x17650, 0x339a0, 0x341b0, 0x33899);
    #[cfg(not(feature = "1_151"))]
    let (capture_rva, destroy_rva, damage_rva, fuse_rva, count_rva) =
        (0x52b49, 0x174d0, 0x33f80, 0x348a8, 0x33e77);
    [
        Site {
            rva: capture_rva,
            overwrite_len: 6,
            callback: capture as *const (),
            // MOV RCX,RBX (body); MOV RDX,RDI (ammo).
            arguments: &[0x48, 0x89, 0xd9, 0x48, 0x89, 0xfa],
            entry: false,
            count_result: false,
        },
        Site {
            rva: destroy_rva,
            overwrite_len: 5,
            callback: forget as *const (),
            arguments: &[],
            entry: true,
            count_result: false,
        },
        Site {
            rva: damage_rva,
            overwrite_len: 10,
            callback: set_damage as *const (),
            // MOV RCX,RBX (fragment); MOV RDX,RSI (parent).
            arguments: &[0x48, 0x89, 0xd9, 0x48, 0x89, 0xf2],
            entry: false,
            count_result: false,
        },
        Site {
            rva: fuse_rva,
            overwrite_len: 8,
            callback: set_fuse as *const (),
            arguments: &[0x48, 0x89, 0xf1],
            entry: false, // MOV RCX,RSI.
            count_result: false,
        },
        Site {
            rva: count_rva,
            // Counter initialization and the adjacent R12D setup are both replayed.
            overwrite_len: 8,
            callback: get_count as *const (),
            arguments: COUNT_ARGUMENTS,
            entry: false,
            count_result: true,
        },
    ]
}

/// # Safety
/// The displaced instructions must be replayable verbatim at the trampoline.
unsafe fn trampoline(site: &Site, detour: &DetourSite) -> Trampoline {
    let mut code = Trampoline::new();
    // Replay first, with the original RSP. This preserves the destructor's home
    // slot store and leaves vanilla damage/fuse intact when an override is absent.
    detour.replay_verbatim(&mut code);
    code.call(CallBridge {
        callback: site.callback,
        // None of the replayed instructions changes RSP.
        incoming_stack: if site.entry {
            StackAlignment::Offset8
        } else {
            StackAlignment::Aligned16
        },
        flags: FlagPolicy::Preserve,
        keep_return: KeepReturn::Neither,
        before_call: site.arguments,
        after_call: if site.count_result { STORE_COUNT } else { &[] },
    });
    detour.resume(&mut code);
    code
}

/// # Safety
/// Must run against the selected Steam executable before gameplay starts.
pub unsafe fn patch(session: &mut PatchSession, module: ProcessModule) -> patchy::Result {
    for site in sites() {
        let address = module.resolve_rva(site.rva)?;

        // These sites contain no RIP-relative operands or relative branches;
        // copying their complete instructions requires no relocation.
        session.detour_trampoline_with(address, site.overwrite_len, |detour| {
            Ok(trampoline(&site, &detour))
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_body(fill: u8) -> Body {
        let mut body = std::mem::MaybeUninit::<Body>::uninit();
        // Fill opaque state for byte-level assertions, then initialize every owning string and vector before the Body can be assumed initialized or dropped.
        // Non-owning raw pointer fields are never dereferenced by these tests.
        unsafe {
            body.as_mut_ptr()
                .cast::<u8>()
                .write_bytes(fill, std::mem::size_of::<Body>());
            let pointer = body.as_mut_ptr();
            (*pointer).base.base.base.vtable = std::ptr::null();
            std::ptr::addr_of_mut!((*pointer).base.base.base.class_name)
                .write(highfleet::general::EscadraString::new());
            std::ptr::addr_of_mut!((*pointer).base.name)
                .write(highfleet::general::EscadraString::new());
            std::ptr::addr_of_mut!((*pointer).oid).write(highfleet::general::EscadraString::new());
            std::ptr::addr_of_mut!((*pointer).logic_s1)
                .write(highfleet::general::EscadraString::new());
            std::ptr::addr_of_mut!((*pointer).ship_convertion_error)
                .write(highfleet::general::EscadraString::new());
            std::ptr::addr_of_mut!((*pointer).routs).write(highfleet::general::CVec::empty());
            std::ptr::addr_of_mut!((*pointer).slots).write(highfleet::general::CVec::empty());
            std::ptr::addr_of_mut!((*pointer).spasmcode).write(highfleet::general::CVec::empty());
            std::ptr::addr_of_mut!((*pointer).logic_editor)
                .write(highfleet::general::CVec::empty());
            std::ptr::addr_of_mut!((*pointer).sectors).write(highfleet::general::CVec::empty());
            std::ptr::addr_of_mut!((*pointer).sprites).write(highfleet::general::CVec::empty());
            body.assume_init()
        }
    }

    fn body_bytes(body: &Body) -> Vec<u8> {
        // Layout tests ensure all bytes, including native gaps, are explicit
        // fields: there is no implicit Rust padding to read in either view.
        unsafe {
            std::slice::from_raw_parts(
                std::ptr::from_ref(body).cast::<u8>(),
                std::mem::size_of::<Body>(),
            )
            .to_vec()
        }
    }

    #[test]
    fn test_owned_body_strings_remain_usable_across_native_style_borrows() {
        let mut body = test_body(0);
        let text = "A heap-allocated string owned by a Rust Body".to_owned();
        body.base.base.base.class_name.set_string(&text);
        body.base.name.set_string(&text);
        body.oid.set_string(&text);
        body.logic_s1.set_string(&text);
        body.ship_convertion_error.set_string(&text);
        let pointer = std::ptr::from_mut(&mut body);
        {
            let borrowed = unsafe { &*pointer };
            assert_eq!(borrowed.base.base.base.class_name.get_string(), text);
            assert_eq!(borrowed.base.name.get_string(), text);
            assert_eq!(borrowed.oid.get_string(), text);
            assert_eq!(borrowed.logic_s1.get_string(), text);
            assert_eq!(borrowed.ship_convertion_error.get_string(), text);
        }
        assert_eq!(body.oid.get_string(), text);
        // The test owns this Body; all five heap-backed strings and six empty vectors drop normally.
        drop(body);
    }

    #[test]
    fn metadata_is_per_projectile_and_removed_on_reinitialization_or_destruction() {
        let a = Settings {
            damage: Some(7.0),
            count: Some(20),
            fuse_time: Some(0.3),
        };
        let b = Settings {
            damage: Some(0.0),
            count: None,
            fuse_time: None,
        };
        let mut state = State::default();
        state.ammo.insert(10, a);
        state.ammo.insert(20, b);
        state.capture(100, 10);
        state.capture(200, 20);
        assert_eq!(state.bodies.get(&100), Some(&a));
        assert_eq!(state.bodies.get(&200), Some(&b));
        state.capture(100, 30);
        assert!(!state.bodies.contains_key(&100));
        state.forget(200);
        assert!(state.bodies.is_empty());
        state.capture(200, 10);
        assert_eq!(state.bodies.get(&200), Some(&a));
    }

    #[test]
    fn hooks_are_needed_only_for_logic_150_ammo_with_overrides() {
        #[cfg(feature = "1_151")]
        let json = include_str!("../ammo_extended_1_151.json");
        #[cfg(not(feature = "1_151"))]
        let json = include_str!("../ammo_extended_1_163.json");
        let mut config = crate::config::parse(json).unwrap();
        assert!(!configure(&config.ammos, &config.shrapnel));
        config.shrapnel[0].damage = Some(0.0);
        config.ammos[0].logic = 100;
        assert!(!configure(&config.ammos, &config.shrapnel));
        config.ammos[0].logic = 150;
        assert!(configure(&config.ammos, &config.shrapnel));
        with_state(|state| {
            let ammo = &config.ammos[0] as *const Ammo as usize;
            assert_eq!(state.ammo.len(), 1);
            state.capture(100, ammo);
            assert_eq!(state.bodies.get(&100), Some(&config.shrapnel[0]));
        });
        config.shrapnel[0] = Settings {
            count: Some(0),
            ..Settings::default()
        };
        assert!(configure(&config.ammos, &config.shrapnel));
        with_state(|state| {
            let ammo = &config.ammos[0] as *const Ammo as usize;
            state.capture(100, ammo);
            assert_eq!(state.count(100, 10), 0);
        });
        configure(&[], &[]);
    }

    #[test]
    fn fragment_counts_are_snapshotted_without_clamping_or_zero_handling() {
        let mut state = State::default();
        assert_eq!(state.count(100, 23), 23);
        for count in [0, 1, 10, 20, u32::MAX] {
            state.ammo.insert(
                10,
                Settings {
                    count: Some(count),
                    ..Settings::default()
                },
            );
            state.capture(100, 10);
            assert_eq!(state.count(100, 23), count);
            state.ammo.insert(10, Settings::default());
            assert_eq!(state.count(100, 23), count);
            state.capture(100, 10);
            assert_eq!(state.count(100, 23), 23);
            state.forget(100);
            assert_eq!(state.count(100, 23), 23);
        }
    }

    #[test]
    fn overrides_touch_only_fragment_damage_and_parent_fuse_for_logic_150() {
        for logic in [100i32, 150, 160] {
            for settings in [
                Settings::default(),
                Settings {
                    damage: Some(0.0),
                    count: None,
                    fuse_time: Some(0.0),
                },
                Settings {
                    damage: Some(8.5),
                    count: Some(20),
                    fuse_time: Some(0.75),
                },
            ] {
                let mut parent = test_body(0xa5);
                let mut fragment = test_body(0x5a);
                parent.logic = logic;
                parent.explosive_power = 17.0;
                parent.logic_f1 = 0.5;
                fragment.explosive_power = 2.0;
                fragment.logic_state = 3;
                let mut expected_parent = body_bytes(&parent);
                let mut expected_fragment = body_bytes(&fragment);
                if logic == 150 {
                    if let Some(value) = settings.damage {
                        let offset = std::mem::offset_of!(Body, explosive_power);
                        expected_fragment[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
                    }
                    if let Some(value) = settings.fuse_time {
                        let offset = std::mem::offset_of!(Body, logic_f1);
                        expected_parent[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
                    }
                }
                unsafe {
                    apply_damage(&mut fragment, &parent, settings);
                    apply_fuse(&mut parent, settings);
                }
                assert_eq!(body_bytes(&parent), expected_parent);
                assert_eq!(body_bytes(&fragment), expected_fragment);
            }
        }
    }

    fn trampoline_bytes(site: &Site, address: usize, snapshot: &[u8]) -> Vec<u8> {
        unsafe { trampoline(site, &DetourSite::new(site.overwrite_len)) }
            .into_plan()
            .unwrap()
            .build_for_site(address + 0x10000, snapshot, address + site.overwrite_len)
            .unwrap()
    }

    #[test]
    fn trampolines_replay_the_installer_snapshot_then_resume_after_the_span() {
        for site in sites() {
            let address = 0x140000000 + site.rva;
            let mut snapshot = vec![0x90; site.overwrite_len];
            snapshot[0] = 0xcc;
            let bytes = trampoline_bytes(&site, address, &snapshot);
            assert!(bytes.starts_with(&snapshot));
            assert_eq!(bytes[bytes.len() - 5], 0xe9);
            let displacement = i32::from_le_bytes(bytes[bytes.len() - 4..].try_into().unwrap());
            assert_eq!(
                (address + 0x10000 + bytes.len()) as i64 + displacement as i64,
                (address + site.overwrite_len) as i64
            );
        }
    }

    #[test]
    fn count_hook_routes_the_callback_result_before_the_bridge_restores_rax() {
        let site = sites().into_iter().find(|site| site.count_result).unwrap();
        let bytes = trampoline_bytes(
            &site,
            0x140000000 + site.rva,
            &vec![0x90; site.overwrite_len],
        );
        #[cfg(feature = "1_151")]
        let expected = [0x41, 0x89, 0xc7]; // MOV R15D,EAX.
        #[cfg(not(feature = "1_151"))]
        let expected = [0x41, 0x89, 0xc5]; // MOV R13D,EAX.

        // The bridge's inline callback address is followed by after_call.
        let callback = (get_count as *const () as usize).to_le_bytes();
        let after_call = bytes
            .windows(callback.len())
            .position(|window| window == callback)
            .expect("trampoline must call get_count")
            + callback.len();
        assert_eq!(&bytes[after_call..after_call + expected.len()], &expected);
    }
}
