//! System stats. Samples at 1 Hz only while visible; sysinfo handles are created on first
//! show, so the module costs nothing until opened.

use super::{Ctx, Module};
use crate::config::{Config, StatsView};
use crate::{LabeledValue, NotchWindow, StatsState};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};
use sysinfo::{Components, Disks, Networks, ProcessesToUpdate, System};

const HISTORY: usize = 60;
const TICK: Duration = Duration::from_secs(1);
/// Disks, temperatures and battery change slowly: refresh every N ticks.
const SLOW_EVERY: u32 = 10;

#[derive(Default)]
struct Series(VecDeque<f32>);

impl Series {
    fn push(&mut self, v: f32) {
        if self.0.len() == HISTORY {
            self.0.pop_front();
        }
        self.0.push_back(v);
    }

    /// Samples scaled to 0..1 by `max`, oldest first.
    fn scaled(&self, max: f32) -> Vec<f32> {
        let max = max.max(f32::EPSILON);
        self.0.iter().map(|v| (v / max).clamp(0.0, 1.0)).collect()
    }
}

/// "11.4 GB" -> ("11.4", "GB"): the number is drawn large, the unit small.
fn split_unit(s: &str) -> (SharedString, SharedString) {
    match s.rsplit_once(' ') {
        Some((n, u)) => (n.into(), u.into()),
        None => (s.into(), SharedString::new()),
    }
}

/// Memory pressure from the used fraction (a portable stand-in for the OS's own signal).
fn pressure(frac: f32) -> &'static str {
    match frac {
        f if f < 0.85 => "Normal",
        f if f < 0.95 => "Elevated",
        _ => "High",
    }
}

/// GPU busy percent, where the OS exposes it without privileges.
#[cfg(target_os = "macos")]
fn gpu_usage() -> Option<f32> {
    // shortcut: spawns ioreg once a second while the stats view is open (~10 ms); move to IOKit
    // calls if it shows up in profiles.
    let out = std::process::Command::new("ioreg")
        .args(["-r", "-d", "1", "-w", "0", "-c", "IOAccelerator"])
        .output()
        .ok()?;
    parse_gpu(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(target_os = "linux")]
fn gpu_usage() -> Option<f32> {
    // AMD and some Intel drivers; NVIDIA needs nvidia-smi and is skipped.
    (0..4).find_map(|i| {
        std::fs::read_to_string(format!("/sys/class/drm/card{i}/device/gpu_busy_percent"))
            .ok()?
            .trim()
            .parse()
            .ok()
    })
}

#[cfg(target_os = "windows")]
fn gpu_usage() -> Option<f32> {
    None
}

/// `"Device Utilization %"=25` in ioreg's IOAccelerator PerformanceStatistics.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_gpu(ioreg: &str) -> Option<f32> {
    let key = "\"Device Utilization %\"=";
    let rest = &ioreg[ioreg.find(key)? + key.len()..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

pub fn format_bytes(b: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    if u == 0 || v >= 100.0 {
        format!("{v:.0} {}", UNITS[u])
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

struct Probes {
    sys: System,
    // Each probe is optional: any of them can fail on some systems.
    nets: Option<Networks>,
    /// Detailed view only, created on first use: enumerating sensors via IOKit briefly
    /// costs ~200 MB on macOS.
    slow: Option<SlowProbes>,
    last: Instant,
}

impl Probes {
    fn new() -> Option<Self> {
        let mut sys = System::new().map_err(|e| log::warn!("stats: {e}")).ok()?;
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        Some(Self {
            sys,
            nets: Networks::new_with_refreshed_list().ok(),
            slow: None,
            last: Instant::now(),
        })
    }
}

struct SlowProbes {
    disks: Option<Disks>,
    temps: Option<Components>,
    battery: Option<starship_battery::Manager>,
}

impl SlowProbes {
    fn new() -> Self {
        Self {
            disks: Disks::new_with_refreshed_list().ok(),
            temps: Components::new_with_refreshed_list().ok(),
            battery: starship_battery::Manager::new().ok(),
        }
    }

    fn refresh(&mut self) {
        if let Some(d) = &mut self.disks {
            d.refresh(true)
        }
        if let Some(t) = &mut self.temps {
            t.refresh(true)
        }
    }
}

#[derive(Default)]
struct Data {
    cpu: Series,
    mem: Series,
    down: Series,
    up: Series,
    gpu: Series,
}

pub struct Stats {
    me: Weak<Stats>,
    probes: RefCell<Option<Probes>>,
    data: RefCell<Data>,
    view: Cell<StatsView>,
    ticks: Cell<u32>,
    timer: slint::Timer,
    windows: RefCell<Vec<slint::Weak<NotchWindow>>>,
}

impl Stats {
    pub fn new(_ctx: Ctx) -> Rc<Self> {
        Rc::new_cyclic(|me| Self {
            me: me.clone(),
            probes: RefCell::default(),
            data: RefCell::default(),
            view: Cell::new(StatsView::Graphs),
            ticks: Cell::new(0),
            timer: slint::Timer::default(),
            windows: RefCell::default(),
        })
    }

    fn each_window(&self, f: impl Fn(&StatsState)) {
        self.windows.borrow_mut().retain(|w| match w.upgrade() {
            Some(ui) => {
                f(&ui.global::<StatsState>());
                true
            }
            None => false,
        });
    }

    fn sample(&self) {
        let mut probes = self.probes.borrow_mut();
        if probes.is_none() {
            *probes = Probes::new();
        }
        let Some(p) = probes.as_mut() else { return };
        let detailed = self.view.get() == StatsView::Detailed;
        let tick = self.ticks.get();
        self.ticks.set(tick.wrapping_add(1));

        p.sys.refresh_cpu_usage();
        p.sys.refresh_memory();
        let secs = p.last.elapsed().as_secs_f64().max(0.001);
        p.last = Instant::now();
        let (rx, tx) = p.nets.as_mut().map_or((0, 0), |nets| {
            nets.refresh(true);
            nets.iter().fold((0u64, 0u64), |(r, t), (_, n)| {
                (r + n.received(), t + n.transmitted())
            })
        });
        let (down, up) = (rx as f64 / secs, tx as f64 / secs);
        let cpu = p.sys.global_cpu_usage();
        let core_use: Vec<f32> = p.sys.cpus().iter().map(|c| c.usage() / 100.0).collect();
        let load = System::load_average().map_or(0.0, |l| l.one);
        let cpu_sub: SharedString = if load > 0.0 {
            format!("{} cores · load {load:.1}", core_use.len())
        } else {
            format!("{} cores", core_use.len())
        }
        .into();
        let gpu = gpu_usage();
        let (used, total) = (p.sys.used_memory(), p.sys.total_memory().max(1));

        let mut d = self.data.borrow_mut();
        d.cpu.push(cpu);
        d.mem.push(used as f32 / total as f32);
        d.down.push(down as f32);
        d.up.push(up as f32);
        if let Some(g) = gpu {
            d.gpu.push(g);
        }
        let gpu_peak = d.gpu.0.iter().copied().fold(0.0, f32::max);
        let gpu_hist = d.gpu.scaled(100.0);
        // One shared scale so up and down are comparable.
        let net_max = d.down.0.iter().chain(&d.up.0).copied().fold(1.0, f32::max);

        let cpu_text: SharedString = format!("{cpu:.0}%").into();
        let mem_text: SharedString = format!(
            "{} / {}",
            format_bytes(used as f64),
            format_bytes(total as f64)
        )
        .into();
        let down_text: SharedString = format!("{}/s", format_bytes(down)).into();
        let up_text: SharedString = format!("{}/s", format_bytes(up)).into();
        let hist = [
            d.cpu.scaled(100.0),
            d.mem.scaled(1.0),
            d.down.scaled(net_max),
            d.up.scaled(net_max),
        ];
        let mem_frac = used as f32 / total as f32;
        let mem_sub: SharedString =
            format!("of {} · {}", format_bytes(total as f64), pressure(mem_frac)).into();
        let mem_used: SharedString = format_bytes(used as f64).into();
        let (mem_num, mem_unit) = split_unit(&mem_used);
        let (down_num, down_unit) = split_unit(&down_text);
        let disk = tick.is_multiple_of(SLOW_EVERY).then(main_disk).flatten();

        let detail = detailed.then(|| {
            p.sys.refresh_processes(ProcessesToUpdate::All, true);
            match &mut p.slow {
                None => p.slow = Some(SlowProbes::new()),
                Some(slow) if tick.is_multiple_of(SLOW_EVERY) => slow.refresh(),
                Some(_) => {}
            }
            detail(p)
        });

        self.each_window(|g| {
            g.set_detailed(detailed);
            g.set_cpu_text(cpu_text.clone());
            g.set_mem_text(mem_text.clone());
            g.set_down_text(down_text.clone());
            g.set_up_text(up_text.clone());
            g.set_cpu_hist(model(hist[0].clone()));
            g.set_mem_hist(model(hist[1].clone()));
            g.set_down_hist(model(hist[2].clone()));
            g.set_up_hist(model(hist[3].clone()));
            g.set_mem_used(mem_used.clone());
            g.set_mem_sub(mem_sub.clone());
            g.set_mem_num(mem_num.clone());
            g.set_mem_unit(mem_unit.clone());
            g.set_down_num(down_num.clone());
            g.set_down_unit(down_unit.clone());
            g.set_cpu_num(format!("{cpu:.0}").into());
            g.set_cpu_sub(cpu_sub.clone());
            g.set_cpu_bars(model(core_use.clone()));
            g.set_has_gpu(gpu.is_some());
            if let Some(v) = gpu {
                g.set_gpu_num(format!("{v:.0}").into());
                g.set_gpu_sub(format!("Peak {gpu_peak:.0}%").into());
                g.set_gpu_hist(model(gpu_hist.clone()));
            }
            if let Some((free, total)) = disk {
                let (n, u) = split_unit(&format_bytes(free as f64));
                g.set_disk_num(n);
                g.set_disk_unit(u);
                g.set_disk_free(format_bytes(free as f64).into());
                g.set_disk_sub(format!("free of {}", format_bytes(total as f64)).into());
                g.set_disk_frac(1.0 - free as f32 / total.max(1) as f32);
            }
            if let Some(dt) = &detail {
                g.set_cores(model(dt.cores.clone()));
                g.set_disks(model(dt.disks.clone()));
                g.set_temps(model(dt.temps.clone()));
                g.set_procs(model(dt.procs.clone()));
                g.set_battery_text(dt.battery.as_str().into());
            }
        });
    }
}

struct Detail {
    cores: Vec<f32>,
    disks: Vec<LabeledValue>,
    temps: Vec<LabeledValue>,
    procs: Vec<LabeledValue>,
    battery: String,
}

fn row(label: impl Into<SharedString>, text: impl Into<SharedString>, frac: f32) -> LabeledValue {
    LabeledValue {
        label: label.into(),
        text: text.into(),
        frac,
    }
}

fn detail(p: &Probes) -> Detail {
    let cores = p.sys.cpus().iter().map(|c| c.usage() / 100.0).collect();

    let slow = p.slow.as_ref();
    let mut disks: Vec<_> = slow
        .and_then(|s| s.disks.as_ref())
        .into_iter()
        .flatten()
        .filter(|d| d.total_space() > 0)
        .map(|d| {
            let (total, free) = (d.total_space(), d.available_space());
            let used = total.saturating_sub(free);
            row(
                d.mount_point().display().to_string(),
                format!(
                    "{} free of {}",
                    format_bytes(free as f64),
                    format_bytes(total as f64)
                ),
                used as f32 / total as f32,
            )
        })
        .collect();
    disks.dedup_by(|a, b| a.text == b.text); // APFS volumes share one container
    disks.truncate(4);

    let mut temps: Vec<_> = slow
        .and_then(|s| s.temps.as_ref())
        .into_iter()
        .flatten()
        .filter_map(|c| c.temperature().map(|t| (c.label().to_string(), t)))
        .collect();
    temps.sort_by(|a, b| b.1.total_cmp(&a.1));
    let temps = temps
        .into_iter()
        .take(5)
        .map(|(l, t)| row(l, format!("{t:.0} °C"), -1.0))
        .collect();

    let mut procs: Vec<_> = p.sys.processes().values().collect();
    procs.sort_by(|a, b| b.cpu_usage().total_cmp(&a.cpu_usage()));
    let procs = procs
        .into_iter()
        .take(5)
        .map(|pr| {
            row(
                pr.name().to_string_lossy().to_string(),
                format!("{:.0}%", pr.cpu_usage()),
                -1.0,
            )
        })
        .collect();

    let battery = slow
        .and_then(|s| s.battery.as_ref())
        .and_then(|m| m.batteries().ok()?.next()?.ok())
        .map(|b| {
            let pct = b.state_of_charge().value * 100.0;
            let mins = |t: Option<starship_battery::units::Time>| {
                t.map(|t| t.get::<starship_battery::units::time::minute>() as u32)
            };
            match (b.state(), mins(b.time_to_empty()), mins(b.time_to_full())) {
                (starship_battery::State::Discharging, Some(m), _) => {
                    format!("{pct:.0}%, {}:{:02} left", m / 60, m % 60)
                }
                (starship_battery::State::Charging, _, Some(m)) => {
                    format!("{pct:.0}%, charging, full in {}:{:02}", m / 60, m % 60)
                }
                (s, _, _) => format!("{pct:.0}%, {s}"),
            }
        })
        .unwrap_or_default();

    Detail {
        cores,
        disks,
        temps,
        procs,
        battery,
    }
}

/// (free, total) bytes of the system volume.
fn main_disk() -> Option<(u64, u64)> {
    let disks = Disks::new_with_refreshed_list().ok()?;
    let d = disks
        .iter()
        .find(|d| d.mount_point() == std::path::Path::new("/"))
        .or(disks.first())?;
    Some((d.available_space(), d.total_space()))
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    Rc::new(VecModel::from(v)).into()
}

impl Module for Stats {
    fn id(&self) -> &'static str {
        "stats"
    }

    fn title(&self) -> &'static str {
        "Stats"
    }

    fn attach(&self, ui: &NotchWindow) {
        self.windows.borrow_mut().push(ui.as_weak());
    }

    fn on_show(&self) {
        self.ticks.set(0);
        self.sample();
        let me = self.me.clone();
        self.timer.start(slint::TimerMode::Repeated, TICK, move || {
            if let Some(s) = me.upgrade() {
                s.sample()
            }
        });
    }

    fn on_hide(&self) {
        self.timer.stop();
        // Drop the sysinfo handles (process list can be several MB); recreated on show.
        *self.probes.borrow_mut() = None;
    }

    fn config_changed(&self, cfg: &Config) {
        self.view.set(cfg.modules.stats.view);
        self.each_window(|g| g.set_detailed(cfg.modules.stats.view == StatsView::Detailed));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes() {
        assert_eq!(format_bytes(0.0), "0 B");
        assert_eq!(format_bytes(999.0), "999 B");
        assert_eq!(format_bytes(1500.0), "1.5 KB");
        assert_eq!(format_bytes(250_000_000.0), "250 MB");
        assert_eq!(format_bytes(16e9), "16.0 GB");
    }

    #[test]
    fn series_is_bounded_and_scaled() {
        let mut s = Series::default();
        for i in 0..100 {
            s.push(i as f32);
        }
        assert_eq!(s.0.len(), HISTORY);
        let v = s.scaled(99.0);
        assert_eq!(v.len(), HISTORY);
        assert_eq!(*v.last().unwrap(), 1.0);
        assert!(
            s.scaled(0.0).iter().all(|x| *x == 1.0),
            "zero max doesn't divide by zero"
        );
    }
}

#[cfg(test)]
mod card_tests {
    use super::*;

    #[test]
    fn units_pressure_gpu() {
        assert_eq!(split_unit("11.4 GB"), ("11.4".into(), "GB".into()));
        assert_eq!(split_unit("1.9 MB/s"), ("1.9".into(), "MB/s".into()));
        assert_eq!(pressure(0.5), "Normal");
        assert_eq!(pressure(0.9), "Elevated");
        assert_eq!(pressure(0.99), "High");
        let s = r#"| "PerformanceStatistics" = {"In use system memory"=1,"Device Utilization %"=25,"Renderer Utilization %"=20}"#;
        assert_eq!(parse_gpu(s), Some(25.0));
        assert_eq!(parse_gpu("nothing"), None);
    }
}
