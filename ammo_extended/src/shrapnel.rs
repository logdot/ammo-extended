//! Per-ammo overrides for logic 150. Metadata follows the parent Body's lifetime;
//! no native Ammo or Body fields are repurposed.

use crate::Ammo;
use patchy::{PatchSession, ProcessModule, ReturnType, Trampoline};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Settings {
    pub damage: Option<f32>,
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
}

static STATE: OnceLock<Mutex<State>> = OnceLock::new();

/// Called before installing the hooks. The ammo vector's allocation must remain
/// at the same address for as long as the game uses it. Returns whether any
/// logic-150 ammo needs the hooks.
pub fn configure(ammos: &[Ammo], settings: &[Settings]) -> bool {
    assert_eq!(ammos.len(), settings.len());
    let mut state = State::default();
    for (ammo, settings) in ammos.iter().zip(settings) {
        if ammo.logic == 150 && *settings != Settings::default() {
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

unsafe extern "C" fn capture(body: *mut u8, ammo: *const u8) {
    with_state(|state| state.capture(body as usize, ammo as usize));
}

unsafe extern "C" fn forget(body: *mut u8) {
    with_state(|state| state.forget(body as usize));
}

#[cfg(feature = "1_151")]
const BODY_LOGIC: usize = 0x3b4;
#[cfg(not(feature = "1_151"))]
const BODY_LOGIC: usize = 0x3c4;
#[cfg(feature = "1_151")]
const BODY_DAMAGE: usize = 0x2cc;
#[cfg(not(feature = "1_151"))]
const BODY_DAMAGE: usize = 0x2dc;
const BODY_FUSE: usize = BODY_LOGIC + 0xc;

unsafe fn apply_damage(fragment: *mut u8, parent: *const u8, settings: Settings) {
    if parent.add(BODY_LOGIC).cast::<i32>().read_unaligned() == 150 {
        if let Some(damage) = settings.damage {
            fragment
                .add(BODY_DAMAGE)
                .cast::<f32>()
                .write_unaligned(damage);
        }
    }
}

unsafe fn apply_fuse(parent: *mut u8, settings: Settings) {
    if parent.add(BODY_LOGIC).cast::<i32>().read_unaligned() == 150 {
        if let Some(time) = settings.fuse_time {
            parent.add(BODY_FUSE).cast::<f32>().write_unaligned(time);
        }
    }
}

unsafe extern "C" fn set_damage(fragment: *mut u8, parent: *const u8) {
    let settings = with_state(|state| state.bodies.get(&(parent as usize)).copied()).flatten();
    if let Some(settings) = settings {
        apply_damage(fragment, parent, settings);
    }
}

unsafe extern "C" fn set_fuse(parent: *mut u8) {
    let settings = with_state(|state| state.bodies.get(&(parent as usize)).copied()).flatten();
    if let Some(settings) = settings {
        apply_fuse(parent, settings);
    }
}

struct Site {
    rva: usize,
    /// Whole relocation-free instructions displaced by the detour.
    overwrite_len: usize,
    callback: *const (),
    arguments: &'static [u8],
    /// At a Windows-x64 function entry RSP is 8 mod 16; inside these functions it is 0.
    entry: bool,
}

fn sites() -> [Site; 4] {
    #[cfg(feature = "1_151")]
    let (capture_rva, destroy_rva, damage_rva, fuse_rva) = (0x503db, 0x17650, 0x339a0, 0x341b0);
    #[cfg(not(feature = "1_151"))]
    let (capture_rva, destroy_rva, damage_rva, fuse_rva) = (0x52b49, 0x174d0, 0x33f80, 0x348a8);
    [
        Site {
            rva: capture_rva,
            overwrite_len: 6,
            callback: capture as *const (),
            // MOV RCX,RBX (body); MOV RDX,RDI (ammo).
            arguments: &[0x48, 0x89, 0xd9, 0x48, 0x89, 0xfa],
            entry: false,
        },
        Site {
            rva: destroy_rva,
            overwrite_len: 5,
            callback: forget as *const (),
            arguments: &[],
            entry: true,
        },
        Site {
            rva: damage_rva,
            overwrite_len: 10,
            callback: set_damage as *const (),
            // MOV RCX,RBX (fragment); MOV RDX,RSI (parent).
            arguments: &[0x48, 0x89, 0xd9, 0x48, 0x89, 0xf2],
            entry: false,
        },
        Site {
            rva: fuse_rva,
            overwrite_len: 8,
            callback: set_fuse as *const (),
            arguments: &[0x48, 0x89, 0xf1],
            entry: false, // MOV RCX,RSI.
        },
    ]
}

fn trampoline(site: &Site, address: usize, original: &[u8]) -> Trampoline {
    let mut code = Trampoline::new();
    // Replay first, with the original RSP. This preserves the destructor's home
    // slot store and leaves vanilla damage/fuse intact when an override is absent.
    code.bytes(original);
    code.bytes(&[0x9c]); // PUSHFQ; preserved_call preserves registers but not flags.
    if !site.entry {
        code.bytes(&[0x48, 0x8d, 0x64, 0x24, 0xf8]); // LEA RSP,[RSP-8], align call.
    }
    code.preserved_call(site.callback, site.arguments, &[], ReturnType::None);
    if !site.entry {
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

    #[test]
    fn metadata_is_per_projectile_and_removed_on_reinitialization_or_destruction() {
        let a = Settings {
            damage: Some(7.0),
            fuse_time: Some(0.3),
        };
        let b = Settings {
            damage: Some(0.0),
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
        configure(&[], &[]);
    }

    #[test]
    fn overrides_touch_only_fragment_damage_and_parent_fuse_for_logic_150() {
        for logic in [100i32, 150, 160] {
            for settings in [
                Settings::default(),
                Settings {
                    damage: Some(0.0),
                    fuse_time: Some(0.0),
                },
                Settings {
                    damage: Some(8.5),
                    fuse_time: Some(0.75),
                },
            ] {
                let mut parent = [0xa5u8; 0x400];
                parent[BODY_LOGIC..BODY_LOGIC + 4].copy_from_slice(&logic.to_le_bytes());
                let mut fragment = [0x5au8; 0x400];
                let mut expected_parent = parent;
                let mut expected_fragment = fragment;
                if logic == 150 {
                    if let Some(value) = settings.damage {
                        expected_fragment[BODY_DAMAGE..BODY_DAMAGE + 4]
                            .copy_from_slice(&value.to_le_bytes());
                    }
                    if let Some(value) = settings.fuse_time {
                        expected_parent[BODY_FUSE..BODY_FUSE + 4]
                            .copy_from_slice(&value.to_le_bytes());
                    }
                }
                unsafe {
                    apply_damage(fragment.as_mut_ptr(), parent.as_ptr(), settings);
                    apply_fuse(parent.as_mut_ptr(), settings);
                }
                assert_eq!(parent, expected_parent);
                assert_eq!(fragment, expected_fragment);
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
