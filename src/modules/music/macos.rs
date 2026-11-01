//! macOS: MediaRemote through the native bridge (native/nowplaying.m) loaded into Apple's signed
//! /usr/bin/perl, with an AppleScript fallback for Music and Spotify. The library and loader
//! script are embedded and written to the cache dir.

use super::{Artwork, Cmd, Snapshot, Update};
use base64::Engine;
use serde::Deserialize;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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

/// Apps the AppleScript fallback can read (when the MediaRemote bridge is blocked or idle).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptApp {
    Spotify,
    Music,
}

impl ScriptApp {
    const ALL: [ScriptApp; 2] = [ScriptApp::Spotify, ScriptApp::Music];

    fn name(self) -> &'static str {
        match self {
            ScriptApp::Spotify => "Spotify",
            ScriptApp::Music => "Music",
        }
    }

    fn bundle_id(self) -> &'static str {
        match self {
            ScriptApp::Spotify => "com.spotify.client",
            ScriptApp::Music => "com.apple.Music",
        }
    }

    /// Tab-separated: state, title, artist, album, duration, position, artwork URL.
    fn query(self) -> String {
        let art = match self {
            ScriptApp::Spotify => "(artwork url of t)",
            ScriptApp::Music => "\"\"",
        };
        format!(
            "tell application \"{}\"\n\
             if player state is stopped then return \"stopped\"\n\
             set t to current track\n\
             return (player state as text) & tab & (name of t) & tab & (artist of t) & tab & \
             (album of t) & tab & (duration of t) & tab & (player position) & tab & {art}\n\
             end tell",
            self.name()
        )
    }

    /// Only asks apps that are already running: `tell application` would launch them.
    fn running(self) -> bool {
        use objc2_app_kit::NSRunningApplication;
        use objc2_foundation::NSString;
        NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(
            self.bundle_id(),
        ))
        .count()
            > 0
    }
}

/// AppleScript output → (snapshot, artwork URL). Numbers may use a decimal comma (locale).
pub fn parse_script(app: ScriptApp, out: &str) -> Option<(Snapshot, String)> {
    let f: Vec<&str> = out.trim_end_matches('\n').split('\t').collect();
    if f.len() < 7 {
        return None;
    }
    let num = |v: &str| v.trim().replace(',', ".").parse::<f64>().ok();
    // Spotify reports the track length in milliseconds, Music in seconds.
    let duration = num(f[4]).map(|d| {
        if app == ScriptApp::Spotify {
            d / 1000.0
        } else {
            d
        }
    });
    Some((
        Snapshot {
            title: f[1].to_string(),
            artist: f[2].to_string(),
            album: f[3].to_string(),
            duration: duration.filter(|d| *d > 0.0),
            elapsed: num(f[5]).unwrap_or(0.0),
            playing: f[0] == "playing",
            artwork: Artwork::Unchanged,
        },
        f[6].trim().to_string(),
    ))
}

fn osascript(script: &str) -> Option<String> {
    let out = Command::new("/usr/bin/osascript")
        .args(["-e", script])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The first running app with a track, preferring one that plays.
fn script_snapshot() -> Option<(ScriptApp, Snapshot, String)> {
    let mut found: Option<(ScriptApp, Snapshot, String)> = None;
    for app in ScriptApp::ALL.into_iter().filter(|a| a.running()) {
        let Some((snap, art)) = osascript(&app.query()).and_then(|o| parse_script(app, &o)) else {
            continue;
        };
        let playing = snap.playing;
        if found.is_none() || playing {
            found = Some((app, snap, art));
        }
        if playing {
            break;
        }
    }
    found
}

/// Which source the current track came from; commands go there. 0 bridge, 1 Spotify, 2 Music.
static SOURCE: AtomicU8 = AtomicU8::new(0);

#[derive(Default)]
struct Arbiter {
    bridge_track: bool,
    script_track: bool,
}

const POLL: Duration = Duration::from_secs(2);

pub struct Watcher {
    child: Option<Child>,
    stop: Arc<AtomicBool>,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(c) = &mut self.child {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// MediaRemote bridge first; the AppleScript fallback fills in whenever the bridge has no track
/// (blocked by macOS, exited, or idle) and Music or Spotify is running.
pub fn watch(on: impl Fn(Update) + Send + Sync + 'static) -> Option<Watcher> {
    let on: Arc<dyn Fn(Update) + Send + Sync> = Arc::new(on);
    let state = Arc::new(Mutex::new(Arbiter::default()));
    let stop = Arc::new(AtomicBool::new(false));

    let child = files()
        .map_err(|e| log::warn!("now playing bridge: {e}"))
        .ok()
        .and_then(|(lib, script)| {
            Command::new("/usr/bin/perl")
                .arg(&script)
                .arg(&lib)
                .arg("stream")
                // Keep stdin open: the bridge exits when it closes (i.e. when we go away).
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| log::warn!("now playing bridge: {e}"))
                .ok()
        });
    let mut child = child;
    if let Some(out) = child.as_mut().and_then(|c| c.stdout.take()) {
        let (on, state) = (on.clone(), state.clone());
        let _ = std::thread::Builder::new()
            .name("now-playing".into())
            .spawn(move || {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    let Some(u) = parse_line(&line) else { continue };
                    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
                    match &u {
                        Update::Track(_) => {
                            st.bridge_track = true;
                            SOURCE.store(0, Ordering::Release);
                        }
                        _ => st.bridge_track = false,
                    }
                    // An empty/blocked bridge must not hide what the fallback found.
                    let forward = matches!(u, Update::Track(_)) || !st.script_track;
                    drop(st);
                    if forward {
                        on(u);
                    }
                }
                state.lock().unwrap_or_else(|e| e.into_inner()).bridge_track = false;
            });
    }

    let flag = stop.clone();
    let _ = std::thread::Builder::new()
        .name("now-playing-script".into())
        .spawn(move || {
            let mut last_art = String::new();
            while !flag.load(Ordering::Acquire) {
                let bridge_has_track = state.lock().unwrap_or_else(|e| e.into_inner()).bridge_track;
                if !bridge_has_track {
                    match script_snapshot() {
                        Some((app, mut snap, art)) => {
                            if art != last_art {
                                snap.artwork =
                                    fetch_art(&art).map_or(Artwork::Cleared, Artwork::Data);
                                last_art = art;
                            }
                            state.lock().unwrap_or_else(|e| e.into_inner()).script_track = true;
                            SOURCE.store(
                                if app == ScriptApp::Spotify { 1 } else { 2 },
                                Ordering::Release,
                            );
                            on(Update::Track(snap));
                        }
                        None => {
                            let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
                            if std::mem::take(&mut st.script_track) {
                                drop(st);
                                last_art.clear();
                                on(Update::Empty);
                            }
                        }
                    }
                }
                std::thread::sleep(POLL);
            }
        });
    Some(Watcher { child, stop })
}

/// Cover from an https artwork URL (Spotify), max 5 MB.
fn fetch_art(url: &str) -> Option<Vec<u8>> {
    if !url.starts_with("https://") {
        return None;
    }
    let mut r = ureq::get(url).call().ok()?;
    r.body_mut().with_config().limit(5 << 20).read_to_vec().ok()
}

pub fn command(cmd: Cmd) {
    let app = match SOURCE.load(Ordering::Acquire) {
        1 => Some(ScriptApp::Spotify),
        2 => Some(ScriptApp::Music),
        _ => None,
    };
    let child = match app {
        Some(app) => {
            let verb = match cmd {
                Cmd::PlayPause => "playpause",
                Cmd::Next => "next track",
                Cmd::Previous => "previous track",
            };
            Command::new("/usr/bin/osascript")
                .args([
                    "-e",
                    &format!("tell application \"{}\" to {verb}", app.name()),
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
        }
        None => {
            let code = match cmd {
                Cmd::PlayPause => "2",
                Cmd::Next => "4",
                Cmd::Previous => "5",
            };
            let Ok((lib, script)) = files() else { return };
            Command::new("/usr/bin/perl")
                .arg(&script)
                .arg(&lib)
                .arg("command")
                .env("NP_CMD", code)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
        }
    };
    // Reap the short-lived child off the UI thread.
    if let Ok(mut c) = child {
        std::thread::spawn(move || {
            let _ = c.wait();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_applescript_output() {
        let (t, art) = parse_script(
            ScriptApp::Spotify,
            "playing\tAfterglow Avenue\tNeon Harbor\tLights\t148000\t66,5\thttps://i.scdn.co/image/x\n",
        )
        .unwrap();
        assert_eq!(
            (t.title.as_str(), t.duration, t.elapsed, t.playing),
            ("Afterglow Avenue", Some(148.0), 66.5, true)
        );
        assert_eq!(art, "https://i.scdn.co/image/x");
        let (m, _) = parse_script(
            ScriptApp::Music,
            "paused\tSong\tArtist\tAlbum\t201.3\t12.25\t\n",
        )
        .unwrap();
        assert_eq!(
            (m.duration, m.playing),
            (Some(201.3), false),
            "Music reports seconds"
        );
        assert!(parse_script(ScriptApp::Music, "stopped").is_none());
    }

    #[test]
    fn query_mentions_the_app_and_only_reads() {
        let q = ScriptApp::Music.query();
        assert!(q.starts_with("tell application \"Music\""));
        assert!(
            !q.contains("play") || q.contains("player state"),
            "no playback commands in the query"
        );
    }

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
