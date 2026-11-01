//! Notes: a list of plain-text notes, saved as you type to notes.json in the data dir.
//! The first line is the title; pinned notes sort first, then the most recently edited.

use super::{Ctx, Module};
use crate::{NotchWindow, NoteRow, NotesState};
use chrono::{DateTime, Local, TimeZone};
use serde::{Deserialize, Serialize};
use slint::{ComponentHandle, VecModel};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::Duration;

const SAVE_AFTER: Duration = Duration::from_millis(600);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Note {
    pub id: u64,
    pub body: String,
    pub pinned: bool,
    /// Unix seconds of the last edit.
    pub updated: i64,
}

fn store_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "notch-utils").map(|d| d.data_dir().join("notes.json"))
}

/// First non-empty line (title) and the next one (preview).
fn title_preview(body: &str) -> (String, String) {
    let mut lines = body.lines().map(str::trim).filter(|l| !l.is_empty());
    let title = lines.next().unwrap_or("New note").to_string();
    let preview = lines.next().unwrap_or("No additional text").to_string();
    (title, preview)
}

/// "Today", "Yesterday", "Monday" (this week), else "28.10.26".
fn when(updated: i64, now: DateTime<Local>) -> String {
    let Some(t) = Local.timestamp_opt(updated, 0).single() else {
        return String::new();
    };
    let days = (now.date_naive() - t.date_naive()).num_days();
    match days {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=6 => t.format("%A").to_string(),
        _ => t.format("%d.%m.%y").to_string(),
    }
}

/// Pinned first, then newest edit first.
fn sort(notes: &mut [Note]) {
    notes.sort_by_key(|n| (!n.pinned, std::cmp::Reverse(n.updated)));
}

pub struct Notes {
    me: Weak<Notes>,
    notes: RefCell<Vec<Note>>,
    selected: Cell<Option<u64>>,
    save_timer: slint::Timer,
    windows: RefCell<Vec<slint::Weak<NotchWindow>>>,
}

impl Notes {
    pub fn new(_ctx: Ctx) -> Rc<Self> {
        let mut notes: Vec<Note> = store_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| {
                serde_json::from_str(&s)
                    .map_err(|e| log::warn!("notes: unreadable notes.json: {e}"))
                    .ok()
            })
            .unwrap_or_default();
        sort(&mut notes);
        let selected = notes.first().map(|n| n.id);
        Rc::new_cyclic(|me| Self {
            me: me.clone(),
            notes: RefCell::new(notes),
            selected: Cell::new(selected),
            save_timer: slint::Timer::default(),
            windows: RefCell::default(),
        })
    }

    fn save(&self) {
        let Some(path) = store_path() else { return };
        let json = match serde_json::to_string_pretty(&*self.notes.borrow()) {
            Ok(j) => j,
            Err(e) => return log::warn!("notes: {e}"),
        };
        // Write a temp file and rename, so a crash mid-write never loses the notes.
        let tmp = path.with_extension("json.tmp");
        let r = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|_| std::fs::write(&tmp, json))
            .and_then(|_| std::fs::rename(&tmp, &path));
        if let Err(e) = r {
            log::warn!("notes: save failed: {e}");
        }
    }

    fn save_soon(&self) {
        let me = self.me.clone();
        self.save_timer
            .start(slint::TimerMode::SingleShot, SAVE_AFTER, move || {
                if let Some(n) = me.upgrade() {
                    n.save()
                }
            });
    }

    /// Drop empty notes other than the selected one.
    fn prune(&self) {
        let sel = self.selected.get();
        let before = self.notes.borrow().len();
        self.notes
            .borrow_mut()
            .retain(|n| Some(n.id) == sel || !n.body.trim().is_empty());
        if self.notes.borrow().len() != before {
            self.save_soon();
        }
    }

    fn rows(&self) -> Vec<NoteRow> {
        let now = Local::now();
        let sel = self.selected.get();
        self.notes
            .borrow()
            .iter()
            .map(|n| {
                let (title, preview) = title_preview(&n.body);
                NoteRow {
                    id: n.id as i32,
                    title: title.into(),
                    sub: format!("{} · {preview}", when(n.updated, now)).into(),
                    pinned: n.pinned,
                    selected: Some(n.id) == sel,
                }
            })
            .collect()
    }

    /// Push the list; `body` also replaces the editor text (on select/new/delete, not typing).
    fn sync(&self, body: bool) {
        let rows = self.rows();
        let count = rows.len() as i32;
        let sel = self.selected.get();
        let current = sel.and_then(|id| self.notes.borrow().iter().find(|n| n.id == id).cloned());
        self.windows.borrow_mut().retain(|w| {
            let Some(ui) = w.upgrade() else { return false };
            let g = ui.global::<NotesState>();
            g.set_notes(Rc::new(VecModel::from(rows.clone())).into());
            g.set_count(count);
            g.set_has_selection(current.is_some());
            g.set_pinned(current.as_ref().is_some_and(|n| n.pinned));
            if body {
                g.set_body(current.as_ref().map_or("", |n| n.body.as_str()).into());
            }
            true
        });
    }

    fn select(&self, id: u64) {
        self.selected.set(Some(id));
        self.prune();
        self.sync(true);
    }

    fn create(&self) {
        // Reuse an empty note instead of stacking blank ones.
        let id = match self
            .notes
            .borrow()
            .iter()
            .find(|n| n.body.trim().is_empty())
        {
            Some(n) => n.id,
            None => 0,
        };
        let id = if id != 0 {
            id
        } else {
            let id = self.notes.borrow().iter().map(|n| n.id).max().unwrap_or(0) + 1;
            self.notes.borrow_mut().insert(
                0,
                Note {
                    id,
                    body: String::new(),
                    pinned: false,
                    updated: Local::now().timestamp(),
                },
            );
            id
        };
        self.select(id);
    }

    fn edited(&self, text: &str) {
        // The editor only shows with a note selected.
        let Some(id) = self.selected.get() else {
            return;
        };
        if let Some(n) = self.notes.borrow_mut().iter_mut().find(|n| n.id == id) {
            if n.body == text {
                return;
            }
            n.body = text.to_string();
            n.updated = Local::now().timestamp();
        }
        sort(&mut self.notes.borrow_mut());
        self.save_soon();
        self.sync(false);
    }

    fn toggle_pin(&self) {
        let Some(id) = self.selected.get() else {
            return;
        };
        if let Some(n) = self.notes.borrow_mut().iter_mut().find(|n| n.id == id) {
            n.pinned = !n.pinned;
        }
        sort(&mut self.notes.borrow_mut());
        self.save_soon();
        self.sync(false);
    }

    fn delete(&self) {
        let Some(id) = self.selected.get() else {
            return;
        };
        let mut notes = self.notes.borrow_mut();
        let i = notes.iter().position(|n| n.id == id);
        notes.retain(|n| n.id != id);
        let next = i.and_then(|i| {
            notes
                .get(i.min(notes.len().saturating_sub(1)))
                .map(|n| n.id)
        });
        drop(notes);
        self.selected.set(next);
        self.save_soon();
        self.sync(true);
    }
}

impl Module for Notes {
    fn id(&self) -> &'static str {
        "notes"
    }

    fn title(&self) -> &'static str {
        "Notes"
    }

    fn attach(&self, ui: &NotchWindow) {
        let g = ui.global::<NotesState>();
        let me = self.me.clone();
        g.on_select(move |id| {
            if let Some(n) = me.upgrade() {
                n.select(id as u64)
            }
        });
        let me = self.me.clone();
        g.on_create(move || {
            if let Some(n) = me.upgrade() {
                n.create()
            }
        });
        let me = self.me.clone();
        g.on_edited(move |text| {
            if let Some(n) = me.upgrade() {
                n.edited(&text)
            }
        });
        let me = self.me.clone();
        g.on_toggle_pin(move || {
            if let Some(n) = me.upgrade() {
                n.toggle_pin()
            }
        });
        let me = self.me.clone();
        g.on_delete(move || {
            if let Some(n) = me.upgrade() {
                n.delete()
            }
        });
        self.windows.borrow_mut().push(ui.as_weak());
        self.sync(true);
    }

    fn on_show(&self) {
        self.sync(true);
    }

    fn on_hide(&self) {
        // Flush now: the app may be quit while the notch is closed.
        self.save_timer.stop();
        self.prune();
        self.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_dates_order() {
        assert_eq!(
            title_preview("\n Island ideas \nA glance\nmore"),
            ("Island ideas".into(), "A glance".into())
        );
        assert_eq!(
            title_preview(""),
            ("New note".into(), "No additional text".into())
        );

        let now = Local.with_ymd_and_hms(2026, 11, 1, 20, 0, 0).unwrap();
        let ago = |d: i64| (now - chrono::Duration::days(d)).timestamp();
        assert_eq!(when(ago(0), now), "Today");
        assert_eq!(when(ago(1), now), "Yesterday");
        assert_eq!(when(ago(10), now), "22.10.26");

        let n = |id, pinned, updated| Note {
            id,
            body: String::new(),
            pinned,
            updated,
        };
        let mut v = vec![n(1, false, 5), n(2, true, 1), n(3, false, 9)];
        sort(&mut v);
        assert_eq!(v.iter().map(|n| n.id).collect::<Vec<_>>(), [2, 3, 1]);
    }
}
