//! Linux probes: keyboard LEDs in sysfs, ALSA capture state, open /dev/video* handles.

fn read(p: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(p).ok()
}

pub fn caps_lock() -> Option<bool> {
    let mut seen = false;
    for e in std::fs::read_dir("/sys/class/leds").ok()?.flatten() {
        if e.file_name().to_string_lossy().contains("capslock") {
            seen = true;
            if read(&e.path().join("brightness")).is_some_and(|b| b.trim() != "0") {
                return Some(true);
            }
        }
    }
    seen.then_some(false)
}

pub fn mic_in_use() -> Option<bool> {
    // /proc/asound/cardN/pcmMc/subK/status says "state: RUNNING" while capturing.
    let mut any = false;
    for card in std::fs::read_dir("/proc/asound").ok()?.flatten() {
        let Ok(pcms) = std::fs::read_dir(card.path()) else {
            continue;
        };
        for pcm in pcms
            .flatten()
            .filter(|p| p.file_name().to_string_lossy().ends_with('c'))
        {
            let Ok(subs) = std::fs::read_dir(pcm.path()) else {
                continue;
            };
            for sub in subs.flatten() {
                any |= read(&sub.path().join("status")).is_some_and(|s| s.contains("RUNNING"));
            }
        }
    }
    Some(any)
}

/// shortcut: scans /proc/*/fd for /dev/video* (only processes we may read); PipeWire's camera
/// portal would be the event-based way.
pub fn camera_in_use() -> Option<bool> {
    for p in std::fs::read_dir("/proc").ok()?.flatten() {
        if !p
            .file_name()
            .to_string_lossy()
            .bytes()
            .all(|b| b.is_ascii_digit())
        {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(p.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            if std::fs::read_link(fd.path())
                .is_ok_and(|t| t.to_string_lossy().starts_with("/dev/video"))
            {
                return Some(true);
            }
        }
    }
    Some(false)
}

pub fn focus_on() -> Option<bool> {
    None
}
