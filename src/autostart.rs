//! "Start at login": LaunchAgent (macOS), HKCU Run key (Windows), XDG autostart (Linux).
//! Only touches the system when the wanted state differs from the current one.

use auto_launch::{AutoLaunch, AutoLaunchBuilder, MacOSLaunchMode};

fn launcher() -> Result<AutoLaunch, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    AutoLaunchBuilder::new()
        .set_app_name("notch-utils")
        // Inside notch-utils.app this is Contents/MacOS/notch-utils, which keeps the bundle's
        // permissions (camera, automation) when started at login.
        .set_app_path(&exe.to_string_lossy())
        .set_macos_launch_mode(MacOSLaunchMode::LaunchAgent)
        .build()
        .map_err(|e| e.to_string())
}

pub fn sync(on: bool) -> Result<(), String> {
    let l = launcher()?;
    let now = l.is_enabled().map_err(|e| e.to_string())?;
    match (now, on) {
        (false, true) => l.enable().map_err(|e| format!("Start at login: {e}")),
        (true, false) => l.disable().map_err(|e| format!("Start at login: {e}")),
        _ => Ok(()),
    }
}
