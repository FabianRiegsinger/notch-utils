//! Linux: MPRIS through `playerctl --follow` (no native build dependency).

use super::{Artwork, Cmd, Snapshot, Update};
use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, Stdio};

const FORMAT: &str = "{{status}}\t{{title}}\t{{artist}}\t{{album}}\t{{mpris:length}}\t{{position}}\t{{mpris:artUrl}}";

/// One `playerctl` line → (snapshot without artwork, art URL).
pub fn parse_line(s: &str) -> Option<(Update, String)> {
    let f: Vec<&str> = s.split('\t').collect();
    if f.len() < 7 {
        return None;
    }
    if f[0].is_empty() || f[0] == "Stopped" && f[1].is_empty() {
        return Some((Update::Empty, String::new()));
    }
    let micros = |v: &str| v.trim().parse::<f64>().ok().map(|m| m / 1_000_000.0);
    Some((
        Update::Track(Snapshot {
            title: f[1].to_string(),
            artist: f[2].to_string(),
            album: f[3].to_string(),
            duration: micros(f[4]).filter(|d| *d > 0.0),
            elapsed: micros(f[5]).unwrap_or(0.0),
            playing: f[0] == "Playing",
            artwork: Artwork::Unchanged,
        }),
        f[6].trim().to_string(),
    ))
}

/// Cover bytes from an MPRIS art URL (file:// or http(s)://), max 10 MB.
fn fetch_art(url: &str) -> Option<Vec<u8>> {
    if let Some(path) = url.strip_prefix("file://") {
        let path = path.replace("%20", " ");
        let mut buf = vec![];
        std::fs::File::open(path)
            .ok()?
            .take(10 << 20)
            .read_to_end(&mut buf)
            .ok()?;
        return Some(buf);
    }
    if url.starts_with("https://") || url.starts_with("http://") {
        let mut r = ureq::get(url).call().ok()?;
        return r
            .body_mut()
            .with_config()
            .limit(10 << 20)
            .read_to_vec()
            .ok();
    }
    None
}

pub struct Watcher(Child);

impl Drop for Watcher {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn watch(on: impl Fn(Update) + Send + 'static) -> Option<Watcher> {
    let mut child = Command::new("playerctl")
        .args(["--follow", "metadata", "--format", FORMAT])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| log::warn!("now playing off: playerctl unavailable ({e})"))
        .ok()?;
    let out = child.stdout.take()?;
    std::thread::Builder::new()
        .name("now-playing".into())
        .spawn(move || {
            let mut last_art = String::new();
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                let Some((mut u, art)) = parse_line(&line) else {
                    continue;
                };
                if let Update::Track(t) = &mut u
                    && art != last_art
                {
                    t.artwork = match fetch_art(&art) {
                        Some(bytes) => Artwork::Data(bytes),
                        None => Artwork::Cleared,
                    };
                    last_art = art;
                }
                on(u);
            }
            on(Update::Unavailable("playerctl exited".into()));
        })
        .ok()?;
    Some(Watcher(child))
}

pub fn command(cmd: Cmd) {
    let arg = match cmd {
        Cmd::PlayPause => "play-pause",
        Cmd::Next => "next",
        Cmd::Previous => "previous",
    };
    if let Ok(mut c) = Command::new("playerctl")
        .arg(arg)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        std::thread::spawn(move || {
            let _ = c.wait();
        });
    }
}
