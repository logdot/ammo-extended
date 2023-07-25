//! A Highfleet mod to facilitate modifying and adding new calibers to the game

use highfleet::v1_151::Ammo;
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

unsafe extern "system" fn attach(handle: *mut c_void) -> u32 {
    AllocConsole();

    // Read from memory:

    let config_contents = std::fs::read_to_string("Modloader/config/ammo_extended.json")
        .expect("Couldn't find the config file! Where is it?");

    let config_ammos: Vec<Ammo> = match serde_json::from_str(&config_contents) {
        Ok(result) => result,
        Err(err) => panic!("Failed to read config file: \n{:?}", err),
    };

    let ammo_list_begin = 0x1439426e0 as *mut *mut Ammo;
    let ammo_list = unsafe { slice::from_raw_parts_mut(*ammo_list_begin, 32) };

    for (hf_ammo, conf_ammo) in ammo_list.iter_mut().zip(config_ammos) {
        *hf_ammo = conf_ammo;
    }

    // *ammo_list_begin = config_ammos.as_mut_ptr();

    unsafe {
        FreeLibraryAndExitThread(HMODULE(handle as _), 0);
    };
}
