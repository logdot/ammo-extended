use crate::escadra_string::EscadraString;

#[repr(C)]
#[derive(Debug)]
pub struct Ammo {
    pub reticle: u64,
    pub item_name: EscadraString,
    pub shell_kind: EscadraString,
    pub shell_kind2: EscadraString,
    pub milimeterage: EscadraString,
    pub shell_name: EscadraString,
    pub sign_ammo: EscadraString,
    pub unknown_cch: f32,
    pub padding: u32,
    pub shell_in: EscadraString,
    pub shell_out: EscadraString,
    pub shell_far: EscadraString,
    pub caliber: u32,
    pub index: u32,
    pub speed: f32,
    pub ap_drag: f32,
    pub explosive_power: f32,
    pub penetrative_power: f32,
    pub incendiary_power: f32,
    pub incendiary_14ch: u32,
    pub incendiary_150h: f32,
    pub incendiary_154h: f32,
    pub unknown_158h: f32,
    pub unknown_15ch: u32,
    pub unknown_160h: f32,
    pub unknown_164h: u32,
}
