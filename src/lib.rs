//! A Highfleet mod to facilitate modifying and adding new calibers to the game

use ammo::Ammo;
use core::slice;
use std::ffi::c_void;
use windows::Win32::System::Console::{AllocConsole, FreeConsole};
use windows::Win32::System::LibraryLoader::FreeLibraryAndExitThread;
use windows::Win32::System::Threading::{CreateThread, THREAD_CREATION_FLAGS};
use windows::{Win32::Foundation::*, Win32::System::SystemServices::*};

mod ammo;
mod escadra_string;

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

    let ammo_list_begin = 0x1439426e0 as *mut *mut Ammo;
    let ammo_list = unsafe { slice::from_raw_parts_mut(*ammo_list_begin, 32) };

    for ammo in ammo_list {
        // ammo.item_name
        //     .set_string("fuck shit".to_string().as_mut_str());

        println!("{ammo:#?}");
        println!();
        println!("------------------");
        println!();

        if ammo.item_name.get_string() == "ITEM_AMMO_100MM" {
            ammo.index = 70;
            ammo.explosive_power = 0.3;
        }
    }

    unsafe {
        FreeLibraryAndExitThread(HMODULE(handle as _), 0);
    };
}
