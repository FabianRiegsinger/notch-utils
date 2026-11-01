//! Windows probes: GetKeyState, CapabilityAccessManager consent store (in-use apps have a
//! LastUsedTimeStart but no LastUsedTimeStop yet).

use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, RRF_RT_REG_QWORD, RegCloseKey, RegEnumKeyExW, RegGetValueW,
    RegOpenKeyExW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CAPITAL};
use windows::core::{HSTRING, PCWSTR, PWSTR};

pub fn caps_lock() -> Option<bool> {
    // SAFETY: plain query; low bit = toggled.
    Some(unsafe { GetKeyState(VK_CAPITAL.0 as i32) } & 1 != 0)
}

fn qword(key: HKEY, sub: &str, name: &str) -> Option<u64> {
    let mut v = 0u64;
    let mut size = 8u32;
    // SAFETY: 8-byte out buffer for a QWORD.
    unsafe {
        RegGetValueW(
            key,
            &HSTRING::from(sub),
            &HSTRING::from(name),
            RRF_RT_REG_QWORD,
            None,
            Some(&mut v as *mut u64 as *mut _),
            Some(&mut size),
        )
        .ok()
        .ok()?;
    }
    Some(v)
}

/// Any app (packaged or not) currently using `capability` ("microphone" or "webcam").
fn in_use(capability: &str) -> Option<bool> {
    let base = format!(
        r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\{capability}"
    );
    let mut found = false;
    for root in [base.clone(), format!(r"{base}\NonPackaged")] {
        let mut key = HKEY::default();
        // SAFETY: standard registry enumeration; key closed below.
        unsafe {
            if RegOpenKeyExW(
                HKEY_CURRENT_USER,
                &HSTRING::from(root.as_str()),
                Some(0),
                KEY_READ,
                &mut key,
            )
            .is_err()
            {
                continue;
            }
            let mut i = 0u32;
            loop {
                let mut name = [0u16; 512];
                let mut len = name.len() as u32;
                if RegEnumKeyExW(
                    key,
                    i,
                    Some(PWSTR(name.as_mut_ptr())),
                    &mut len,
                    None,
                    Some(PWSTR::null()),
                    None,
                    None,
                )
                .is_err()
                {
                    break;
                }
                let sub = String::from_utf16_lossy(&name[..len as usize]);
                let start = qword(key, &sub, "LastUsedTimeStart").unwrap_or(0);
                let stop = qword(key, &sub, "LastUsedTimeStop").unwrap_or(1);
                found |= start > 0 && stop == 0;
                i += 1;
            }
            let _ = RegCloseKey(key);
        }
    }
    let _ = PCWSTR::null();
    Some(found)
}

pub fn mic_in_use() -> Option<bool> {
    in_use("microphone")
}

pub fn camera_in_use() -> Option<bool> {
    in_use("webcam")
}

pub fn focus_on() -> Option<bool> {
    None
}
