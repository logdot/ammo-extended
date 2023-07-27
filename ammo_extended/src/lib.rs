//! A Highfleet mod to facilitate modifying and adding new calibers to the game

#[cfg(feature = "1_151")]
use highfleet::v1_151::Ammo;

#[cfg(feature = "1_163")]
use highfleet::v1_163::Ammo;

#[cfg(not(any(feature = "1_151", feature = "1_163")))]
use highfleet::v1_163::Ammo;

use std::error::Error;
use std::ffi::c_void;
use std::slice;
use windows::Win32::System::Console::{AllocConsole, FreeConsole};
use windows::Win32::System::LibraryLoader::FreeLibraryAndExitThread;
use windows::Win32::System::Threading::{CreateThread, THREAD_CREATION_FLAGS};
use windows::{Win32::Foundation::*, Win32::System::SystemServices::*};

#[no_mangle]
#[allow(non_snake_case, unused_variables)]
unsafe extern "system" fn DllMain(dll_module: HMODULE, call_reason: u32, _: *mut ()) -> bool {
    match call_reason {
        DLL_PROCESS_ATTACH => unsafe {
            let handle = CreateThread(
                None,
                0,
                Some(attach),
                Some(std::ptr::addr_of!(dll_module).cast() as *const c_void),
                THREAD_CREATION_FLAGS(0),
                None,
            )
            .unwrap();
            CloseHandle(handle)
        },
        DLL_PROCESS_DETACH => FreeConsole(),
        _ => TRUE,
    };

    if let DLL_PROCESS_ATTACH = call_reason {}

    true
}

fn read_config(path: &str) -> Result<Vec<Ammo>, Box<dyn Error>> {
    let file_contents = std::fs::read_to_string(path)?;

    Ok(serde_json::from_str(&file_contents)?)
}

unsafe extern "system" fn attach(handle: *mut c_void) -> u32 {
    AllocConsole();

    let mut ammo_list_begin: *mut *mut Ammo = 0x143a13be0 as *mut *mut Ammo;
    if cfg!(feature = "1_151") {
        ammo_list_begin = 0x1439426e0 as *mut *mut Ammo;
    }

    let ammo_list_end = ammo_list_begin.offset(1);
    let ammo_list_length = (*ammo_list_end).offset_from(*ammo_list_begin) as usize;

    let ammo_list = slice::from_raw_parts_mut(*ammo_list_begin, ammo_list_length);

    if cfg!(debug_assertions) {
        let string = serde_json::to_string_pretty(ammo_list).unwrap();
        println!("{string}");
    }

    let conf_ammos = read_config("Modloader/config/ammo_extended.json")
        .map_err(|err| {
            println!("Failed to read config. Encountered error: \n{err:?}");
            err
        })
        .ok();

    if let Some(mut conf_ammos) = conf_ammos {
        *ammo_list_begin = conf_ammos.as_mut_ptr();

        *ammo_list_end = (*ammo_list_begin).add(conf_ammos.len());

        std::mem::forget(conf_ammos);
    }

    FreeLibraryAndExitThread(HMODULE(handle as _), 0);
}
