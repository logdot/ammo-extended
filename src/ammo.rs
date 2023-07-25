//! The Ammo

use serde::{Deserialize, Serialize};

use crate::escadra_string::EscadraString;

/// Represents an Ammo object in Highfleet
#[repr(C)]
#[derive(Serialize, Deserialize, Debug)]
pub struct Ammo {
    /// What reticle to use when firing the ammo.
    ///
    /// The vanilla ammos use one of four values:
    /// - 1: Standard reticle used by most ammos.
    /// - 2: Used by aircraft bombs.
    /// - 3: Used mostly by rockets.
    /// - 4: Used by aircraft ammos.
    pub reticle: u64,
    /// The internal name for the item within Highfleet.
    pub item_name: EscadraString,
    /// The text that displays the shell's kind in the shop.
    ///
    /// For example: "Incendiary".
    pub shell_kind: EscadraString,
    /// The internal text to determine the shell's kind.
    ///
    /// For example: "@INCENDIARY".
    pub shell_kind2: EscadraString,
    /// The text to display for the ammo's milimeter in the shop.
    ///
    /// For example: "57mm".
    pub milimeterage: EscadraString,
    /// The shell name used internally by the game.
    pub shell_name: EscadraString,
    /// What sign to use for the reticle?
    ///
    /// In vanilla it's one of these:
    /// - "sign_ammo_unset" for the standard rounds.
    /// - "sign_ammo_inc_small" for small incendiary rounds.
    /// - "sign_ammo_ap" for armour piercing rounds.
    /// - "sign_ammo_proxy" for proxy rounds.
    /// - "sign_ammo_inc" for standard incendiary rounds.
    /// - "sign_ammo_guided" for lazer guided rounds.
    /// - "sign_ammo_craft" for rounds (bombs, or rockets) used by aircraft.
    pub sign_ammo: EscadraString,
    /// How tall the bullet is in the magazine.
    ///
    /// In the vanilla game it ranges from 16 to 38.
    pub bullet_height: f32,
    padding_cch: u32,
    /// A string with unknown purpose.
    ///
    /// In vanilla it is one of these values:
    /// - "shell_in_small"
    /// - "shell_in_med"
    pub shell_in: EscadraString,
    /// A string with unknown purpose.
    ///
    /// In vanilla it is one of these values:
    /// - "shell_out_tiny"
    /// - "shell_out_small2"
    /// - "shell_out_small2"
    /// - "shell_out_big"
    pub shell_out: EscadraString,
    /// A string with unknown purpose.
    ///
    /// In vanilla it is one of these values:
    /// - "shell_far_med"
    /// - "shell_out_big_far"
    pub shell_far: EscadraString,
    /// Determines if the shell behaves like HE, AP, INC, or LG?
    ///
    /// In vanilla it is one of these values:
    /// - 100: The default
    /// - 130: Rocket and Incendiary?
    /// - 140: Laser Guided
    /// - 160: Proxy
    pub caliber: u32,
    /// The index of the ammo.
    /// A weapon's m_weapon_caliber should match with an ammo index.
    pub index: u32,
    /// The speed of the shell.
    pub speed: f32,
    /// The drag the shell experiences?
    ///
    /// A value between 0 and 1.
    /// In the vanilla game it's either set to 0 or to 0.0007
    pub ap_drag: f32,
    /// The shell's explosive power.
    /// Higher is better.
    pub explosive_power: f32,
    /// The shell's penetrative power.
    /// Higher is better.
    pub penetrative_power: f32,
    /// The shell's incendiary power.
    /// Higher is better.
    ///
    /// By default it is 100.0, where incendiary rounds having it set to 1000.0
    pub incendiary_power: f32,
    /// Value related to incendiary rounds.
    /// It is unknown what it affects.
    pub incendiary_14ch: u32,
    /// Value related to incendiary rounds.
    /// It is unknown what it affects.
    pub incendiary_150h: f32,
    /// Value related to incendiary rounds.
    /// It is unknown what it affects.
    pub incendiary_154h: f32,
    /// Value between 0 and 1.
    /// By default it is 0.5.
    ///
    /// The only exceptions are:
    /// - The NAR122 where it's 0.2
    /// - The 37MM aircraft rounds where it's 0.1
    /// - The 57MM aircraft rounds where it's 0.2
    pub unknown_158h: f32,
    /// Value with unknown purpose.
    /// By default it is 10.
    ///
    /// The only exception being the 57MM aircraft rounds where it's 7.
    pub unknown_15ch: u32,
    /// Value with unknown purpose.
    /// By default it is 0.0.
    ///
    /// The only exceptions are:
    /// - The NAR122 where it's 3.0
    /// - The NAR340 where it's 5.0
    /// - The FAB100 where it's 3.0
    /// - The FAB250 where it's 5.0
    /// - The FAB500 where it's 8.0
    /// - The 37MM aircraft where it's 1.0
    /// - The 57MM aircraft where it's 2.0
    pub unknown_160h: f32,
    padding_164h: u32,
}
