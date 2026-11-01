//! Devices: connected Bluetooth devices with battery levels (AirPods: left / right / case).
//! macOS: `system_profiler SPBluetoothDataType -json`; Linux: `upower --dump`.

use super::{Ctx, Hud, Module};
use crate::{DevEntry, DevState, NotchWindow};
use slint::{ComponentHandle, VecModel};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

const POLL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Device {
    pub name: String,
    /// "headphones", "keyboard", "mouse" or "devices".
    pub icon: &'static str,
    pub main: Option<u8>,
    pub left: Option<u8>,
    pub right: Option<u8>,
    pub case: Option<u8>,
    /// Bluetooth address, lowercase with '-' (to match IOKit battery entries).
    pub address: String,
}

impl Device {
    /// "L 80% · R 75% · Case 60%" or "80%"
    pub fn battery_text(&self) -> String {
        let parts: Vec<String> = [("L ", self.left), ("R ", self.right), ("Case ", self.case)]
            .iter()
            .filter_map(|(l, v)| v.map(|v| format!("{l}{v}%")))
            .collect();
        if !parts.is_empty() {
            parts.join(" · ")
        } else {
            self.main.map(|v| format!("{v}%")).unwrap_or_default()
        }
    }

    /// Lowest known level, for the low-battery color.
    pub fn lowest(&self) -> Option<u8> {
        [self.main, self.left, self.right]
            .into_iter()
            .flatten()
            .min()
    }
}

fn icon_for(kind: &str) -> &'static str {
    let k = kind.to_ascii_lowercase();
    if [
        "headphone",
        "headset",
        "earbud",
        "airpods",
        "speaker",
        "audio",
    ]
    .iter()
    .any(|w| k.contains(w))
    {
        "headphones"
    } else if k.contains("keyboard") {
        "keyboard"
    } else if k.contains("mouse") || k.contains("trackpad") || k.contains("pointing") {
        "mouse"
    } else {
        "devices"
    }
}

fn pct(v: Option<&serde_json::Value>) -> Option<u8> {
    v?.as_str()?
        .trim()
        .trim_end_matches('%')
        .trim()
        .parse()
        .ok()
}

/// Connected devices from `system_profiler SPBluetoothDataType -json`.
pub fn parse_system_profiler(json: &str) -> Vec<Device> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return vec![];
    };
    let Some(list) = v
        .pointer("/SPBluetoothDataType/0/device_connected")
        .and_then(|l| l.as_array())
    else {
        return vec![];
    };
    list.iter()
        .filter_map(|entry| {
            let (name, d) = entry.as_object()?.iter().next()?;
            let kind = d
                .get("device_minorType")
                .and_then(|k| k.as_str())
                .unwrap_or("");
            Some(Device {
                name: name.clone(),
                icon: icon_for(&format!("{kind} {name}")),
                main: pct(d.get("device_batteryLevelMain")),
                left: pct(d.get("device_batteryLevelLeft")),
                right: pct(d.get("device_batteryLevelRight")),
                case: pct(d.get("device_batteryLevelCase")),
                address: d
                    .get("device_address")
                    .and_then(|a| a.as_str())
                    .map(norm_addr)
                    .unwrap_or_default(),
            })
        })
        .collect()
}

fn norm_addr(a: &str) -> String {
    a.trim().to_ascii_lowercase().replace(':', "-")
}

/// (address, percent) of Apple peripherals from `ioreg -r -k BatteryPercent -l`
/// (Magic Trackpad/Keyboard/Mouse don't report battery in system_profiler).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn parse_ioreg(text: &str) -> Vec<(String, u8)> {
    let mut out = vec![];
    let (mut addr, mut pct) = (None::<String>, None::<u8>);
    for line in text.lines() {
        let l = line.trim_start_matches([' ', '|']).trim();
        if l.starts_with("+-o") {
            if let (Some(a), Some(p)) = (addr.take(), pct.take()) {
                out.push((a, p));
            }
            addr = None;
            pct = None;
        } else if let Some(v) = l.strip_prefix("\"DeviceAddress\" = ") {
            addr = Some(norm_addr(v.trim_matches('"')));
        } else if let Some(v) = l.strip_prefix("\"BatteryPercent\" = ") {
            pct = v.trim().parse().ok();
        }
    }
    if let (Some(a), Some(p)) = (addr, pct) {
        out.push((a, p));
    }
    out
}

/// Bluetooth peripherals (paths with `_dev_`) from `upower --dump`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_upower(text: &str) -> Vec<Device> {
    text.split("Device: ")
        .filter(|b| b.lines().next().is_some_and(|p| p.contains("_dev_")))
        .map(|b| {
            let field = |k: &str| {
                b.lines()
                    .find_map(|l| l.trim().strip_prefix(k).map(|v| v.trim().to_string()))
                    .unwrap_or_default()
            };
            let path = b.lines().next().unwrap_or_default();
            let model = field("model:");
            Device {
                name: if model.is_empty() {
                    "Bluetooth device".into()
                } else {
                    model.clone()
                },
                icon: icon_for(&format!("{path} {model}")),
                main: field("percentage:")
                    .trim_end_matches('%')
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .map(|p| p.round() as u8),
                ..Default::default()
            }
        })
        .collect()
}

/// Runs on a background thread. None = not supported here.
fn read() -> Option<Vec<Device>> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("system_profiler")
            .args(["SPBluetoothDataType", "-json"])
            .output()
            .ok()?;
        let mut devices = parse_system_profiler(&String::from_utf8_lossy(&out.stdout));
        if let Ok(io) = std::process::Command::new("ioreg")
            .args(["-r", "-k", "BatteryPercent", "-l"])
            .output()
        {
            for (addr, pct) in parse_ioreg(&String::from_utf8_lossy(&io.stdout)) {
                if let Some(d) = devices
                    .iter_mut()
                    .find(|d| d.address == addr && d.battery_text().is_empty())
                {
                    d.main = Some(pct);
                }
            }
        }
        Some(devices)
    }
    #[cfg(target_os = "linux")]
    {
        let out = std::process::Command::new("upower")
            .arg("--dump")
            .output()
            .ok()?;
        Some(parse_upower(&String::from_utf8_lossy(&out.stdout)))
    }
    // shortcut: Windows needs Windows.Devices.Enumeration + per-vendor GATT battery reads; not
    // done yet, so the tab says so.
    #[cfg(target_os = "windows")]
    {
        None
    }
}

thread_local! {
    static INSTANCE: RefCell<Weak<Devices>> = RefCell::default();
}

pub struct Devices {
    me: Weak<Devices>,
    ctx: Ctx,
    enabled: Cell<bool>,
    devices: RefCell<Option<Vec<Device>>>,
    busy: Cell<bool>,
    timer: slint::Timer,
    windows: RefCell<Vec<slint::Weak<NotchWindow>>>,
}

impl Devices {
    pub fn new(ctx: Ctx) -> Rc<Self> {
        let d = Rc::new_cyclic(|me| Self {
            me: me.clone(),
            ctx,
            enabled: Cell::new(false),
            devices: RefCell::default(),
            busy: Cell::new(false),
            timer: slint::Timer::default(),
            windows: RefCell::default(),
        });
        INSTANCE.with(|i| *i.borrow_mut() = Rc::downgrade(&d));
        d
    }

    fn refresh(&self) {
        if self.busy.replace(true) {
            return;
        }
        let _ = std::thread::Builder::new()
            .name("devices".into())
            .spawn(|| {
                let found = read();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(d) = INSTANCE.with(|i| i.borrow().upgrade()) else {
                        return;
                    };
                    d.busy.set(false);
                    d.update(found);
                });
            });
    }

    fn update(&self, found: Option<Vec<Device>>) {
        // HUD for newly connected devices (not on the first reading).
        if let (Some(before), Some(now)) = (self.devices.borrow().as_ref(), found.as_ref()) {
            for dev in now
                .iter()
                .filter(|d| !before.iter().any(|b| b.name == d.name))
            {
                let battery = dev.battery_text();
                (self.ctx.hud)(Hud {
                    icon: dev.icon,
                    text: if battery.is_empty() {
                        "Connected".into()
                    } else {
                        battery
                    },
                    ..Default::default()
                });
            }
        }
        *self.devices.borrow_mut() = found;
        self.sync();
    }

    fn sync(&self) {
        let devices = self.devices.borrow();
        let rows: Vec<DevEntry> = devices
            .iter()
            .flatten()
            .map(|d| DevEntry {
                name: d.name.as_str().into(),
                icon: d.icon.into(),
                battery: d.battery_text().into(),
                low: d.lowest().is_some_and(|l| l <= 20),
            })
            .collect();
        let supported = cfg!(not(target_os = "windows"));
        self.windows.borrow_mut().retain(|w| {
            let Some(ui) = w.upgrade() else { return false };
            let g = ui.global::<DevState>();
            g.set_entries(Rc::new(VecModel::from(rows.clone())).into());
            g.set_supported(supported);
            true
        });
    }
}

impl Module for Devices {
    fn id(&self) -> &'static str {
        "devices"
    }

    fn title(&self) -> &'static str {
        "Devices"
    }

    fn attach(&self, ui: &NotchWindow) {
        self.windows.borrow_mut().push(ui.as_weak());
        self.sync();
    }

    fn on_show(&self) {
        self.refresh();
    }

    fn config_changed(&self, cfg: &crate::config::Config) {
        let on = cfg.layout.modules.iter().any(|m| m == "devices");
        if self.enabled.replace(on) == on {
            return;
        }
        if on {
            self.refresh();
            let me = self.me.clone();
            self.timer.start(slint::TimerMode::Repeated, POLL, move || {
                if let Some(d) = me.upgrade() {
                    d.refresh()
                }
            });
        } else {
            self.timer.stop();
            *self.devices.borrow_mut() = None;
            self.sync();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn airpods_from_system_profiler() {
        let json = r#"{"SPBluetoothDataType":[{"device_connected":[
            {"Fabian's AirPods Pro":{"device_minorType":"Headphones","device_batteryLevelLeft":"80%","device_batteryLevelRight":"75%","device_batteryLevelCase":"60%"}},
            {"Magic Keyboard":{"device_minorType":"Keyboard","device_batteryLevelMain":"15%"}}
        ],"device_not_connected":[{"Old Speaker":{}}]}]}"#;
        let d = parse_system_profiler(json);
        assert_eq!(d.len(), 2, "only connected devices");
        assert_eq!(
            (d[0].icon, d[0].battery_text().as_str()),
            ("headphones", "L 80% · R 75% · Case 60%")
        );
        assert_eq!(
            (d[1].icon, d[1].battery_text().as_str(), d[1].lowest()),
            ("keyboard", "15%", Some(15))
        );
        assert!(parse_system_profiler("{}").is_empty());
    }

    #[test]
    fn ioreg_battery() {
        let text = r#"+-o AppleDeviceManagementHIDEventService  <class AppleDeviceManagementHIDEventService>
    {
      "Product" = "Magic Trackpad"
      "DeviceAddress" = "AC-49-DB-01-02-03"
      "BatteryPercent" = 63
    }
+-o Other  <class X>
    {
      "BatteryPercent" = 50
    }"#;
        assert_eq!(parse_ioreg(text), [("ac-49-db-01-02-03".to_string(), 63)]);
        assert_eq!(norm_addr("AC:49:DB:01:02:03"), "ac-49-db-01-02-03");
    }

    #[test]
    fn upower_bluetooth_devices() {
        let text = "Device: /org/freedesktop/UPower/devices/battery_BAT0\n  native-path: BAT0\n  percentage: 90%\n\n\
Device: /org/freedesktop/UPower/devices/headset_dev_AA_BB\n  model:   WH-1000XM4\n    percentage:          70%\n\n\
Device: /org/freedesktop/UPower/devices/mouse_dev_CC\n  model: MX Master 3\n  percentage: 12%\n";
        let d = parse_upower(text);
        assert_eq!(d.len(), 2, "laptop battery excluded");
        assert_eq!(
            (d[0].name.as_str(), d[0].icon, d[0].main),
            ("WH-1000XM4", "headphones", Some(70))
        );
        assert_eq!((d[1].icon, d[1].main), ("mouse", Some(12)));
    }
}

#[cfg(all(test, target_os = "macos"))]
mod live {
    /// Reads this Mac's devices (no prompts): `cargo test -- --ignored devices_live`.
    #[test]
    #[ignore]
    fn devices_live() {
        let d = super::read().expect("supported on macOS");
        for x in &d {
            println!("icon={} battery={:?}", x.icon, x.battery_text());
        }
    }
}
