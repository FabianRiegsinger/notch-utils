//! Prevent idle sleep (display stays on) while an `Inhibitor` lives. Dropping it releases.

#[cfg(target_os = "macos")]
mod imp {
    use objc2_foundation::NSString;
    use std::ffi::c_void;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPMAssertionCreateWithName(
            kind: *const c_void,
            level: u32,
            name: *const c_void,
            id: *mut u32,
        ) -> i32;
        fn IOPMAssertionRelease(id: u32) -> i32;
    }

    pub struct Inhibitor(u32);

    impl Inhibitor {
        pub fn new() -> Option<Self> {
            // NSString is toll-free bridged to CFString.
            let kind = NSString::from_str("PreventUserIdleDisplaySleep");
            let name = NSString::from_str("notch-utils Keep Awake");
            let mut id = 0u32;
            // SAFETY: valid CFString pointers for the call; 255 = kIOPMAssertionLevelOn.
            let r = unsafe {
                IOPMAssertionCreateWithName(
                    &*kind as *const NSString as *const c_void,
                    255,
                    &*name as *const NSString as *const c_void,
                    &mut id,
                )
            };
            (r == 0).then_some(Self(id))
        }
    }

    impl Drop for Inhibitor {
        fn drop(&mut self) {
            // SAFETY: id came from a successful IOPMAssertionCreateWithName.
            unsafe { IOPMAssertionRelease(self.0) };
        }
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use std::sync::mpsc;
    use windows::Win32::System::Power::{
        ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
    };

    /// The execution state belongs to a thread, so a dedicated thread holds it.
    pub struct Inhibitor(Option<mpsc::Sender<()>>);

    impl Inhibitor {
        pub fn new() -> Option<Self> {
            let (tx, rx) = mpsc::channel::<()>();
            std::thread::Builder::new()
                .name("keep-awake".into())
                .spawn(move || {
                    // SAFETY: plain Win32 calls on this thread.
                    unsafe {
                        SetThreadExecutionState(
                            ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED,
                        )
                    };
                    let _ = rx.recv(); // until the Inhibitor is dropped
                    unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
                })
                .ok()?;
            Some(Self(Some(tx)))
        }
    }

    impl Drop for Inhibitor {
        fn drop(&mut self) {
            if let Some(tx) = self.0.take() {
                let _ = tx.send(());
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::process::{Child, Command, Stdio};

    pub struct Inhibitor(Child);

    impl Inhibitor {
        pub fn new() -> Option<Self> {
            Command::new("systemd-inhibit")
                .args([
                    "--what=idle",
                    "--who=notch-utils",
                    "--why=Keep Awake",
                    "sleep",
                    "infinity",
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| log::warn!("keep awake: systemd-inhibit unavailable ({e})"))
                .ok()
                .map(Self)
        }
    }

    impl Drop for Inhibitor {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

pub use imp::Inhibitor;

#[cfg(all(test, target_os = "macos"))]
mod tests {
    fn assertions() -> String {
        let out = std::process::Command::new("pmset").args(["-g", "assertions"]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Touches the real power state for a moment: run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn assertion_is_visible_and_released() {
        let i = super::Inhibitor::new().expect("IOPMAssertionCreateWithName");
        assert!(assertions().contains("notch-utils Keep Awake"));
        drop(i);
        assert!(!assertions().contains("notch-utils Keep Awake"));
    }
}
