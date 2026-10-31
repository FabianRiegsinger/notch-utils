mod app;
mod config;
mod hotkey;
mod ipc;
mod modules;
mod notch;
mod platform;
mod settings;
mod state;
mod watch;

use app::App;
use config::Config;
use ipc::Command;
use slint::winit_030::winit;

slint::include_modules!();

const USAGE: &str = "usage: notch-utils [--toggle | --settings]";

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let cmd = match std::env::args().nth(1) {
        None => Command::Ping,
        Some(a) if a == "-h" || a == "--help" => {
            println!("{USAGE}");
            return Ok(());
        }
        Some(a) => {
            Command::parse(&a).ok_or_else(|| anyhow::anyhow!("unknown argument {a:?}\n{USAGE}"))?
        }
    };
    // Already running? Hand over the command and exit.
    if ipc::send(cmd)? {
        return Ok(());
    }

    let cfg_path = Config::path();
    let (cfg, error) = match Config::load(&cfg_path) {
        Ok(c) => (c, String::new()),
        Err(e) => {
            log::error!("invalid config {}: {e}; using defaults", cfg_path.display());
            (
                Config::default(),
                format!("config.toml: {e}\nUsing defaults."),
            )
        }
    };

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
        .renderer_name(cfg.appearance.renderer.slint_name().into())
        .with_winit_event_loop_builder(event_loop)
        .with_winit_window_attributes_hook(|attrs| {
            #[cfg(target_os = "windows")]
            let attrs = {
                use winit::platform::windows::WindowAttributesExtWindows;
                // Only the notch: the settings window should appear in the taskbar.
                let notch = attrs.title == "notch-utils";
                attrs.with_skip_taskbar(notch)
            };
            attrs
        })
        .select()?;

    let app = App::new(cfg, cfg_path.clone(), error)?;
    if cmd != Command::Ping {
        app.command(cmd);
    }

    if let Err(e) = ipc::serve(App::post) {
        log::warn!("ipc unavailable, --toggle won't reach this instance: {e}");
    }
    let _watcher = watch::watch(&cfg_path, App::post_reload)
        .map_err(|e| log::warn!("config live reload unavailable: {e}"))
        .ok();

    let tray = Tray::new()?;
    tray.on_toggle(|| App::post(Command::Toggle));
    tray.on_open_settings(|| App::post(Command::Settings));
    tray.on_quit(|| {
        let _ = slint::quit_event_loop();
    });

    slint::run_event_loop_until_quit()?;
    drop(tray);
    drop(app);
    Ok(())
}
