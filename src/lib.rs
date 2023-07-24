use ammo::Ammo;
use windows::{ Win32::Foundation::*, Win32::System::SystemServices::* };
use windows::{ core::*, Win32::UI::WindowsAndMessaging::MessageBoxA, };

pub mod ammo;

#[no_mangle]
#[allow(non_snake_case, unused_variables)]
extern "system" fn DllMain(
    dll_module: HMODULE,
    call_reason: u32,
    _: *mut ())
    -> bool
{
    match call_reason {
        DLL_PROCESS_ATTACH => attach(),
        DLL_PROCESS_DETACH => detach(),
        _ => ()
    } 

    true
}

fn attach() {
    let ammo_list_begin = 0x1439426e0 as *mut *mut Ammo;
    let ammo: &mut Ammo = unsafe { (* ammo_list_begin ).as_mut().unwrap() };

    let pcstr_message = PCSTR::from_raw(ammo.item_name.as_ptr());

    unsafe {
        MessageBoxA(HWND(0),
            pcstr_message,
            s!("HEY HEY!"),
            Default::default()
        );
    }
}

fn detach() {
    unsafe {
        MessageBoxA(HWND(0),
            s!("BYE BYE!"),
            s!("test"),
            Default::default()
        );
    }
}