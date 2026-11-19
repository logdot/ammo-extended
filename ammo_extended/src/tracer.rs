//! Per-ammo tracer images for cannon shells.
//!
//! The `Body` draw pass (`Body` vtable slot `+0x238`) builds a shell's tracer
//! from two sprites loaded once by the battle setup: `sign_tracer_02` is the
//! segment at the shell and `sign_tracer_01` is the tail drawn twice behind it.
//! Both are switched to their `_blue` twins when `body->logic == 160`; the choice
//! is hardcoded per body, so there is no native per-ammo field to configure.
//!
//! This module keeps the vanilla choice and replaces the pair with the sprites
//! named in the config. Shell bodies do not remember their `Ammo` record, so it
//! is captured where the game still holds both pointers
//! (`Body_InitializeCannonShell`). Anything that does not come through that
//! function (rockets, canisters, mines) keeps the vanilla tracers.
//!
//! The tracer's length and width are the two scale factors the draw computes
//! right before it renders (`halfWidth`/`halfHeight` of `Gfx_QuadRenderEx`), so
//! they are scaled by a third hook that only exists when a config asks for it.
//!
//! **One sprite object per image name.** The media manager hands out a single,
//! cached object for a name (`ResDesc` stores it at `+0x90`), so a tint applied
//! at load is a property of the *image*, not of the ammo that asked for it. That
//! is why a colour on one of the game's own tracer sprites recolours every
//! tracer drawing that image, and why two colours for one name cannot coexist.

use std::collections::{HashMap, HashSet};
use std::ffi::{c_char, CString};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

#[cfg(feature = "1_151")]
use highfleet::v1_151::Body;
#[cfg(not(feature = "1_151"))]
use highfleet::v1_163::Body;
use patchy::{PatchError, PatchSession, ProcessModule, ReturnType, Trampoline};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::Ammo;

/// An ARGB colour, written either as `"#RRGGBB"` / `"#AARRGGBB"` or as an
/// integer in the same layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(pub u32);

impl Color {
    /// No tint: the sprite is drawn with its own texture colours.
    pub const WHITE: Self = Self(u32::MAX);
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("#{:08X}", self.0))
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let value = serde_json::Value::deserialize(deserializer)?;
        match value {
            serde_json::Value::String(text) => {
                parse_color(&text).map(Self).map_err(D::Error::custom)
            }
            serde_json::Value::Number(number) => number
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .map(Self)
                .ok_or_else(|| {
                    D::Error::custom(format!("tracer_color {number} is not a 32-bit ARGB value"))
                }),
            other => Err(D::Error::custom(format!(
                "tracer_color must be a \"#RRGGBB\"/\"#AARRGGBB\" string or an ARGB integer, got {other}"
            ))),
        }
    }
}

fn parse_color(text: &str) -> Result<u32, String> {
    let digits = text.strip_prefix('#').unwrap_or(text);
    let value = u32::from_str_radix(digits, 16)
        .map_err(|_| format!("tracer_color {text:?} is not hexadecimal"))?;
    match digits.len() {
        6 => Ok(0xff00_0000 | value),
        8 => Ok(value),
        _ => Err(format!(
            "tracer_color {text:?} must have 6 or 8 hexadecimal digits"
        )),
    }
}

/// User-facing tracer overrides for one ammo entry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    /// Replaces both the head and the tail.
    pub image: Option<String>,
    /// Replaces the head segment only.
    pub head: Option<String>,
    /// Replaces the tail streak only.
    pub tail: Option<String>,
    /// ARGB tint applied to every sprite this ammo loads.
    pub color: Option<Color>,
    /// Uniform length and width multiplier.
    pub scale: Option<f32>,
    /// Length multiplier, overriding `scale` for this axis.
    pub length: Option<f32>,
    /// Width multiplier, overriding `scale` for this axis.
    pub width: Option<f32>,
}

impl Settings {
    /// Name drawn at the shell; the head override wins over the shared image.
    pub fn head_name(&self) -> Option<&str> {
        self.head.as_deref().or(self.image.as_deref())
    }

    /// Name drawn behind the shell; the tail override wins over the shared image.
    pub fn tail_name(&self) -> Option<&str> {
        self.tail.as_deref().or(self.image.as_deref())
    }

    /// Tint passed to the sprite loader; white means "no tint".
    pub fn argb(&self) -> u32 {
        self.color.unwrap_or(Color::WHITE).0
    }

    /// Multiplier for the tracer's length: `tracer_length` wins over
    /// `tracer_scale`. 1.0 keeps the game's own value.
    pub fn length_factor(&self) -> f32 {
        self.length.or(self.scale).unwrap_or(1.0)
    }

    /// Multiplier for the tracer's width: `tracer_width` wins over
    /// `tracer_scale`. 1.0 keeps the game's own value.
    pub fn width_factor(&self) -> f32 {
        self.width.or(self.scale).unwrap_or(1.0)
    }
}

/// Resolved state for one live shell body.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Tracer {
    /// Sprite drawn at the shell (`sign_tracer_02` in vanilla); 0 keeps vanilla.
    head: usize,
    /// Sprite drawn behind it (`sign_tracer_01` in vanilla); 0 keeps vanilla.
    tail: usize,
    /// `ammo.logic`, which the shell copies into its `logic` field; guards
    /// recycled body addresses against a stale entry.
    logic: i32,
    /// Length multiplier for the draw (`halfHeight`).
    length: f32,
    /// Width multiplier for the draw (`halfWidth`).
    width: f32,
}

/// One loaded sprite: the colour it was tinted with and its object pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Sprite {
    argb: u32,
    pointer: usize,
}

#[derive(Default)]
struct State {
    /// Ammo record pointer -> configured values, for entries that name anything.
    ammo: HashMap<usize, Settings>,
    /// Shell body pointer -> resolved values.
    bodies: HashMap<usize, Tracer>,
    /// Sprite name -> loaded sprite (0 once a name failed to load).
    sprites: HashMap<String, Sprite>,
    /// Names already reported as requested with two different colours.
    color_conflicts: HashSet<String>,
    /// Sprite pointers already reported as shared with the game's own tracers.
    vanilla_aliases: HashSet<usize>,
}

impl State {
    fn capture(&mut self, body: usize, ammo: usize, logic: i32, label: &str) {
        self.bodies.remove(&body);
        let Some(settings) = self.ammo.get(&ammo).cloned() else {
            return;
        };
        let argb = settings.argb();
        let head = settings
            .head_name()
            .map(|name| self.sprite(name, argb))
            .unwrap_or(0);
        let tail = settings
            .tail_name()
            .map(|name| self.sprite(name, argb))
            .unwrap_or(0);
        let game = [
            read_global(0),
            read_global(1),
            read_global(2),
            read_global(3),
        ];
        self.warn_vanilla_alias(label, settings.head_name(), head, argb, game);
        if tail != head {
            self.warn_vanilla_alias(label, settings.tail_name(), tail, argb, game);
        }
        self.bodies.insert(
            body,
            Tracer {
                head,
                tail,
                logic,
                length: settings.length_factor(),
                width: settings.width_factor(),
            },
        );
    }

    /// Loads a sprite once per name. A failed name stays cached as 0 so the
    /// loader is not retried for every shell.
    ///
    /// The media manager caches one sprite object per name, so a second colour
    /// for the same image cannot take effect; the first one is kept.
    fn sprite(&mut self, name: &str, argb: u32) -> usize {
        if let Some(sprite) = self.sprites.get(name) {
            if sprite.argb != argb && self.color_conflicts.insert(name.to_owned()) {
                log::warn!(
                    "Ammo Extended: tracer image {name:?} is used with colour {argb:#010X} after {:#010X}; the game shares one sprite per image name, so the first colour stays. Give each colour its own image.",
                    sprite.argb
                );
            }
            return sprite.pointer;
        }
        let pointer = load_sprite(name, argb);
        if pointer == 0 {
            log::warn!(
                "Ammo Extended: tracer image {name:?} is not in the loaded media resources; that ammo keeps the vanilla tracer"
            );
        } else {
            log::debug!(
                "Ammo Extended: loaded tracer image {name:?} at {pointer:#x} with colour {argb:#010X}"
            );
        }
        self.sprites.insert(
            name.to_owned(),
            Sprite { argb, pointer },
        );
        pointer
    }

    /// Naming one of the game's four tracer sprites makes the tint global: the
    /// media manager hands out one object per name and the game's own tracers
    /// point at it.
    fn warn_vanilla_alias(
        &mut self,
        label: &str,
        name: Option<&str>,
        pointer: usize,
        argb: u32,
        game: [usize; 4],
    ) {
        if !is_game_tracer(pointer, game) || !self.vanilla_aliases.insert(pointer) {
            return;
        }
        let name = name.unwrap_or("?");
        if argb == Color::WHITE.0 {
            log::warn!(
                "Ammo Extended: ammo {label:?} uses the game's own tracer sprite {name:?}. The game keeps one object per image name, so anything that touches it applies to every tracer drawing it (enemy shells included, except the logic-160 blue pair)."
            );
        } else {
            log::warn!(
                "Ammo Extended: ammo {label:?} recolours the game's own tracer sprite {name:?} with {argb:#010X}. Every tracer drawing that image turns that colour, including enemy shells and ammo you did not configure (the logic-160 blue pair uses different images). Give this ammo a dedicated image if the colour must stay local."
            );
        }
    }

    fn tracer(&self, body: usize, logic: i32) -> Option<Tracer> {
        self.bodies.get(&body).copied().filter(|t| t.logic == logic)
    }
}

/// Whether `pointer` is one of the game's four tracer sprites.
fn is_game_tracer(pointer: usize, game: [usize; 4]) -> bool {
    pointer != 0 && game.contains(&pointer)
}

/// What the hooks have to install. Both are false unless the config asks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Hooks {
    /// Any tracer sprite is configured.
    pub any: bool,
    /// Some ammo changes the tracer's length or width.
    pub scale: bool,
}

static STATE: OnceLock<Mutex<State>> = OnceLock::new();
/// Value returned to the draw trampoline, in register order: `[tail, head]`.
static PAIR: [AtomicUsize; 2] = [AtomicUsize::new(0), AtomicUsize::new(0)];
/// Value returned to the scale trampoline, as `f32` bits: `[length, width]`.
static SCALE_PAIR: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];
/// Runtime address of `Sprite_LoadByNameAndSetFields`, resolved during install.
static LOAD_SPRITE: AtomicUsize = AtomicUsize::new(0);

/// Called before installing the hooks. The ammo vector's allocation must remain
/// at the same address for as long as the game uses it.
pub fn configure(ammos: &[Ammo], settings: &[Settings]) -> Hooks {
    assert_eq!(ammos.len(), settings.len());
    let mut state = State::default();
    let mut hooks = Hooks::default();
    for (ammo, settings) in ammos.iter().zip(settings) {
        if *settings != Settings::default() {
            state
                .ammo
                .insert(ammo as *const Ammo as usize, settings.clone());
            hooks.any = true;
            hooks.scale |= settings.length_factor() != 1.0 || settings.width_factor() != 1.0;
        }
    }
    let enabled = hooks.any;
    *STATE
        .get_or_init(|| Mutex::new(State::default()))
        .lock()
        .unwrap_or_else(|poison| poison.into_inner()) = state;
    if !enabled {
        Hooks::default()
    } else {
        hooks
    }
}

fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> Option<T> {
    STATE
        .get()
        .map(|state| f(&mut state.lock().unwrap_or_else(|poison| poison.into_inner())))
}

type LoadSprite = unsafe extern "C" fn(*const c_char, u32, u32) -> *mut u8;

/// `Sprite_LoadByNameAndSetFields`: loads a sprite by name from the game's
/// media, sets its alpha to 1.0, tints it and stores the blend state.
fn load_sprite(name: &str, argb: u32) -> usize {
    let address = LOAD_SPRITE.load(Ordering::Relaxed);
    if address == 0 {
        return 0;
    }
    // A name with an interior NUL cannot address a media entry.
    let Ok(name) = CString::new(name) else {
        return 0;
    };
    // Blend state 0 matches the vanilla tracers.
    let load: LoadSprite = unsafe { std::mem::transmute(address) };
    unsafe { load(name.as_ptr(), 0, argb) as usize }
}

/// Called from `Body_InitializeCannonShell` with the shell and its ammo record.
///
/// # Safety
///
/// Both pointers must be live; the ammo record must be the one the shell is
/// being initialized from.
unsafe extern "C" fn capture(body: *mut Body, ammo: *const Ammo) {
    let logic = (*ammo).logic;
    let label = (*ammo).item_name.get_string();
    with_state(|state| state.capture(body as usize, ammo as usize, logic, label));
}

/// Called from the tracer draw in place of the vanilla two-sprite selection.
///
/// Returns `[tail, head]`, the register order the draw uses (`RDI` is the tail,
/// `RBX` the segment at the shell). An unconfigured, recycled or unknown body
/// gets the vanilla pair back, so the trampoline can always overwrite both.
///
/// # Safety
///
/// `body` must be the body being drawn.
unsafe extern "C" fn pick(body: *const Body) -> *const usize {
    let logic = (*body).logic;
    let vanilla = vanilla_pair(
        logic,
        read_global(0),
        read_global(1),
        read_global(2),
        read_global(3),
    );
    let (head, tail) = overlay(
        with_state(|state| state.tracer(body as usize, logic)).flatten(),
        vanilla,
    );
    PAIR[0].store(tail, Ordering::Relaxed);
    PAIR[1].store(head, Ordering::Relaxed);
    PAIR.as_ptr() as *const usize
}

/// Called right before the tracer's renders, after the game computed its own
/// length (`XMM6`) and width (`XMM8`). Returns `[length, width]` multipliers.
///
/// # Safety
///
/// `body` must be the body being drawn.
unsafe extern "C" fn scale(body: *const Body) -> *const f32 {
    let (length, width) = with_state(|state| {
        state
            .tracer(body as usize, (*body).logic)
            .map(|tracer| (tracer.length, tracer.width))
    })
    .flatten()
    .unwrap_or((1.0, 1.0));
    SCALE_PAIR[0].store(length.to_bits(), Ordering::Relaxed);
    SCALE_PAIR[1].store(width.to_bits(), Ordering::Relaxed);
    SCALE_PAIR.as_ptr() as *const f32
}

/// The game's own choice: shells with the proximity-fuze logic draw the blue pair.
fn vanilla_pair(
    logic: i32,
    head: usize,
    tail: usize,
    head_blue: usize,
    tail_blue: usize,
) -> (usize, usize) {
    if logic == BLUE_LOGIC {
        (head_blue, tail_blue)
    } else {
        (head, tail)
    }
}

/// Replaces the parts an ammo configured; a missing or failed sprite keeps vanilla.
fn overlay(configured: Option<Tracer>, (head, tail): (usize, usize)) -> (usize, usize) {
    match configured {
        Some(tracer) => (
            if tracer.head != 0 { tracer.head } else { head },
            if tracer.tail != 0 { tracer.tail } else { tail },
        ),
        None => (head, tail),
    }
}

/// Reads one of the game's four global tracer sprite pointers, resolved at
/// install time. Returns 0 when the globals were never resolved.
fn read_global(index: usize) -> usize {
    let address = GLOBALS[index].load(Ordering::Relaxed);
    if address == 0 {
        return 0;
    }
    unsafe { (address as *const usize).read() }
}

#[cfg(feature = "1_151")]
mod sites {
    /// Start of the vanilla tracer pair selection in `handleAmmoInBattle?`
    /// (the `Body` draw): `CMP dword ptr [RSI + 0x3b4], 0xa0` followed by the two
    /// global loads and two conditional moves. All five instructions are
    /// replaced, so the block is `0x28` bytes and resumes at the `UCOMISS` that
    /// consumes the pair.
    pub const DRAW: usize = 0x49ea2;
    pub const DRAW_SIZE: usize = 0x28;
    /// `MOVSS XMM2, [RSI + 0x928]` right before the first render: the length and
    /// width are final here.
    pub const SCALE: usize = 0x49f43;
    /// `MOVAPS [RAX - 0x48], XMM8` in `BulletImpact??`: `RCX` = shell,
    /// `RDX` = ammo record.
    pub const CAPTURE: usize = 0x50324;
    /// `Sprite_LoadByNameAndSetFields`.
    pub const LOAD_SPRITE: usize = 0x1fdd50;
}

#[cfg(not(feature = "1_151"))]
mod sites {
    /// Start of the vanilla tracer pair selection in `handleAmmoInBattle?`
    /// (the `Body` draw): `CMP dword ptr [RSI + 0x3c4], 0xa0` followed by the two
    /// global loads and two conditional moves. All five instructions are
    /// replaced, so the block is `0x28` bytes and resumes at the `UCOMISS` that
    /// consumes the pair.
    pub const DRAW: usize = 0x4c262;
    pub const DRAW_SIZE: usize = 0x28;
    /// `MOVSS XMM2, [RSI + 0x960]` right before the first render: the length and
    /// width are final here.
    pub const SCALE: usize = 0x4c30c;
    /// `MOVAPS [RAX - 0x48], XMM8` in `Body_InitializeCannonShell`: `RCX` = shell,
    /// `RDX` = ammo record.
    pub const CAPTURE: usize = 0x52aa4;
    /// `Sprite_LoadByNameAndSetFields`.
    pub const LOAD_SPRITE: usize = 0x21b0a0;
}

#[cfg(feature = "1_151")]
mod globals {
    /// `sign_tracer_02`: the segment drawn at the shell, tinted blue below.
    pub const HEAD: usize = 0x7f441e0;
    /// `sign_tracer_01`: the tail drawn twice behind it, tinted blue below.
    pub const TAIL: usize = 0x7f441d8;
    pub const HEAD_BLUE: usize = 0x7f441f0;
    pub const TAIL_BLUE: usize = 0x7f441e8;
}

#[cfg(not(feature = "1_151"))]
mod globals {
    /// `sign_tracer_02`: the segment drawn at the shell, tinted blue below.
    pub const HEAD: usize = 0x800edd8;
    /// `sign_tracer_01`: the tail drawn twice behind it, tinted blue below.
    pub const TAIL: usize = 0x800edd0;
    pub const HEAD_BLUE: usize = 0x800ede8;
    pub const TAIL_BLUE: usize = 0x800ede0;
}

/// Runtime addresses of `HEAD`, `TAIL`, `HEAD_BLUE` and `TAIL_BLUE`, resolved
/// during install so the image base is not assumed.
static GLOBALS: [AtomicUsize; 4] = [
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
];

/// `body->logic` value that selects the blue tracer pair in vanilla (logic 160,
/// the proximity fuze).
const BLUE_LOGIC: i32 = 0xa0;

/// `CMP dword ptr [RSI + logic], 0xa0`: identifies the replaced draw block.
#[cfg(feature = "1_151")]
const DRAW_HEAD: [u8; 10] = [0x81, 0xbe, 0xb4, 0x03, 0x00, 0x00, 0xa0, 0x00, 0x00, 0x00];
#[cfg(not(feature = "1_151"))]
const DRAW_HEAD: [u8; 10] = [0x81, 0xbe, 0xc4, 0x03, 0x00, 0x00, 0xa0, 0x00, 0x00, 0x00];

/// `MOVAPS [RAX - 0x48], XMM8`: a whole instruction that touches no flags and
/// leaves `RCX`/`RDX` alone, so it can be replayed around the capture callback.
const CAPTURE_ORIGINAL: [u8; 5] = [0x44, 0x0f, 0x29, 0x40, 0xb8];

/// `MOVSS XMM2, [RSI + 0x960]` (1.163) / `[RSI + 0x928]` (1.151), replayed
/// whole before the scale callback.
#[cfg(feature = "1_151")]
const SCALE_ORIGINAL: [u8; 8] = [0xf3, 0x0f, 0x10, 0x96, 0x28, 0x09, 0x00, 0x00];
#[cfg(not(feature = "1_151"))]
const SCALE_ORIGINAL: [u8; 8] = [0xf3, 0x0f, 0x10, 0x96, 0x60, 0x09, 0x00, 0x00];

/// `MOV RCX,RSI`: passes the body to the draw/scale callbacks.
const BODY_ARGUMENT: [u8; 3] = [0x48, 0x89, 0xf1];
/// `MOV RDI,[RAX]` (tail); `MOV RBX,[RAX+8]` (head).
const STORE_PAIR: [u8; 7] = [0x48, 0x8b, 0x38, 0x48, 0x8b, 0x58, 0x08];
/// `MOVSS XMM0,[RAX]`; `MULSS XMM6,XMM0`; `MOVSS XMM0,[RAX+4]`; `MULSS XMM8,XMM0`.
const APPLY_SCALE: [u8; 18] = [
    0xf3, 0x0f, 0x10, 0x00, // MOVSS XMM0,[RAX]
    0xf3, 0x0f, 0x59, 0xf0, // MULSS XMM6,XMM0
    0xf3, 0x0f, 0x10, 0x40, 0x04, // MOVSS XMM0,[RAX+4]
    0xf3, 0x44, 0x0f, 0x59, 0xc0, // MULSS XMM8,XMM0
];

/// Reproduces the replaced selection and hands the pair to `RDI`/`RBX`, which
/// the rest of the draw reads. The replaced `CMP` only feeds the two conditional
/// moves, so nothing has to be replayed or preserved.
fn draw_trampoline(address: usize) -> Trampoline {
    let mut code = Trampoline::new();
    code.preserved_call(pick as *const (), &BODY_ARGUMENT, &[], ReturnType::Rax);
    code.bytes(&STORE_PAIR);
    code.relative_jump(address + sites::DRAW_SIZE);
    code
}

/// `RSP` is already 16-byte aligned here and the replayed instruction touches no
/// flags, so the callback runs directly and the prologue continues unchanged.
fn capture_trampoline(address: usize) -> Trampoline {
    let mut code = Trampoline::new();
    code.bytes(&CAPTURE_ORIGINAL);
    code.preserved_call(capture as *const (), &[], &[], ReturnType::None);
    code.relative_jump(address + CAPTURE_ORIGINAL.len());
    code
}

/// Replays the position load, scales the game's length/width registers and
/// resumes at the instruction that consumes them.
fn scale_trampoline(address: usize) -> Trampoline {
    let mut code = Trampoline::new();
    code.bytes(&SCALE_ORIGINAL);
    code.preserved_call(scale as *const (), &BODY_ARGUMENT, &[], ReturnType::Rax);
    code.bytes(&APPLY_SCALE);
    code.relative_jump(address + SCALE_ORIGINAL.len());
    code
}

/// # Safety
///
/// Must run against the selected Steam executable before gameplay starts.
pub unsafe fn patch(
    session: &mut PatchSession,
    module: ProcessModule,
    hooks: Hooks,
) -> patchy::Result {
    LOAD_SPRITE.store(
        module.resolve_rva(sites::LOAD_SPRITE)? as usize,
        Ordering::Relaxed,
    );
    for (slot, rva) in GLOBALS.iter().zip([
        globals::HEAD,
        globals::TAIL,
        globals::HEAD_BLUE,
        globals::TAIL_BLUE,
    ]) {
        slot.store(module.resolve_rva(rva)? as usize, Ordering::Relaxed);
    }

    let capture_address = module.resolve_rva(sites::CAPTURE)?;
    verify(capture_address, &CAPTURE_ORIGINAL)?;
    session.detour_trampoline(
        capture_address,
        CAPTURE_ORIGINAL.len(),
        capture_trampoline(capture_address),
    )?;

    let draw_address = module.resolve_rva(sites::DRAW)?;
    verify(draw_address, &DRAW_HEAD)?;
    session.detour_trampoline(
        draw_address,
        sites::DRAW_SIZE,
        draw_trampoline(draw_address),
    )?;

    let mut prepared = format!("capture {capture_address:#x}, draw {draw_address:#x}");
    if hooks.scale {
        let scale_address = module.resolve_rva(sites::SCALE)?;
        verify(scale_address, &SCALE_ORIGINAL)?;
        session.detour_trampoline(
            scale_address,
            SCALE_ORIGINAL.len(),
            scale_trampoline(scale_address),
        )?;
        prepared.push_str(&format!(", scale {scale_address:#x}"));
    }

    log::debug!("Prepared shell tracer hooks ({prepared})");
    Ok(())
}

/// # Safety
///
/// `address` must be readable for `expected.len()` bytes.
unsafe fn verify(address: usize, expected: &[u8]) -> patchy::Result {
    let actual = unsafe { std::slice::from_raw_parts(address as *const u8, expected.len()) };
    if actual != expected {
        return Err(PatchError::SourceChanged { address });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "1_151")]
    const CONFIG: &str = include_str!("../ammo_extended_1_151.json");
    #[cfg(not(feature = "1_151"))]
    const CONFIG: &str = include_str!("../ammo_extended_1_163.json");

    fn settings(image: Option<&str>, head: Option<&str>, tail: Option<&str>) -> Settings {
        Settings {
            image: image.map(str::to_owned),
            head: head.map(str::to_owned),
            tail: tail.map(str::to_owned),
            ..Settings::default()
        }
    }

    #[test]
    fn shared_image_fills_both_parts_and_overrides_win_per_part() {
        let shared = settings(Some("my_tracer_01"), None, None);
        assert_eq!(shared.head_name(), Some("my_tracer_01"));
        assert_eq!(shared.tail_name(), Some("my_tracer_01"));

        let mixed = settings(Some("my_tracer_01"), Some("my_head_01"), None);
        assert_eq!(mixed.head_name(), Some("my_head_01"));
        assert_eq!(mixed.tail_name(), Some("my_tracer_01"));

        let only_tail = settings(None, None, Some("my_tail_01"));
        assert_eq!(only_tail.head_name(), None);
        assert_eq!(only_tail.tail_name(), Some("my_tail_01"));

        assert_eq!(Settings::default().head_name(), None);
        assert_eq!(Settings::default().tail_name(), None);
    }

    #[test]
    fn colors_parse_from_strings_and_integers() {
        assert_eq!(parse_color("#FF0000"), Ok(0xffff_0000));
        assert_eq!(parse_color("ff0000"), Ok(0xffff_0000));
        assert_eq!(parse_color("#80FF0000"), Ok(0x80ff_0000));
        assert_eq!(parse_color("00000000"), Ok(0));
        assert!(parse_color("").is_err());
        assert!(parse_color("FFF").is_err());
        assert!(parse_color("GGGGGG").is_err());
        assert!(parse_color("#FF00000").is_err());

        let text: Color = serde_json::from_str("\"#11223344\"").unwrap();
        assert_eq!(text, Color(0x1122_3344));
        let short: Color = serde_json::from_str("\"112233\"").unwrap();
        assert_eq!(short, Color(0xff11_2233));
        let number: Color = serde_json::from_str("4278190080").unwrap();
        assert_eq!(number, Color(0xff00_0000));
        assert!(serde_json::from_str::<Color>("4294967296").is_err());
        assert!(serde_json::from_str::<Color>("true").is_err());
        assert!(serde_json::from_str::<Color>("\"nope\"").is_err());
        assert_eq!(
            serde_json::to_string(&Color(0xff11_2233)).unwrap(),
            "\"#FF112233\""
        );
        assert_eq!(Settings::default().argb(), u32::MAX);
        assert_eq!(
            Settings {
                color: Some(Color(1)),
                ..Settings::default()
            }
            .argb(),
            1
        );
    }

    #[test]
    fn scale_precedence_is_per_axis_over_uniform() {
        let default = Settings::default();
        assert_eq!(default.length_factor(), 1.0);
        assert_eq!(default.width_factor(), 1.0);

        let uniform = Settings {
            scale: Some(2.5),
            ..Settings::default()
        };
        assert_eq!(uniform.length_factor(), 2.5);
        assert_eq!(uniform.width_factor(), 2.5);

        // An explicit per-axis value replaces the uniform one, even at 1.0.
        let split = Settings {
            scale: Some(2.5),
            length: Some(0.5),
            width: Some(4.0),
            ..Settings::default()
        };
        assert_eq!(split.length_factor(), 0.5);
        assert_eq!(split.width_factor(), 4.0);

        let reset = Settings {
            scale: Some(5.0),
            length: Some(1.0),
            ..Settings::default()
        };
        assert_eq!(reset.length_factor(), 1.0);
        assert_eq!(reset.width_factor(), 5.0);

        // Values are taken as written: zero and negatives are meaningful.
        let odd = Settings {
            length: Some(0.0),
            width: Some(-1.0),
            ..Settings::default()
        };
        assert_eq!(odd.length_factor(), 0.0);
        assert_eq!(odd.width_factor(), -1.0);
    }

    #[test]
    fn capture_resolves_per_ammo_and_keeps_scale_without_sprites() {
        let mut state = State::default();
        state.ammo.insert(10, settings(Some("custom"), None, None));
        // No loader is installed in tests, so the name resolves to 0 and the
        // body still records the ammo's scale.
        state.capture(100, 10, 100, "ITEM_TEST");
        assert_eq!(
            state.bodies.get(&100).copied(),
            Some(Tracer {
                head: 0,
                tail: 0,
                logic: 100,
                length: 1.0,
                width: 1.0
            })
        );
        assert_eq!(state.sprites.get("custom").map(|s| s.pointer), Some(0));

        state.sprites.insert(
            "head".into(),
            Sprite {
                argb: u32::MAX,
                pointer: 0x1000,
            },
        );
        state.sprites.insert(
            "tail".into(),
            Sprite {
                argb: u32::MAX,
                pointer: 0x2000,
            },
        );
        state.ammo.insert(
            11,
            Settings {
                head: Some("head".into()),
                tail: Some("tail".into()),
                scale: Some(3.0),
                ..Settings::default()
            },
        );
        state.capture(200, 11, 130, "ITEM_TEST");
        assert_eq!(
            state.bodies.get(&200).copied(),
            Some(Tracer {
                head: 0x1000,
                tail: 0x2000,
                logic: 130,
                length: 3.0,
                width: 3.0
            })
        );

        // A body reused by a different shell (or another projectile) is replaced.
        state.ammo.remove(&11);
        state.capture(200, 12, 100, "ITEM_TEST");
        assert!(!state.bodies.contains_key(&200));

        // An unconfigured ammo never enters the map.
        state.capture(300, 99, 100, "ITEM_TEST");
        assert!(!state.bodies.contains_key(&300));
    }

    #[test]
    fn logic_mismatch_does_not_use_a_recycled_body_entry() {
        let tracer = Tracer {
            head: 0x10,
            tail: 0x20,
            logic: 100,
            length: 1.0,
            width: 1.0,
        };
        let mut state = State::default();
        state.bodies.insert(50, tracer);
        assert_eq!(state.tracer(50, 100).map(|t| t.head), Some(0x10));
        assert!(state.tracer(50, 130).is_none());
        assert!(state.tracer(51, 100).is_none());
    }

    #[test]
    fn sprite_names_are_cached_with_the_first_color() {
        let mut state = State::default();
        assert_eq!(state.sprite("absent", 0xffff_ffff), 0);
        assert_eq!(state.sprites.get("absent").map(|s| s.pointer), Some(0));
        state.sprites.insert(
            "absent".into(),
            Sprite {
                argb: 0xffff_ffff,
                pointer: 0x1234,
            },
        );
        assert_eq!(state.sprite("absent", 0xffff_ffff), 0x1234);
        // A second colour cannot take effect on the shared object.
        assert_eq!(state.sprite("absent", 0xff00_0000), 0x1234);
        assert!(state.color_conflicts.contains("absent"));
        assert_eq!(state.sprites.len(), 1);
    }

    #[test]
    fn game_tracer_sprites_are_recognised_by_pointer() {
        let game = [0x100, 0x200, 0x300, 0x400];
        assert!(is_game_tracer(0x100, game));
        assert!(is_game_tracer(0x400, game));
        assert!(!is_game_tracer(0x500, game));
        // A failed load (0) is never one of them.
        assert!(!is_game_tracer(0, [0, 0x200, 0x300, 0x400]));
    }

    #[test]
    fn hooks_are_reported_only_for_configured_ammo() {
        let mut config = crate::config::parse(CONFIG).unwrap();
        assert_eq!(configure(&config.ammos, &config.tracer), Hooks::default());

        config.tracer[0].image = Some("sign_tracer_01".to_owned());
        assert_eq!(
            configure(&config.ammos, &config.tracer),
            Hooks {
                any: true,
                scale: false
            }
        );
        with_state(|state| {
            let ammo = &config.ammos[0] as *const Ammo as usize;
            assert_eq!(state.ammo.len(), 1);
            assert_eq!(state.ammo[&ammo].image.as_deref(), Some("sign_tracer_01"));
        });

        // A scale of exactly 1.0 keeps the scale hook off.
        config.tracer[1].scale = Some(1.0);
        assert_eq!(
            configure(&config.ammos, &config.tracer),
            Hooks {
                any: true,
                scale: false
            }
        );
        // A per-axis value that resolves to 1.0 also keeps it off.
        config.tracer[1].scale = Some(5.0);
        config.tracer[1].length = Some(1.0);
        config.tracer[1].width = Some(1.0);
        assert_eq!(
            configure(&config.ammos, &config.tracer),
            Hooks {
                any: true,
                scale: false
            }
        );
        config.tracer[1].length = None;
        config.tracer[1].width = None;
        assert_eq!(
            configure(&config.ammos, &config.tracer),
            Hooks {
                any: true,
                scale: true
            }
        );

        // Colour alone switches the hooks on (it changes what gets loaded).
        let mut only_color = vec![Settings::default(); 2];
        only_color[1].color = Some(Color(0xffff_0000));
        let ammos = &config.ammos[..2];
        assert_eq!(
            configure(ammos, &only_color),
            Hooks {
                any: true,
                scale: false
            }
        );
        configure(&[], &[]);
    }

    #[test]
    fn vanilla_pair_switches_to_blue_for_the_proximity_fuze_logic() {
        assert_eq!(vanilla_pair(100, 1, 2, 3, 4), (1, 2));
        assert_eq!(vanilla_pair(BLUE_LOGIC, 1, 2, 3, 4), (3, 4));
        assert_eq!(vanilla_pair(130, 1, 2, 3, 4), (1, 2));
        assert_eq!(vanilla_pair(0, 1, 2, 3, 4), (1, 2));
    }

    #[test]
    fn overlay_replaces_only_the_parts_that_resolved() {
        let both = Tracer {
            head: 0x11,
            tail: 0x22,
            logic: 100,
            length: 1.0,
            width: 1.0,
        };
        assert_eq!(overlay(Some(both), (1, 2)), (0x11, 0x22));
        let head_only = Tracer {
            head: 0x11,
            tail: 0,
            ..both
        };
        assert_eq!(overlay(Some(head_only), (1, 2)), (0x11, 2));
        let tail_only = Tracer {
            head: 0,
            tail: 0x22,
            ..both
        };
        assert_eq!(overlay(Some(tail_only), (1, 2)), (1, 0x22));
        assert_eq!(overlay(None, (1, 2)), (1, 2));
    }

    #[test]
    fn draw_trampoline_calls_pick_and_the_capture_trampoline_replays_its_instruction() {
        let address = 0x140000000;
        let bytes = draw_trampoline(address).build(address + 0x1000).unwrap();
        assert_eq!(bytes[0], 0x50); // PUSH RAX, the first preserved-call byte.
        assert!(bytes
            .windows(STORE_PAIR.len())
            .any(|window| window == STORE_PAIR));
        assert_eq!(bytes[bytes.len() - 5], 0xe9);
        let displacement = i32::from_le_bytes(bytes[bytes.len() - 4..].try_into().unwrap());
        assert_eq!(
            (address + 0x1000 + bytes.len()) as i64 + displacement as i64,
            (address + sites::DRAW_SIZE) as i64
        );

        let bytes = capture_trampoline(address).build(address + 0x1000).unwrap();
        assert!(bytes.starts_with(&CAPTURE_ORIGINAL));
        assert_eq!(bytes[CAPTURE_ORIGINAL.len()], 0x50); // PUSH RAX
        assert_eq!(bytes[bytes.len() - 5], 0xe9);

        let bytes = scale_trampoline(address).build(address + 0x1000).unwrap();
        assert!(bytes.starts_with(&SCALE_ORIGINAL));
        assert_eq!(bytes[SCALE_ORIGINAL.len()], 0x50); // PUSH RAX
        let scale_at = bytes.len() - 5 - APPLY_SCALE.len();
        assert_eq!(&bytes[scale_at..bytes.len() - 5], &APPLY_SCALE);
        let displacement = i32::from_le_bytes(bytes[bytes.len() - 4..].try_into().unwrap());
        assert_eq!(
            (address + 0x1000 + bytes.len()) as i64 + displacement as i64,
            (address + SCALE_ORIGINAL.len()) as i64
        );
    }

    #[test]
    fn sites_and_opcodes_match_the_expected_shape() {
        assert_eq!(DRAW_HEAD.len(), 10);
        assert_eq!(DRAW_HEAD[0], 0x81); // CMP dword ptr [RSI + disp32], 0xa0
        assert_eq!(DRAW_HEAD[1], 0xbe);
        assert_eq!(&DRAW_HEAD[6..], &[0xa0, 0x00, 0x00, 0x00]);
        assert_eq!(sites::DRAW_SIZE, 0x28);
        assert_eq!(BODY_ARGUMENT, [0x48, 0x89, 0xf1]);
        assert_eq!(STORE_PAIR, [0x48, 0x8b, 0x38, 0x48, 0x8b, 0x58, 0x08]);
        assert_eq!(CAPTURE_ORIGINAL, [0x44, 0x0f, 0x29, 0x40, 0xb8]);
        assert_eq!(APPLY_SCALE.len(), 18);
        #[cfg(feature = "1_151")]
        {
            assert_eq!(sites::DRAW, 0x49ea2);
            assert_eq!(sites::SCALE, 0x49f43);
            assert_eq!(sites::CAPTURE, 0x50324);
            assert_eq!(sites::LOAD_SPRITE, 0x1fdd50);
            assert_eq!(&DRAW_HEAD[2..6], &[0xb4, 0x03, 0x00, 0x00]);
            assert_eq!(&SCALE_ORIGINAL[4..6], &[0x28, 0x09]);
            assert_eq!(globals::HEAD, 0x7f441e0);
            assert_eq!(globals::TAIL, 0x7f441d8);
            assert_eq!(globals::HEAD_BLUE, 0x7f441f0);
            assert_eq!(globals::TAIL_BLUE, 0x7f441e8);
        }
        #[cfg(not(feature = "1_151"))]
        {
            assert_eq!(sites::DRAW, 0x4c262);
            assert_eq!(sites::SCALE, 0x4c30c);
            assert_eq!(sites::CAPTURE, 0x52aa4);
            assert_eq!(sites::LOAD_SPRITE, 0x21b0a0);
            assert_eq!(&DRAW_HEAD[2..6], &[0xc4, 0x03, 0x00, 0x00]);
            assert_eq!(&SCALE_ORIGINAL[4..6], &[0x60, 0x09]);
            assert_eq!(globals::HEAD, 0x800edd8);
            assert_eq!(globals::TAIL, 0x800edd0);
            assert_eq!(globals::HEAD_BLUE, 0x800ede8);
            assert_eq!(globals::TAIL_BLUE, 0x800ede0);
        }
        // Unresolved globals read as 0 instead of dereferencing null.
        assert_eq!(read_global(0), 0);
    }
}
