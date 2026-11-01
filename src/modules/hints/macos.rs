//! macOS probes: CGEventSource flags, CoreAudio / CoreMediaIO "running somewhere", DND store.

use std::ffi::c_void;

#[repr(C)]
struct Address {
    selector: u32,
    scope: u32,
    element: u32,
}

const fn cc(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}
const GLOBAL: u32 = cc(b"glob");
const RUNNING_SOMEWHERE: u32 = cc(b"gone");

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceFlagsState(state: i32) -> u64;
}

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectGetPropertyData(
        id: u32,
        a: *const Address,
        qs: u32,
        q: *const c_void,
        size: *mut u32,
        data: *mut c_void,
    ) -> i32;
}

#[link(name = "CoreMediaIO", kind = "framework")]
unsafe extern "C" {
    fn CMIOObjectGetPropertyDataSize(
        id: u32,
        a: *const Address,
        qs: u32,
        q: *const c_void,
        size: *mut u32,
    ) -> i32;
    fn CMIOObjectGetPropertyData(
        id: u32,
        a: *const Address,
        qs: u32,
        q: *const c_void,
        size: u32,
        used: *mut u32,
        data: *mut c_void,
    ) -> i32;
}

pub fn caps_lock() -> Option<bool> {
    // SAFETY: plain query; 1 = kCGEventSourceStateHIDSystemState. 0x10000 = alpha shift (Caps Lock).
    Some(unsafe { CGEventSourceFlagsState(1) } & 0x0001_0000 != 0)
}

fn audio_u32(id: u32, selector: u32) -> Option<u32> {
    let a = Address {
        selector,
        scope: GLOBAL,
        element: 0,
    };
    let (mut v, mut size) = (0u32, 4u32);
    // SAFETY: 4-byte out buffer.
    let st = unsafe {
        AudioObjectGetPropertyData(
            id,
            &a,
            0,
            std::ptr::null(),
            &mut size,
            &mut v as *mut u32 as *mut c_void,
        )
    };
    (st == 0).then_some(v)
}

pub fn mic_in_use() -> Option<bool> {
    let input = audio_u32(1, cc(b"dIn "))?; // default input device on the system object
    audio_u32(input, RUNNING_SOMEWHERE).map(|v| v != 0)
}

pub fn camera_in_use() -> Option<bool> {
    let devices = Address {
        selector: cc(b"dev#"),
        scope: GLOBAL,
        element: 0,
    };
    let mut size = 0u32;
    // SAFETY: size query, then a buffer of exactly that many bytes.
    unsafe {
        if CMIOObjectGetPropertyDataSize(1, &devices, 0, std::ptr::null(), &mut size) != 0 {
            return None;
        }
        let mut ids = vec![0u32; size as usize / 4];
        let mut used = 0u32;
        if CMIOObjectGetPropertyData(
            1,
            &devices,
            0,
            std::ptr::null(),
            size,
            &mut used,
            ids.as_mut_ptr() as *mut c_void,
        ) != 0
        {
            return None;
        }
        let running = Address {
            selector: RUNNING_SOMEWHERE,
            scope: GLOBAL,
            element: 0,
        };
        Some(ids.iter().any(|&id| {
            let mut v = 0u32;
            CMIOObjectGetPropertyData(
                id,
                &running,
                0,
                std::ptr::null(),
                4,
                &mut used,
                &mut v as *mut u32 as *mut c_void,
            ) == 0
                && v != 0
        }))
    }
}

/// Focus is only readable from the Do Not Disturb store, which needs Full Disk Access.
/// None (no hint) when it can't be read.
pub fn focus_on() -> Option<bool> {
    let path = directories::BaseDirs::new()?
        .home_dir()
        .join("Library/DoNotDisturb/DB/Assertions.json");
    let text = std::fs::read_to_string(path).ok()?;
    super::focus_from_assertions(&text)
}

#[cfg(test)]
mod tests {
    /// Reads the real system state (no prompts): `cargo test -- --ignored probes_answer`.
    #[test]
    #[ignore]
    fn probes_answer() {
        let (caps, mic, cam) = (
            super::caps_lock(),
            super::mic_in_use(),
            super::camera_in_use(),
        );
        println!(
            "caps={caps:?} mic={mic:?} camera={cam:?} focus={:?} online={:?}",
            super::focus_on(),
            super::super::online()
        );
        assert!(caps.is_some() && mic.is_some() && cam.is_some());
    }
}
