//! Windows: GlobalSystemMediaTransportControls (WinRT) — covers every app that publishes media
//! sessions, browsers included.

use super::{Artwork, Cmd, Snapshot, Update};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Storage::Streams::DataReader;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

fn read(
    mgr: &Manager,
    want_art: bool,
) -> windows_core::Result<Option<(Snapshot, Option<Vec<u8>>)>> {
    let Ok(session) = mgr.GetCurrentSession() else {
        return Ok(None);
    };
    let props = session.TryGetMediaPropertiesAsync()?.join()?;
    let tl = session.GetTimelineProperties()?;
    let playing = session.GetPlaybackInfo()?.PlaybackStatus()? == Status::Playing;
    let secs = |t: windows::Foundation::TimeSpan| t.Duration as f64 / 10_000_000.0;
    let art = if want_art {
        props.Thumbnail().ok().and_then(|r| {
            let stream = r.OpenReadAsync().ok()?.join().ok()?;
            let size = stream.Size().ok()? as u32;
            let reader = DataReader::CreateDataReader(&stream).ok()?;
            reader.LoadAsync(size).ok()?.join().ok()?;
            let mut buf = vec![0u8; size as usize];
            reader.ReadBytes(&mut buf).ok()?;
            Some(buf)
        })
    } else {
        None
    };
    Ok(Some((
        Snapshot {
            title: props.Title()?.to_string(),
            artist: props.Artist()?.to_string(),
            album: props.AlbumTitle()?.to_string(),
            duration: Some(secs(tl.EndTime()?)).filter(|d| *d > 0.0),
            elapsed: secs(tl.Position()?),
            playing,
            artwork: Artwork::Unchanged,
        },
        art,
    )))
}

pub struct Watcher(Arc<AtomicBool>);

impl Drop for Watcher {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// shortcut: polls the current session once a second instead of subscribing to the session
/// events (CurrentSessionChanged / MediaPropertiesChanged / PlaybackInfoChanged); upgrade if the
/// poll shows up in profiles.
pub fn watch(on: impl Fn(Update) + Send + 'static) -> Option<Watcher> {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    std::thread::Builder::new()
        .name("now-playing".into())
        .spawn(move || {
            // SAFETY: COM init for this thread's lifetime.
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            let mgr = match Manager::RequestAsync().and_then(|op| op.join()) {
                Ok(m) => m,
                Err(e) => return on(Update::Unavailable(e.to_string())),
            };
            let mut last: Option<(String, String, bool)> = None;
            while !flag.load(Ordering::Acquire) {
                match read(&mgr, false) {
                    Ok(Some((mut snap, _))) => {
                        let key = (snap.title.clone(), snap.artist.clone(), snap.playing);
                        if last.as_ref() != Some(&key) {
                            // New track or play state: fetch the thumbnail once.
                            if last.as_ref().is_none_or(|l| l.0 != key.0 || l.1 != key.1) {
                                snap.artwork =
                                    match read(&mgr, true).ok().flatten().and_then(|(_, a)| a) {
                                        Some(bytes) => Artwork::Data(bytes),
                                        None => Artwork::Cleared,
                                    };
                            }
                            last = Some(key);
                            on(Update::Track(snap));
                        }
                    }
                    Ok(None) if last.is_some() => {
                        last = None;
                        on(Update::Empty);
                    }
                    _ => {}
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        })
        .ok()?;
    Some(Watcher(stop))
}

pub fn command(cmd: Cmd) {
    std::thread::spawn(move || {
        // SAFETY: COM init for this short-lived thread.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let run = || -> windows_core::Result<()> {
            let session = Manager::RequestAsync()?.join()?.GetCurrentSession()?;
            match cmd {
                Cmd::PlayPause => session.TryTogglePlayPauseAsync()?.join()?,
                Cmd::Next => session.TrySkipNextAsync()?.join()?,
                Cmd::Previous => session.TrySkipPreviousAsync()?.join()?,
            };
            Ok(())
        };
        if let Err(e) = run() {
            log::warn!("now playing command: {e}");
        }
    });
}
