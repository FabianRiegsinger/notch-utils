mod config;
mod notch;
mod platform;
mod state;

use config::Config;
use slint::winit_030::winit;
use std::cell::RefCell;
use std::rc::Rc;

slint::include_modules!();

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let cfg_path = Config::path();
    let cfg = Config::load(&cfg_path).unwrap_or_else(|e| {
        log::error!("invalid config {}: {e}; using defaults", cfg_path.display());
        Config::default()
    });
    let cfg = Rc::new(RefCell::new(cfg));

    #[allow(unused_mut)]
    let mut event_loop = winit::event_loop::EventLoop::with_user_event();
    #[cfg(target_os = "macos")]
    {
        // No Dock icon, no app menu: the notch and tray are the whole UI.
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        event_loop.with_activation_policy(ActivationPolicy::Accessory);
    }
    #[cfg(target_os = "linux")]
    {
        // Wayland forbids client-side window positioning; XWayland gives us top-center + always-on-top.
        use winit::platform::x11::EventLoopBuilderExtX11;
        event_loop.with_x11();
    }

    slint::BackendSelector::new()
        .backend_name("winit".into())
        .with_winit_event_loop_builder(event_loop)
        .with_winit_window_attributes_hook(|attrs| {
            #[cfg(target_os = "windows")]
            let attrs = {
                use winit::platform::windows::WindowAttributesExtWindows;
                attrs.with_skip_taskbar(true)
            };
            attrs
        })
        .select()?;

    let notch = notch::Notch::new(cfg.clone())?;

    let tray = Tray::new()?;
    let n = Rc::downgrade(&notch);
    tray.on_toggle(move || {
        if let Some(n) = n.upgrade() {
            n.handle(state::Event::Toggle)
        }
    });
    tray.on_open_settings(|| log::info!("settings window: not implemented yet"));
    tray.on_quit(|| {
        let _ = slint::quit_event_loop();
    });

    slint::run_event_loop_until_quit()?;
    drop(tray);
    Ok(())
}
