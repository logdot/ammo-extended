use windows::{ Win32::Foundation::*, Win32::System::SystemServices::* };
use windows::{ core::*, Win32::UI::WindowsAndMessaging::MessageBoxA, };

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
    unsafe {
        MessageBoxA(HWND(0),
            s!("HEY HEY!"),
            s!("test"),
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