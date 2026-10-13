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

highfleet_mod_api::export_logger!();

use patchy::{PatchError, PatchSession, ProcessModule};
use std::error::Error;
use std::ffi::{c_char, CStr};
use std::slice;

unsafe fn prepare_patches(session: &mut PatchSession, module: ProcessModule) -> patchy::Result {
    // Equivalent to `MOV EAX, dword ptr [R15 + 0xCC]
    let switch_instruction: [u8; 7] = [0x41, 0x8b, 0x87, 0xcc, 0x00, 00, 00];

    let (switch_rva, original_switch) = if cfg!(feature = "1_151") {
        (0x30a37, [0x41, 0x8b, 0x87, 0x34, 0x01, 0x00, 0x00])
    } else {
        (0x302c8, [0x41, 0x8b, 0x87, 0x54, 0x01, 0x00, 0x00])
    };
    let switch_addr = module.resolve_rva(switch_rva)?;
    let ttl = missile_ttl::patch(cfg!(feature = "1_151"));
    let ttl_addr = module.resolve_rva(ttl.rva)?;
    for (address, expected) in [
        (switch_addr, original_switch.as_slice()),
        (ttl_addr, ttl.original.as_slice()),
    ] {
        if slice::from_raw_parts(address as *const u8, expected.len()) != expected {
            return Err(PatchError::SourceChanged { address });
        }
    }
    session.overwrite(switch_addr, &switch_instruction)?;
    session.overwrite(ttl_addr, &ttl.replacement)?;
    Ok(())
}

fn read_config(path: &str) -> Result<Vec<Ammo>, Box<dyn Error>> {
    let file_contents = std::fs::read_to_string(path)?;

    Ok(config::parse(&file_contents)?)
}

#[no_mangle]
unsafe extern "C" fn init() -> bool {
    attach()
}

#[no_mangle]
unsafe extern "C" fn version(version: *const c_char) -> bool {
    let version = CStr::from_ptr(version).to_str().unwrap();
    if cfg!(feature = "1_151") {
        version == "Steam 1.151"
    } else if cfg!(feature = "1_163") {
        version == "Steam 1.163"
    } else if version == "Gog 1.163" {
        error!("Gog 1.163 detected");
        error!("Your game will crash. Ammo Extended only supports steam versions of the game.");
        false
    } else {
        false
    }
}

unsafe fn attach() -> bool {
    let module = match ProcessModule::main() {
        Ok(module) => module,
        Err(err) => {
            error!("Failed to locate game module: {err}");
            return false;
        }
    };
    let mut patches = PatchSession::new();
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

    if let Some(mut conf_ammos) = conf_ammos {
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

        debug!("Ammo behaviour switch and configured missile TTL patches installed");

        true
    } else {
        false
    }
}
