//! Calendar from ICS subscription URLs (Google "secret iCal address", iCloud, Outlook, CalDAV
//! exports). Fetched on a background thread every `refresh_minutes`; recurrences expanded with
//! `rrule`. Shows today's remaining events and tomorrow's. URLs are never logged in full.

use super::{Ctx, Module};
use crate::config::Config;
use crate::{CalEvent, CalState, NotchWindow};
use chrono::{DateTime, Duration as Span, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use slint::{ComponentHandle, VecModel};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::time::Duration;

const MAX_ICS_BYTES: u64 = 10 * 1024 * 1024;
const MAX_INSTANCES: u16 = 500;
/// Peek when an event starts within this many minutes.
const PEEK_MINUTES: i64 = 15;

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub start: DateTime<Local>,
    pub end: DateTime<Local>,
    pub all_day: bool,
    pub title: String,
    pub location: String,
}

/// One property line: name, params (upper-cased keys), value.
#[derive(Debug, Clone)]
struct Prop {
    name: String,
    params: HashMap<String, String>,
    value: String,
}

/// RFC 5545 line unfolding: a line starting with space/tab continues the previous one.
fn unfold(text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(cont) = line.strip_prefix([' ', '\t'])
            && let Some(last) = out.last_mut()
        {
            last.push_str(cont);
            continue;
        }
        out.push(line.to_string());
    }
    out
}

fn parse_prop(line: &str) -> Option<Prop> {
    // The value starts at the first ':' outside a quoted parameter value.
    let mut quoted = false;
    let split = line.char_indices().find(|&(_, c)| {
        if c == '"' {
            quoted = !quoted;
        }
        c == ':' && !quoted
    })?;
    let (head, value) = (&line[..split.0], &line[split.0 + 1..]);
    let mut parts = head.split(';');
    let name = parts.next()?.to_ascii_uppercase();
    let params = parts
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.to_ascii_uppercase(), v.trim_matches('"').to_string()))
        .collect();
    Some(Prop {
        name,
        params,
        value: value.to_string(),
    })
}

fn unescape(s: &str) -> String {
    s.replace("\\n", " ")
        .replace("\\N", " ")
        .replace("\\,", ",")
        .replace("\\;", ";")
        .replace("\\\\", "\\")
}

/// DATE-TIME / DATE value → local time. Unknown TZIDs (e.g. Windows names) fall back to local.
fn parse_time(p: &Prop) -> Option<(DateTime<Local>, bool)> {
    let v = p.value.trim();
    if p.params.get("VALUE").is_some_and(|x| x == "DATE") || v.len() == 8 {
        let d = NaiveDate::parse_from_str(v, "%Y%m%d").ok()?;
        return Some((
            Local
                .from_local_datetime(&d.and_hms_opt(0, 0, 0)?)
                .earliest()?,
            true,
        ));
    }
    if let Some(utc) = v.strip_suffix('Z') {
        let n = NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S").ok()?;
        return Some((Utc.from_utc_datetime(&n).with_timezone(&Local), false));
    }
    let n = NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S").ok()?;
    let local = match p
        .params
        .get("TZID")
        .and_then(|t| t.parse::<chrono_tz::Tz>().ok())
    {
        Some(tz) => tz.from_local_datetime(&n).earliest()?.with_timezone(&Local),
        None => Local.from_local_datetime(&n).earliest()?,
    };
    Some((local, false))
}

/// "PT1H30M", "P1D", "-PT15M" → span.
fn parse_duration(s: &str) -> Option<Span> {
    let (neg, s) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let s = s.strip_prefix('P')?;
    let (mut total, mut num, mut in_time) = (0i64, String::new(), false);
    for c in s.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' => num.push(c),
            _ => {
                let n: i64 = num.parse().ok()?;
                num.clear();
                total += n * match (c, in_time) {
                    ('W', _) => 604_800,
                    ('D', _) => 86_400,
                    ('H', true) => 3600,
                    ('M', true) => 60,
                    ('S', true) => 1,
                    _ => return None,
                };
            }
        }
    }
    Some(Span::seconds(if neg { -total } else { total }))
}

/// The DTSTART/RRULE/EXDATE block `rrule` parses. TZIDs it can't resolve go through UTC.
fn rrule_block(dtstart: &Prop, rules: &[&Prop]) -> String {
    let tz_ok = dtstart
        .params
        .get("TZID")
        .is_none_or(|t| t.parse::<chrono_tz::Tz>().is_ok());
    let line = |p: &Prop| -> String {
        if tz_ok || p.value.ends_with('Z') {
            let params: String = p.params.iter().map(|(k, v)| format!(";{k}={v}")).collect();
            format!("{}{params}:{}", p.name, p.value)
        } else {
            match parse_time(p) {
                Some((t, _)) => format!(
                    "{}:{}",
                    p.name,
                    t.with_timezone(&Utc).format("%Y%m%dT%H%M%SZ")
                ),
                None => String::new(),
            }
        }
    };
    let mut s = line(dtstart);
    for r in rules {
        s.push('\n');
        s.push_str(&line(r));
    }
    s
}

/// All events overlapping `[from, to)`, sorted by start.
pub fn parse_ics(text: &str, from: DateTime<Local>, to: DateTime<Local>) -> Vec<Event> {
    let lines = unfold(text);
    let mut events: Vec<Vec<Prop>> = vec![];
    let mut current: Option<Vec<Prop>> = None;
    let mut depth = 0; // nested components (VALARM) inside VEVENT
    for l in &lines {
        let Some(p) = parse_prop(l) else { continue };
        match (p.name.as_str(), p.value.to_ascii_uppercase().as_str()) {
            ("BEGIN", "VEVENT") => current = Some(vec![]),
            ("END", "VEVENT") => events.extend(current.take()),
            ("BEGIN", _) if current.is_some() => depth += 1,
            ("END", _) if current.is_some() && depth > 0 => depth -= 1,
            _ if depth == 0 => {
                if let Some(c) = current.as_mut() {
                    c.push(p)
                }
            }
            _ => {}
        }
    }

    // Moved/edited instances of recurring events: (UID, original start) → skip in expansion.
    let mut overridden: HashSet<(String, i64)> = HashSet::new();
    for e in &events {
        let get = |n: &str| e.iter().find(|p| p.name == n);
        if let (Some(uid), Some(rid)) = (get("UID"), get("RECURRENCE-ID"))
            && let Some((t, _)) = parse_time(rid)
        {
            overridden.insert((uid.value.clone(), t.timestamp()));
        }
    }

    let mut out = vec![];
    for e in &events {
        let get = |n: &str| e.iter().find(|p| p.name == n);
        if get("STATUS").is_some_and(|s| s.value.eq_ignore_ascii_case("CANCELLED")) {
            continue;
        }
        let Some(dtstart) = get("DTSTART") else {
            continue;
        };
        let Some((start, all_day)) = parse_time(dtstart) else {
            continue;
        };
        let len = get("DTEND")
            .and_then(parse_time)
            .map(|(end, _)| end - start)
            .or_else(|| get("DURATION").and_then(|d| parse_duration(&d.value)))
            .unwrap_or(if all_day { Span::days(1) } else { Span::zero() });
        let title = get("SUMMARY")
            .map(|s| unescape(&s.value))
            .unwrap_or_default();
        let location = get("LOCATION")
            .map(|s| unescape(&s.value))
            .unwrap_or_default();
        let uid = get("UID").map(|u| u.value.clone()).unwrap_or_default();
        let make = |s: DateTime<Local>| Event {
            start: s,
            end: s + len,
            all_day,
            title: title.clone(),
            location: location.clone(),
        };

        let rules: Vec<&Prop> = e
            .iter()
            .filter(|p| matches!(p.name.as_str(), "RRULE" | "RDATE" | "EXDATE"))
            .collect();
        if rules.iter().any(|p| p.name == "RRULE") && get("RECURRENCE-ID").is_none() {
            let set = match rrule_block(dtstart, &rules).parse::<rrule::RRuleSet>() {
                Ok(s) => s,
                Err(err) => {
                    log::debug!("calendar: skipping recurrence {title:?}: {err}");
                    continue;
                }
            };
            // Instances that started before `from` can still be running.
            let lo = (from - len - Span::seconds(1)).with_timezone(&rrule::Tz::Local(Local));
            let hi = to.with_timezone(&rrule::Tz::Local(Local));
            for t in set.after(lo).before(hi).all(MAX_INSTANCES).dates {
                let s = t.with_timezone(&Local);
                if !overridden.contains(&(uid.clone(), s.timestamp())) {
                    out.push(make(s));
                }
            }
        } else {
            out.push(make(start));
        }
    }
    // Overlap test; a zero-length event counts as one second.
    out.retain(|e| e.start < to && e.end.max(e.start + Span::seconds(1)) > from);
    out.sort_by_key(|e| e.start);
    out.dedup();
    out
}

/// webcal:// is just https.
pub fn http_url(u: &str) -> String {
    match u.strip_prefix("webcal://") {
        Some(rest) => format!("https://{rest}"),
        None => u.to_string(),
    }
}

/// Host only: the path usually contains the secret.
pub fn redact(u: &str) -> String {
    let rest = u.split_once("://").map_or(u, |(_, r)| r);
    format!("{}/…", rest.split('/').next().unwrap_or(""))
}

fn fetch(url: &str) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into();
    agent
        .get(&http_url(url))
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .with_config()
        .limit(MAX_ICS_BYTES)
        .read_to_string()
        .map_err(|e| e.to_string())
}

thread_local! {
    static INSTANCE: RefCell<Weak<CalendarModule>> = RefCell::default();
}

fn day_start(d: NaiveDate) -> DateTime<Local> {
    Local
        .from_local_datetime(&d.and_hms_opt(0, 0, 0).expect("midnight"))
        .earliest()
        .unwrap_or_else(Local::now)
}

pub struct CalendarModule {
    me: Weak<CalendarModule>,
    ctx: Ctx,
    urls: RefCell<Vec<String>>,
    events: RefCell<Vec<Event>>,
    errors: RefCell<Vec<String>>,
    loading: std::cell::Cell<bool>,
    refresh: slint::Timer,
    tick: slint::Timer,
    windows: RefCell<Vec<slint::Weak<NotchWindow>>>,
}

impl CalendarModule {
    pub fn new(ctx: Ctx) -> Rc<Self> {
        let m = Rc::new_cyclic(|me| Self {
            me: me.clone(),
            ctx,
            urls: RefCell::default(),
            events: RefCell::default(),
            errors: RefCell::default(),
            loading: std::cell::Cell::new(false),
            refresh: slint::Timer::default(),
            tick: slint::Timer::default(),
            windows: RefCell::default(),
        });
        INSTANCE.with(|i| *i.borrow_mut() = Rc::downgrade(&m));
        m
    }

    fn reload(&self) {
        let urls = self.urls.borrow().clone();
        if urls.is_empty() || self.loading.replace(true) {
            return;
        }
        self.sync();
        let _ = std::thread::Builder::new()
            .name("calendar".into())
            .spawn(move || {
                let today = Local::now().date_naive();
                let (from, to) = (day_start(today), day_start(today + Span::days(2)));
                let mut events = vec![];
                let mut errors = vec![];
                for u in &urls {
                    match fetch(u) {
                        Ok(text) => events.extend(parse_ics(&text, from, to)),
                        Err(e) => {
                            log::warn!("calendar {}: {e}", redact(u));
                            errors.push(format!("{}: {e}", redact(u)));
                        }
                    }
                }
                events.sort_by_key(|e| e.start);
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(m) = INSTANCE.with(|i| i.borrow().upgrade()) else {
                        return;
                    };
                    m.loading.set(false);
                    *m.events.borrow_mut() = events;
                    *m.errors.borrow_mut() = errors;
                    m.sync();
                });
            });
    }

    fn rows(&self, day: NaiveDate, now: DateTime<Local>) -> Vec<CalEvent> {
        let (from, to) = (day_start(day), day_start(day + Span::days(1)));
        self.events
            .borrow()
            .iter()
            .filter(|e| e.start < to && e.end > from && e.end > now)
            .map(|e| CalEvent {
                time: if e.all_day {
                    "All day".into()
                } else {
                    format!("{} – {}", e.start.format("%H:%M"), e.end.format("%H:%M")).into()
                },
                title: e.title.as_str().into(),
                location: e.location.as_str().into(),
                now: e.start <= now && e.end > now,
            })
            .collect()
    }

    fn sync(&self) {
        let now = Local::now();
        let today = self.rows(now.date_naive(), now);
        let tomorrow = self.rows(now.date_naive() + Span::days(1), now);
        let configured = !self.urls.borrow().is_empty();
        let errors = self.errors.borrow().join("\n");
        let loading = self.loading.get();
        self.windows.borrow_mut().retain(|w| {
            let Some(ui) = w.upgrade() else { return false };
            let g = ui.global::<CalState>();
            g.set_today(Rc::new(VecModel::from(today.clone())).into());
            g.set_tomorrow(Rc::new(VecModel::from(tomorrow.clone())).into());
            g.set_configured(configured);
            g.set_loading(loading);
            g.set_error(errors.as_str().into());
            true
        });
        (self.ctx.peek_changed)();
    }
}

impl Module for CalendarModule {
    fn id(&self) -> &'static str {
        "calendar"
    }

    fn title(&self) -> &'static str {
        "Calendar"
    }

    fn attach(&self, ui: &NotchWindow) {
        let me = self.me.clone();
        ui.global::<CalState>().on_refresh(move || {
            if let Some(m) = me.upgrade() {
                m.reload()
            }
        });
        self.windows.borrow_mut().push(ui.as_weak());
        self.sync();
    }

    fn on_show(&self) {
        self.sync();
        let me = self.me.clone();
        self.tick.start(
            slint::TimerMode::Repeated,
            Duration::from_secs(60),
            move || {
                if let Some(m) = me.upgrade() {
                    m.sync()
                }
            },
        );
    }

    fn on_hide(&self) {
        self.tick.stop();
    }

    fn config_changed(&self, cfg: &Config) {
        let enabled = cfg.layout.modules.iter().any(|m| m == "calendar");
        let urls = if enabled {
            cfg.modules.calendar.urls.clone()
        } else {
            vec![]
        };
        let changed = *self.urls.borrow() != urls;
        *self.urls.borrow_mut() = urls;
        if changed {
            self.events.borrow_mut().clear();
            self.errors.borrow_mut().clear();
            self.reload();
        }
        if self.urls.borrow().is_empty() {
            self.refresh.stop();
        } else {
            let me = self.me.clone();
            let every = Duration::from_secs(cfg.modules.calendar.refresh_minutes.max(1) * 60);
            self.refresh
                .start(slint::TimerMode::Repeated, every, move || {
                    if let Some(m) = me.upgrade() {
                        m.reload()
                    }
                });
        }
        self.sync();
    }

    fn activity(&self) -> Option<super::Hud> {
        let now = Local::now();
        let next = self
            .events
            .borrow()
            .iter()
            .find(|e| !e.all_day && e.start > now)
            .cloned()?;
        let mins = (next.start - now).num_minutes();
        (mins <= PEEK_MINUTES).then(|| super::Hud {
            icon: "calendar",
            level: None,
            text: format!("{} min", mins.max(1)),
            warn: false,
        })
    }

    fn peek(&self) -> Option<String> {
        let now = Local::now();
        let next = self
            .events
            .borrow()
            .iter()
            .find(|e| !e.all_day && e.start > now)
            .cloned()?;
        let mins = (next.start - now).num_minutes();
        (mins <= PEEK_MINUTES).then(|| {
            format!(
                "{} {} · in {} min",
                next.start.format("%H:%M"),
                next.title,
                mins.max(1)
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Local> {
        let n = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap();
        Local.from_local_datetime(&n).earliest().unwrap()
    }

    const ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:a\r\nDTSTART:20261102T090000\r\nDTEND:20261102T093000\r\nSUMMARY:Stand\r\n up\r\nLOCATION:Room 1\\, 2nd floor\r\n\
BEGIN:VALARM\r\nTRIGGER:-PT15M\r\nDESCRIPTION:ignore me\r\nEND:VALARM\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:weekly\r\nDTSTART:20261005T140000\r\nDURATION:PT1H\r\nRRULE:FREQ=WEEKLY;BYDAY=MO\r\nEXDATE:20261026T140000\r\nSUMMARY:Weekly\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:weekly\r\nRECURRENCE-ID:20261102T140000\r\nDTSTART:20261102T160000\r\nDTEND:20261102T170000\r\nSUMMARY:Weekly (moved)\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:c\r\nDTSTART:20261102T120000\r\nSUMMARY:Cancelled\r\nSTATUS:CANCELLED\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:d\r\nDTSTART;VALUE=DATE:20261102\r\nDTEND;VALUE=DATE:20261103\r\nSUMMARY:Holiday\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n";

    #[test]
    fn parses_unfolds_and_skips_cancelled_and_alarms() {
        let ev = parse_ics(ICS, at("2026-11-02 00:00"), at("2026-11-03 00:00"));
        let titles: Vec<&str> = ev.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, ["Holiday", "Standup", "Weekly (moved)"], "{ev:#?}");
        assert_eq!(ev[1].location, "Room 1, 2nd floor");
        assert!(ev[0].all_day);
        assert_eq!(ev[1].end - ev[1].start, Span::minutes(30));
    }

    #[test]
    fn expands_weekly_with_exdate() {
        let ev = parse_ics(ICS, at("2026-10-19 00:00"), at("2026-10-28 00:00"));
        let weekly: Vec<_> = ev
            .iter()
            .filter(|e| e.title == "Weekly")
            .map(|e| e.start)
            .collect();
        assert_eq!(weekly, [at("2026-10-19 14:00")], "Oct 26 is excluded");
        let next = parse_ics(ICS, at("2026-11-09 00:00"), at("2026-11-10 00:00"));
        assert_eq!(
            next[0].end - next[0].start,
            Span::hours(1),
            "DURATION applied"
        );
    }

    #[test]
    fn utc_and_tzid_times() {
        let p = parse_prop("DTSTART:20261102T080000Z").unwrap();
        let (t, _) = parse_time(&p).unwrap();
        assert_eq!(t.with_timezone(&Utc).format("%H:%M").to_string(), "08:00");
        let p = parse_prop("DTSTART;TZID=America/New_York:20260702T090000").unwrap();
        let (t, _) = parse_time(&p).unwrap();
        assert_eq!(
            t.with_timezone(&Utc).format("%H:%M").to_string(),
            "13:00",
            "EDT is UTC-4"
        );
        let p = parse_prop("DTSTART;TZID=W. Europe Standard Time:20260702T090000").unwrap();
        assert!(parse_time(&p).is_some(), "unknown TZID falls back to local");
    }

    #[test]
    fn durations_and_helpers() {
        assert_eq!(parse_duration("PT1H30M"), Some(Span::minutes(90)));
        assert_eq!(parse_duration("P1D"), Some(Span::days(1)));
        assert_eq!(parse_duration("-PT15M"), Some(Span::minutes(-15)));
        assert_eq!(parse_duration("bogus"), None);
        assert_eq!(
            http_url("webcal://p01.icloud.com/x"),
            "https://p01.icloud.com/x"
        );
        assert_eq!(
            redact("https://calendar.google.com/calendar/ical/secret/basic.ics"),
            "calendar.google.com/…"
        );
    }
}
