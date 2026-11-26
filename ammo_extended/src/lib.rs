//! A Highfleet mod to facilitate modifying and adding new calibers to the game

#[cfg(feature = "1_151")]
use highfleet::v1_151::Ammo;

#[cfg(feature = "1_163")]
use highfleet::v1_163::Ammo;

#[cfg(not(any(feature = "1_151", feature = "1_163")))]
use highfleet::v1_163::Ammo;
use log::{debug, error, trace};

mod config;
mod missile_ttl;
mod rocket_sound;
mod rocket_speed;
#[cfg(not(feature = "1_151"))]
mod shop;
mod shrapnel;

highfleet_mod_api::export_logger!();
patchy::export_patchy!();

use patchy::{PatchSession, ProcessModule};
use std::error::Error;
use std::ffi::{c_char, CStr};
use std::slice;
use std::sync::atomic::{AtomicBool, Ordering};

static VERSION_SUPPORTED: AtomicBool = AtomicBool::new(false);

unsafe fn prepare_patches(session: &mut PatchSession, module: ProcessModule) -> patchy::Result {
    // Equivalent to `MOV EAX, dword ptr [R15 + 0xCC]`.
    let switch_instruction: [u8; 7] = [0x41, 0x8b, 0x87, 0xcc, 0x00, 00, 00];

    let switch_rva = if cfg!(feature = "1_151") {
        0x30a37
    } else {
        0x302c8
    };
    session.overwrite(module.resolve_rva(switch_rva)?, &switch_instruction)?;
    missile_ttl::patch(session, module)?;
    rocket_speed::patch(session, module)?;
    rocket_sound::patch(session, module)
}

fn read_config(path: &str) -> Result<config::Config, Box<dyn Error>> {
    let file_contents = std::fs::read_to_string(path)?;

    Ok(config::parse(&file_contents)?)
}

#[no_mangle]
unsafe extern "C" fn init() -> bool {
    attach()
}

#[no_mangle]
unsafe extern "C" fn version(version: *const c_char) -> bool {
    VERSION_SUPPORTED.store(false, Ordering::Release);
    if version.is_null() {
        return false;
    }
    let Ok(version) = CStr::from_ptr(version).to_str() else {
        return false;
    };
    let supported = if cfg!(feature = "1_151") {
        version == "Steam 1.151"
    } else if version == "Gog 1.163" {
        error!("Gog 1.163 detected");
        error!("Ammo Extended only supports Steam versions; initialization will be refused.");
        false
    } else {
        // Match the no-feature fallback used by the layouts and patch sites.
        version == "Steam 1.163"
    };
    VERSION_SUPPORTED.store(supported, Ordering::Release);
    supported
}

unsafe fn attach() -> bool {
    // The Modloader hashes the executable but still calls init() on a mismatch.
    // The patch sites below assume the verified image has not been modified externally.
    if !VERSION_SUPPORTED.load(Ordering::Acquire) {
        error!("Refusing to install Ammo Extended on an unsupported game version");
        return false;
    }

    let module = match ProcessModule::main() {
        Ok(module) => module,
        Err(err) => {
            error!("Failed to locate game module: {err}");
            return false;
        }
    };
    // The Modloader only offers its shared Patchy service through `export_patchy!`.
    let mut patches = match PatchSession::new() {
        Ok(session) => session,
        Err(err) => {
            error!("Failed to open a Modloader patch session: {err}");
            return false;
        }
    };
    if let Err(err) = prepare_patches(&mut patches, module) {
        error!("Failed to prepare ammo patches: {err}");
        return false;
    }
    let ammo_list_rva = if cfg!(feature = "1_151") {
        0x39426e0
    } else {
        0x3a13be0
    };
    let ammo_list_begin = match module.resolve_rva(ammo_list_rva) {
        Ok(address) => address as *mut *mut Ammo,
        Err(err) => {
            error!("Failed to locate ammo list: {err}");
            return false;
        }
    };
    debug!("Ammo list begin: {ammo_list_begin:#?}");

    let ammo_list_end = ammo_list_begin.offset(1);
    let ammo_list_length = (*ammo_list_end).offset_from(*ammo_list_begin) as usize;

    let ammo_list = slice::from_raw_parts_mut(*ammo_list_begin, ammo_list_length);

    if cfg!(debug_assertions) {
        // Seed mod-owned behavior from the vanilla spawning identifier before dumping config.
        ammo_list.iter_mut().for_each(|ammo| {
            ammo.padding_cch = ammo.caliber as u32;
            #[cfg(feature = "1_151")]
            {
                // Vanilla padding has no lifetime meaning; seed the mod's missile default.
                ammo.padding_164h = 10.0f32.to_bits();
            }
        });

        let string = config::serialize(ammo_list).unwrap();
        trace!("{string}");
    }

    debug!("Reading config file");
    let conf_ammos = read_config("Modloader/config/ammo_extended.json")
        .map_err(|err| {
            error!("Failed to read config. Encountered error: \n{err:?}");
            err
        })
        .ok();

    if let Some(config) = conf_ammos {
        let mut conf_ammos = config.ammos;
        if shrapnel::configure(&conf_ammos, &config.shrapnel) {
            if let Err(err) = shrapnel::patch(&mut patches, module) {
                error!("Failed to prepare shrapnel patches: {err}");
                return false;
            }
        }

        #[cfg(all(not(feature = "1_151"), target_os = "windows"))]
        if let Err(err) = shop::patch(&mut patches, module, ammo_list, &conf_ammos) {
            error!("Failed to prepare custom ammo shop: {err}");
            return false;
        }

        if let Err(err) = patches.install_permanently() {
            error!("Failed to install ammo patches: {err}");
            return false;
        }

        debug!("Dropping old ammo list");
        ammo_list.iter().for_each(std::mem::drop);

        *ammo_list_begin = conf_ammos.as_mut_ptr();

        *ammo_list_end = (*ammo_list_begin).add(conf_ammos.len());

        std::mem::forget(conf_ammos);
        debug!("New ammo list set");

        debug!(
            "Ammo patches installed (custom shop on 1.163; shrapnel hooks only when configured)"
        );

        true
    } else {
        false
    }
}

#[cfg(test)]
mod version_tests {
    use super::*;

    #[test]
    fn version_gate_matches_layout_selection_and_fails_closed() {
        unsafe {
            assert_eq!(version(c"Steam 1.151".as_ptr()), cfg!(feature = "1_151"));
            assert_eq!(version(c"Steam 1.163".as_ptr()), !cfg!(feature = "1_151"));
            assert_eq!(
                VERSION_SUPPORTED.load(Ordering::Acquire),
                !cfg!(feature = "1_151")
            );
            assert!(!version(c"Gog 1.163".as_ptr()));
            assert!(!VERSION_SUPPORTED.load(Ordering::Acquire));
            assert!(!version(c"unknown".as_ptr()));
            assert!(!version(std::ptr::null()));
        }
    }
}
