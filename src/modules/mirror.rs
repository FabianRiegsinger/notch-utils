//! Mirror: webcam preview. The camera runs only while the tab is visible (freed on hide).
//! macOS asks for camera permission the first time (needs the app bundle's usage string).

use super::{Ctx, Module};
use crate::{MirrorState, NotchWindow};
use slint::{ComponentHandle, SharedPixelBuffer};
use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const MAX_WIDTH: u32 = 640;
const FRAME_INTERVAL: Duration = Duration::from_millis(33);

enum Msg {
    Frame(SharedPixelBuffer<slint::Rgb8Pixel>),
    Status(&'static str),
}

thread_local! {
    static INSTANCE: RefCell<Weak<Mirror>> = RefCell::default();
}

fn post(m: Msg) {
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(mi) = INSTANCE.with(|i| i.borrow().upgrade()) {
            mi.show(m);
        }
    });
}

/// Camera thread: permission (macOS), open, then frames until `stop`.
fn capture(stop: Arc<AtomicBool>, busy: Arc<AtomicBool>) {
    use nokhwa::Camera;
    use nokhwa::pixel_format::RgbFormat;
    use nokhwa::utils::{CameraIndex, RequestedFormat, RequestedFormatType};

    #[cfg(target_os = "macos")]
    {
        let (tx, rx) = std::sync::mpsc::channel();
        nokhwa::nokhwa_initialize(move |granted| {
            let _ = tx.send(granted);
        });
        if !rx.recv_timeout(Duration::from_secs(60)).unwrap_or(false) {
            return post(Msg::Status("denied"));
        }
    }
    let format = RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestFrameRate);
    let mut cam = match Camera::new(CameraIndex::Index(0), format) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("mirror: {e}");
            return post(Msg::Status("none"));
        }
    };
    if let Err(e) = cam.open_stream() {
        log::warn!("mirror: {e}");
        return post(Msg::Status("none"));
    }
    post(Msg::Status("live"));
    let mut next = Instant::now();
    while !stop.load(Ordering::Acquire) {
        let Ok(frame) = cam.frame() else { continue };
        // Drop frames while the UI hasn't shown the previous one, and cap at ~30 fps.
        if busy.load(Ordering::Acquire) || Instant::now() < next {
            continue;
        }
        next = Instant::now() + FRAME_INTERVAL;
        let Ok(img) = frame.decode_image::<RgbFormat>() else {
            continue;
        };
        let img = if img.width() > MAX_WIDTH {
            let h = img.height() * MAX_WIDTH / img.width();
            image::imageops::resize(&img, MAX_WIDTH, h, image::imageops::FilterType::Triangle)
        } else {
            img
        };
        let img = image::imageops::flip_horizontal(&img);
        let buf = SharedPixelBuffer::<slint::Rgb8Pixel>::clone_from_slice(
            img.as_raw(),
            img.width(),
            img.height(),
        );
        busy.store(true, Ordering::Release);
        post(Msg::Frame(buf));
    }
    let _ = cam.stop_stream();
}

pub struct Mirror {
    stop: RefCell<Option<Arc<AtomicBool>>>,
    busy: Arc<AtomicBool>,
    windows: RefCell<Vec<slint::Weak<NotchWindow>>>,
}

impl Mirror {
    pub fn new(_ctx: Ctx) -> Rc<Self> {
        let m = Rc::new(Self {
            stop: RefCell::default(),
            busy: Arc::default(),
            windows: RefCell::default(),
        });
        INSTANCE.with(|i| *i.borrow_mut() = Rc::downgrade(&m));
        m
    }

    fn show(&self, m: Msg) {
        let running = self.stop.borrow().is_some();
        self.windows.borrow_mut().retain(|w| {
            let Some(ui) = w.upgrade() else { return false };
            let g = ui.global::<MirrorState>();
            match &m {
                // A frame that arrives after hide is ignored.
                Msg::Frame(buf) if running => g.set_frame(slint::Image::from_rgb8(buf.clone())),
                Msg::Frame(_) => {}
                Msg::Status(s) => g.set_status((*s).into()),
            }
            true
        });
        if matches!(m, Msg::Frame(_)) {
            self.busy.store(false, Ordering::Release);
        }
    }
}

impl Module for Mirror {
    fn id(&self) -> &'static str {
        "mirror"
    }

    fn title(&self) -> &'static str {
        "Mirror"
    }

    fn attach(&self, ui: &NotchWindow) {
        self.windows.borrow_mut().push(ui.as_weak());
    }

    fn on_show(&self) {
        if self.stop.borrow().is_some() {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        *self.stop.borrow_mut() = Some(stop.clone());
        self.busy.store(false, Ordering::Release);
        self.show(Msg::Status("starting"));
        let busy = self.busy.clone();
        if let Err(e) = std::thread::Builder::new()
            .name("mirror".into())
            .spawn(move || capture(stop, busy))
        {
            log::warn!("mirror: {e}");
        }
    }

    fn on_hide(&self) {
        // The thread stops the stream and frees the camera.
        if let Some(stop) = self.stop.borrow_mut().take() {
            stop.store(true, Ordering::Release);
        }
        self.show(Msg::Status("off"));
    }
}
