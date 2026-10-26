//! Windows-only Steam 1.163 GUI construction and shop hook.

mod logic;

use self::logic::{can_offer, entries, Entry, Grid};
use crate::Ammo;
use highfleet::v1_163::{EscadraString, Gui, Node, Profile};
use patchy::{PatchError, PatchSession, ProcessModule, ReturnType, Trampoline};
use std::ffi::{c_char, c_void, CString};
use std::sync::OnceLock;

// Verified Steam 1.163 RVAs.
const HOOK_RVA: usize = 0x1fbc34;
// First of the three native room Relink calls, before the security-cookie check.
const ORIGINAL: &[u8] = &[
    0x48, 0x8b, 0x07, // MOV RAX,[RDI]
    0x33, 0xd2, // XOR EDX,EDX
    0x48, 0x8b, 0xcf, // MOV RCX,RDI
    0xff, 0x90, 0xb0, 0x00, 0x00, 0x00, // CALL [RAX+0xB0]
];

type Allocate = unsafe extern "system" fn(usize) -> *mut c_void;
type Construct = unsafe extern "system" fn(*mut c_void, u8) -> *mut Gui;
type CopyString =
    unsafe extern "system" fn(*mut EscadraString, *const c_char, usize) -> *mut EscadraString;
type Initialize = unsafe extern "system" fn(
    *mut Gui,
    *const c_char,
    f32,
    f32,
    f32,
    f32,
    *const c_char,
    *const c_char,
    *const c_char,
    *const c_char,
    *const c_char,
    f32,
);
type Add = unsafe extern "system" fn(*mut Node, *mut Node, *mut Node, u8) -> u8;
type Load = unsafe extern "system" fn(*mut Gui);
type Destroy = unsafe extern "system" fn(*mut Gui, u32) -> *mut c_void;
type GetProfile = unsafe extern "system" fn() -> *mut Profile;
type Quantity = unsafe extern "system" fn(*mut c_void, *const c_char) -> i32;

struct State {
    entries: Vec<Entry>,
    allocate: Allocate,
    construct: Construct,
    copy_string: CopyString,
    initialize: Initialize,
    get_profile: GetProfile,
    city_quantity: Quantity,
    player_quantity: Quantity,
    // Global locations are resolved once, then read at each shop construction.
    city: usize,
    width: usize,
    height: usize,
}
static STATE: OnceLock<State> = OnceLock::new();

/// # Safety
/// Only call for the verified Steam 1.163 executable before gameplay starts.
pub unsafe fn patch(
    session: &mut PatchSession,
    module: ProcessModule,
    vanilla: &[Ammo],
    configured: &[Ammo],
) -> patchy::Result {
    let entries = entries(vanilla, configured);
    if entries.is_empty() {
        log::info!("Custom ammo grid not installed: no eligible custom ammo definitions");
        return Ok(());
    }

    let address = module.resolve_rva(HOOK_RVA)?;
    if std::slice::from_raw_parts(address as *const u8, ORIGINAL.len()) != ORIGINAL {
        return Err(PatchError::SourceChanged { address });
    }

    // The entry-point checks complement the modloader's version gate and hook-byte check.
    for (rva, expected) in [
        (
            0x12a910,
            &[0x48, 0x89, 0x5c, 0x24, 0x10, 0x48, 0x89, 0x4c, 0x24, 0x08][..],
        ),
        (
            0x16a350,
            &[
                0x40, 0x53, 0x55, 0x56, 0x57, 0x41, 0x54, 0x41, 0x56, 0x41, 0x57,
            ][..],
        ),
        (
            0x56e0,
            &[0x48, 0x89, 0x5c, 0x24, 0x10, 0x48, 0x89, 0x6c, 0x24, 0x18][..],
        ),
    ] {
        let address = module.resolve_rva(rva)?;
        if std::slice::from_raw_parts(address as *const u8, expected.len()) != expected {
            return Err(PatchError::SourceChanged { address });
        }
    }

    let state = State {
        entries,
        allocate: std::mem::transmute::<usize, Allocate>(module.resolve_rva(0x334094)?),
        construct: std::mem::transmute::<usize, Construct>(module.resolve_rva(0x12a910)?),
        copy_string: std::mem::transmute::<usize, CopyString>(module.resolve_rva(0x56e0)?),
        initialize: std::mem::transmute::<usize, Initialize>(module.resolve_rva(0x16a350)?),
        get_profile: std::mem::transmute::<usize, GetProfile>(module.resolve_rva(0x292a90)?),
        city_quantity: std::mem::transmute::<usize, Quantity>(module.resolve_rva(0xf0060)?),
        player_quantity: std::mem::transmute::<usize, Quantity>(module.resolve_rva(0x28c460)?),
        city: module.resolve_rva(0x3a11940)?,
        width: module.resolve_rva(0x3a158fc)?,
        height: module.resolve_rva(0x3a13e5c)?,
    };

    log::info!(
        "Preparing 1.163 city-shop grid for {} custom ammo definitions",
        state.entries.len()
    );

    if STATE.set(state).is_err() {
        log::error!("Custom ammo shop hook was already prepared");
        return Err(PatchError::SourceChanged { address });
    }

    let mut code = Trampoline::new();
    // Interior site: RSP is 16-byte aligned; RDI is the supplies-room GUI.
    // The replayed XOR sets flags and no incoming flags are consumed at the continuation.
    code.preserved_call(
        append_entries as *const (),
        &[0x48, 0x89, 0xf9],
        &[],
        ReturnType::None,
    );
    // No relative branches or RIP-relative operands in these displaced instructions.
    code.bytes(ORIGINAL);
    code.relative_jump(address + ORIGINAL.len());
    session.detour_trampoline(address, ORIGINAL.len(), code)?;

    Ok(())
}

unsafe extern "system" fn append_entries(room: *mut Gui) {
    // A Rust panic must never unwind through the game's C++ frames.
    if std::panic::catch_unwind(|| unsafe { build(room) }).is_err() {
        log::error!("Custom ammo shop construction panicked; leaving the native shop intact");
    }
}

unsafe fn build(room: *mut Gui) {
    let Some(state) = STATE.get() else { return };

    if room.is_null() {
        log::warn!("Custom ammo grid skipped: supplies-room hook received a null room");
        return;
    }

    let city = (state.city as *const *mut c_void).read();
    let profile = (state.get_profile)();
    if city.is_null() || profile.is_null() {
        log::warn!("Custom ammo grid skipped: city={city:p}, profile={profile:p}");
        return;
    }

    let width = (state.width as *const i32).read();
    let height = (state.height as *const i32).read();
    let cash = (*profile).cash;
    log::info!(
        "Building custom ammo grid: definitions={}, viewport={}x{}, cash={}, room={:p}, name={:?}, state={}, position=({}, {}), stage={}, opacity={}",
        state.entries.len(), width, height, cash, room, (*room).identifier.get_string(),
        (*room).base.base.state, (*room).base.position_x, (*room).base.position_y,
        (*room).base.stage, (*room).base.opacity,
    );
    let Some(grid) = Grid::new(width, height) else {
        log::warn!("Custom ammo grid skipped: unsupported viewport size {width}x{height}");
        return;
    };

    let mut count = 0;
    let mut unavailable = 0;
    let mut rejected = 0;
    for entry in &state.entries {
        let stock = (state.city_quantity)(city, entry.item.as_ptr());
        let owned = (state.player_quantity)(profile.cast(), entry.item.as_ptr());
        if !can_offer(stock, owned, entry.price, cash) {
            if stock == 0 && owned == 0 {
                unavailable += 1;
                log::info!(
                    "Custom ammo {:?}: no button created (city stock=0, player owned=0); loading a definition does not add stock to an existing campaign",
                    entry.item,
                );
            } else {
                rejected += 1;
                log::warn!(
                    "Custom ammo {:?}: no button created (native transaction safety check failed: city stock={}, player owned={}, price={}, cash={})",
                    entry.item, stock, owned, entry.price, cash,
                );
            }
            continue;
        }

        let Some((x, y, button_width, button_height)) = grid.native_bounds(count) else {
            log::warn!("Custom ammo grid is full; remaining entries do not fit this viewport");
            break;
        };

        let label =
            CString::new(format!("{} | ${} | {}", entry.title, entry.price, stock)).unwrap();
        let memory = (state.allocate)(std::mem::size_of::<Gui>());
        if memory.is_null() {
            log::error!("Custom ammo grid allocation failed for {:?}", entry.item);
            break;
        }

        let button = (state.construct)(memory, 0);
        // Match native order: construct, attach, initialize, load. Parent owns the node.
        let vtable = (*room).base.base.base.base.vtable as *const usize;
        let add: Add = std::mem::transmute(*vtable.add(0x50 / 8));
        if add(room.cast(), button.cast(), std::ptr::null_mut(), 0) == 0 {
            // Node::add rejects an already-owned node without taking ownership.
            if (*button).base.base.master.target.is_null() {
                let vtable = (*button).base.base.base.base.vtable as *const usize;
                let destroy: Destroy = std::mem::transmute(*vtable);
                destroy(button, 1);
            }

            log::error!("Failed to attach a custom ammo button to the supplies room");
            break;
        }

        (state.initialize)(
            button,
            c"SUPPLIES_AMMO".as_ptr(),
            x,
            y,
            button_width,
            button_height,
            label.as_ptr(),
            c"gui_std".as_ptr(),
            c"gui_std_focus".as_ptr(),
            c"gui_std_click".as_ptr(),
            c"dinpro_14_bold".as_ptr(),
            1.0,
        );

        (state.copy_string)(
            &raw mut (*button).gui_s1,
            entry.item.as_ptr(),
            entry.item.as_bytes().len(),
        );

        // Above room masks/foreground (+2) and receipt (+3), below BUY_PNL (0x44).
        // This is an intentional foreground grid, not part of the illustrated shelf.
        (*button).base.stage += 4;

        let vtable = (*button).base.base.base.base.vtable as *const usize;
        let load: Load = std::mem::transmute(*vtable.add(0x88 / 8));
        load(button);
        log::info!(
            "Created custom ammo button {:?}: city stock={}, player owned={}, button={:p}, parent={:p}, state={}, position=({}, {}), native size=({}, {}), stage={} (room stage={}), opacity={}, clickable={}, text={:?}, font={:?}",
            entry.item, stock, owned, button, (*button).base.base.master.target,
            (*button).base.base.state, (*button).base.position_x, (*button).base.position_y,
            button_width, button_height, (*button).base.stage, (*room).base.stage, (*button).base.opacity,
            (*button).clickable, (*button).text.get_string(), (*button).font.get_string(),
        );
        count += 1;
    }

    log::info!(
        "Custom ammo grid result: created={count}, unavailable={unavailable}, rejected={rejected}, configured={}",
        state.entries.len(),
    );
    // The displaced native room Relink call runs next, registering the new children too.
}
