//! Default sink volume via `pactl subscribe` (PulseAudio and PipeWire's pulse server).
//! No native build dependency; if `pactl` is missing the volume HUD is simply off.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

pub struct Watcher(Child);

impl Drop for Watcher {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn query() -> Option<(f32, bool)> {
    let out = |args: &[&str]| {
        Command::new("pactl")
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    };
    let vol = super::parse_pactl_volume(&out(&["get-sink-volume", "@DEFAULT_SINK@"])?)?;
    let muted = super::parse_pactl_mute(&out(&["get-sink-mute", "@DEFAULT_SINK@"])?)?;
    Some((vol, muted))
}

pub fn watch(on_change: impl Fn(f32, bool) + Send + Sync + 'static) -> Option<Watcher> {
    let mut child = Command::new("pactl")
        .arg("subscribe")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| log::warn!("volume HUD off: pactl unavailable ({e})"))
        .ok()?;
    let stdout = child.stdout.take()?;
    std::thread::Builder::new()
        .name("volume".into())
        .spawn(move || {
            let mut last = query();
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                // "Event 'change' on sink #53" / "... on server" (default sink switched)
                if !line.contains("'change'")
                    || !(line.contains(" sink ") || line.contains(" server"))
                {
                    continue;
                }
                let now = query();
                if now.is_some() && now != last {
                    // No HUD for the very first reading.
                    if let (Some((v, m)), Some(_)) = (now, last) {
                        on_change(v, m);
                    }
                    last = now;
                }
            }
        })
        .ok()?;
    Some(Watcher(child))
}
