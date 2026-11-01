//! macOS: MediaRemote through the native bridge (native/nowplaying.m) loaded into Apple's signed
//! /usr/bin/perl. The library and loader script are embedded and written to the cache dir.

use super::{Artwork, Cmd, Snapshot, Update};
use base64::Engine;
use serde::Deserialize;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

const LIB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/libnotchnp.dylib"));
const SCRIPT: &str = include_str!("../../../native/nowplaying.pl");

#[derive(Deserialize)]
struct Line {
    error: Option<String>,
    empty: Option<bool>,
    #[serde(default)]
    title: String,
    #[serde(default)]
    artist: String,
    #[serde(default)]
    album: String,
    duration: Option<f64>,
    #[serde(default)]
    elapsed: f64,
    #[serde(default)]
    playing: bool,
    /// Base64; "" = artwork removed; absent = unchanged.
    artwork: Option<String>,
}

/// One JSON line from the bridge.
pub fn parse_line(s: &str) -> Option<Update> {
    let l: Line = serde_json::from_str(s).ok()?;
    if let Some(e) = l.error {
        return Some(Update::Unavailable(e));
    }
    if l.empty == Some(true) {
        return Some(Update::Empty);
    }
    let artwork = match l.artwork.as_deref() {
        None => Artwork::Unchanged,
        Some("") => Artwork::Cleared,
        Some(b64) => match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(bytes) => Artwork::Data(bytes),
            Err(_) => Artwork::Unchanged,
        },
    };
    Some(Update::Track(Snapshot {
        title: l.title,
        artist: l.artist,
        album: l.album,
        duration: l.duration.filter(|d| *d > 0.0),
        elapsed: l.elapsed.max(0.0),
        playing: l.playing,
        artwork,
    }))
}

/// Write the embedded bridge to the cache dir (content-addressed, so upgrades get a new file).
fn files() -> std::io::Result<(PathBuf, PathBuf)> {
    let dir = directories::ProjectDirs::from("", "", "notch-utils")
        .map(|d| d.cache_dir().to_path_buf())
        .unwrap_or_else(std::env::temp_dir)
        .join("nowplaying");
    std::fs::create_dir_all(&dir)?;
    let hash = LIB.iter().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ *b as u64).wrapping_mul(0x100000001b3)
    });
    let lib = dir.join(format!("libnotchnp-{hash:016x}.dylib"));
    let script = dir.join("nowplaying.pl");
    if !lib.exists() {
        std::fs::write(&lib, LIB)?;
    }
    std::fs::write(&script, SCRIPT)?;
    Ok((lib, script))
}

pub struct Watcher(Child);

impl Drop for Watcher {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn watch(on: impl Fn(Update) + Send + 'static) -> Option<Watcher> {
    let (lib, script) = files().map_err(|e| log::warn!("now playing: {e}")).ok()?;
    let mut child = Command::new("/usr/bin/perl")
        .arg(&script)
        .arg(&lib)
        .arg("stream")
        // Keep stdin open: the bridge exits when it closes (i.e. when we go away).
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| log::warn!("now playing: {e}"))
        .ok()?;
    let out = child.stdout.take()?;
    std::thread::Builder::new()
        .name("now-playing".into())
        .spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if let Some(u) = parse_line(&line) {
                    on(u);
                }
            }
            on(Update::Unavailable("bridge exited".into()));
        })
        .ok()?;
    Some(Watcher(child))
}

pub fn command(cmd: Cmd) {
    let code = match cmd {
        Cmd::PlayPause => "2",
        Cmd::Next => "4",
        Cmd::Previous => "5",
    };
    let Ok((lib, script)) = files() else { return };
    let r = Command::new("/usr/bin/perl")
        .arg(&script)
        .arg(&lib)
        .arg("command")
        .env("NP_CMD", code)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    // Reap the short-lived child off the UI thread.
    if let Ok(mut c) = r {
        std::thread::spawn(move || {
            let _ = c.wait();
        });
    }
    let _ = std::io::stdout().flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bridge_lines() {
        assert!(matches!(
            parse_line(r#"{"empty":true}"#),
            Some(Update::Empty)
        ));
        assert!(matches!(
            parse_line(r#"{"error":"MediaRemote unavailable"}"#),
            Some(Update::Unavailable(_))
        ));
        let Some(Update::Track(t)) = parse_line(
            r#"{"title":"Afterglow Avenue","artist":"Neon Harbor","album":"","playing":true,"duration":148,"elapsed":66.5,"artwork":"aGVsbG8="}"#,
        ) else {
            panic!()
        };
        assert_eq!(
            (t.title.as_str(), t.duration, t.playing),
            ("Afterglow Avenue", Some(148.0), true)
        );
        assert!(matches!(t.artwork, Artwork::Data(ref b) if b == b"hello"));
        let Some(Update::Track(t)) = parse_line(r#"{"title":"x","playing":false,"artwork":""}"#)
        else {
            panic!()
        };
        assert!(matches!(t.artwork, Artwork::Cleared));
        assert_eq!(t.duration, None);
        assert!(parse_line("garbage").is_none());
    }
}
