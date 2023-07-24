#![deny(clippy::pedantic)]
#![allow(clippy::wildcard_imports)]
#![allow(clippy::unreadable_literal)]
#![allow(clippy::ptr_as_ptr)]

use ammo::Ammo;
use windows::Win32::System::LibraryLoader::FreeLibraryAndExitThread;
use windows::Win32::System::Threading::{CreateThread, THREAD_CREATION_FLAGS};
use windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_STYLE;
use windows::{ Win32::Foundation::*, Win32::System::SystemServices::* };
use windows::{ core::*, Win32::UI::WindowsAndMessaging::MessageBoxA, };
use std::ffi::c_void;

pub mod ammo;

#[no_mangle]
#[allow(non_snake_case, unused_variables)]
extern "system" fn DllMain(
    dll_module: HMODULE,
    call_reason: u32,
    _: *mut ())
    -> bool
{
    if let DLL_PROCESS_ATTACH = call_reason {
        unsafe {
            let handle = CreateThread(None, 0, Some(attach), Some(std::ptr::addr_of!(dll_module).cast() as *const c_void), THREAD_CREATION_FLAGS(0), None).unwrap();
            CloseHandle(handle)
        };
    } 

    true
}

unsafe extern "system" fn attach(handle: *mut c_void) -> u32 {
    let ammo_list_begin = 0x1439426e0 as *mut *mut Ammo;
    let ammo: &mut Ammo = unsafe { (* ammo_list_begin ).as_mut().unwrap() };

    let pcstr_message = PCSTR::from_raw(ammo.item_name.as_ptr());

    unsafe {
        MessageBoxA(HWND(0),
            pcstr_message,
            s!("HEY HEY!"),
            MESSAGEBOX_STYLE::default()
        );
    }

    unsafe { FreeLibraryAndExitThread(HMODULE(handle as _), 0); };
}
