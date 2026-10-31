//! Default output volume via IAudioEndpointVolume change notifications (COM, no polling).

use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
};
use windows::Win32::Media::Audio::{
    AUDIO_VOLUME_NOTIFICATION_DATA, IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};

#[windows_core::implement(IAudioEndpointVolumeCallback)]
struct Callback {
    on_change: Box<dyn Fn(f32, bool) + Send + Sync>,
}

impl IAudioEndpointVolumeCallback_Impl for Callback_Impl {
    fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows_core::Result<()> {
        // SAFETY: Windows passes a valid notification struct for the duration of the call.
        if let Some(d) = unsafe { data.as_ref() } {
            (self.on_change)(d.fMasterVolume, d.bMuted.as_bool());
        }
        Ok(())
    }
}

pub struct Watcher;

/// shortcut: follows the default device at startup only; switching output devices needs an
/// IMMNotificationClient. Upgrade when users report it.
pub fn watch(on_change: impl Fn(f32, bool) + Send + Sync + 'static) -> Option<Watcher> {
    let on_change: Box<dyn Fn(f32, bool) + Send + Sync> = Box::new(on_change);
    let (tx, rx) = std::sync::mpsc::channel();
    // A dedicated MTA thread: the UI thread is an STA owned by winit.
    std::thread::Builder::new()
        .name("volume".into())
        .spawn(move || {
            let setup =
                || -> windows_core::Result<(IAudioEndpointVolume, IAudioEndpointVolumeCallback)> {
                    // SAFETY: plain COM calls on this thread's MTA apartment.
                    unsafe {
                        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                        let devices: IMMDeviceEnumerator =
                            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
                        let device = devices.GetDefaultAudioEndpoint(eRender, eConsole)?;
                        let volume: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None)?;
                        let cb: IAudioEndpointVolumeCallback = Callback { on_change }.into();
                        volume.RegisterControlChangeNotify(&cb)?;
                        Ok((volume, cb))
                    }
                };
            let r = setup();
            match r {
                Ok(keep) => {
                    let _ = tx.send(true);
                    // Keep the COM objects (and thus the registration) alive for the app's lifetime.
                    let _keep = keep;
                    loop {
                        std::thread::park();
                    }
                }
                Err(e) => {
                    log::warn!("volume watcher: {e}");
                    let _ = tx.send(false);
                }
            }
        })
        .ok()?;
    rx.recv().ok()?.then_some(Watcher)
}
