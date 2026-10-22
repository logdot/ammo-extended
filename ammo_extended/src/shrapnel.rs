//! Per-ammo overrides for logic 150. Metadata follows the parent Body's lifetime;
//! no native Ammo or Body fields are repurposed.

use crate::Ammo;
#[cfg(feature = "1_151")]
use highfleet::v1_151::Body;
#[cfg(not(feature = "1_151"))]
use highfleet::v1_163::Body;
use patchy::{PatchSession, ProcessModule, ReturnType, Trampoline};
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
            (*parent).logic_timer = time;
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

fn trampoline(site: &Site, address: usize, original: &[u8]) -> Trampoline {
    let mut code = Trampoline::new();
    // Replay first, with the original RSP. This preserves the destructor's home
    // slot store and leaves vanilla damage/fuse intact when an override is absent.
    code.bytes(original);
    code.bytes(&[0x9c]); // PUSHFQ; preserved_call preserves registers but not flags.
    if site.count_result {
        // Also serves as the eight-byte alignment slot after PUSHFQ.
        code.bytes(&[0x50]); // PUSH RAX.
    } else if !site.entry {
        code.bytes(&[0x48, 0x8d, 0x64, 0x24, 0xf8]); // LEA RSP,[RSP-8], align call.
    }
    let result = if site.count_result {
        ReturnType::Rax
    } else {
        ReturnType::None
    };
    code.preserved_call(site.callback, site.arguments, &[], result);
    if site.count_result {
        // Intentionally replace the nonvolatile loop counter only after the call.
        // Zero is not special-cased: the original decrementing loop will underflow.
        code.bytes(STORE_COUNT);
        code.bytes(&[0x58]); // POP RAX.
    } else if !site.entry {
        code.bytes(&[0x48, 0x8d, 0x64, 0x24, 0x08]);
    }
    code.bytes(&[0x9d]); // POPFQ.
    code.relative_jump(address + site.overwrite_len);
    code
}

/// # Safety
/// Must run against the selected Steam executable before gameplay starts.
pub unsafe fn patch(session: &mut PatchSession, module: ProcessModule) -> patchy::Result {
    for site in sites() {
        let address = module.resolve_rva(site.rva)?;

        // These sites contain no RIP-relative operands or relative branches;
        // copying their complete instructions requires no relocation.
        let original = std::slice::from_raw_parts(address as *const u8, site.overwrite_len);
        let code = trampoline(&site, address, original);
        session.detour_trampoline(address, site.overwrite_len, code)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_body(fill: u8) -> Body {
        let mut body = std::mem::MaybeUninit::<Body>::uninit();
        // Fill opaque state for byte-level assertions, then initialize every
        // owning string before the Body can be assumed initialized or dropped.
        // Raw container pointers are never dereferenced by these tests.
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
        // The test owns this Body; all five heap-backed strings drop normally.
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
                parent.logic_timer = 0.5;
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
                        let offset = std::mem::offset_of!(Body, logic_timer);
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

    #[test]
    fn trampolines_copy_supplied_bytes_and_balance_flag_frames() {
        for site in sites() {
            let address = 0x140000000 + site.rva;
            let mut source = vec![0x90; site.overwrite_len];
            source[0] = 0xcc;
            let expected = source.clone();
            let code = trampoline(&site, address, &source);
            source.fill(0);
            let bytes = code.build(address + 0x10000).unwrap();
            assert!(bytes.starts_with(&expected));
            assert_eq!(bytes[site.overwrite_len], 0x9c);
            if site.count_result {
                assert!(!site.entry);
                assert_eq!(site.overwrite_len, 8);
                assert_eq!(bytes[site.overwrite_len + 1], 0x50); // PUSH RAX aligns and saves.
                let count_assignment = bytes.len() - 10;
                assert_eq!(&bytes[count_assignment..count_assignment + 3], STORE_COUNT);
                assert_eq!(bytes[count_assignment + 3], 0x58); // POP RAX, no zero branch.
                #[cfg(feature = "1_151")]
                {
                    assert_eq!(site.rva, 0x33899);
                    assert_eq!(STORE_COUNT, [0x41, 0x89, 0xc7]);
                    assert_eq!(COUNT_ARGUMENTS, [0x48, 0x89, 0xf1, 0x44, 0x89, 0xfa]);
                }
                #[cfg(not(feature = "1_151"))]
                {
                    assert_eq!(site.rva, 0x33e77);
                    assert_eq!(STORE_COUNT, [0x41, 0x89, 0xc5]);
                    assert_eq!(COUNT_ARGUMENTS, [0x48, 0x89, 0xf1, 0x44, 0x89, 0xea]);
                }
            }
            assert_eq!(bytes[bytes.len() - 6], 0x9d);
            assert_eq!(bytes[bytes.len() - 5], 0xe9);
            let displacement = i32::from_le_bytes(bytes[bytes.len() - 4..].try_into().unwrap());
            assert_eq!(
                (address + 0x10000 + bytes.len()) as i64 + displacement as i64,
                (address + site.overwrite_len) as i64
            );
        }
    }
}
