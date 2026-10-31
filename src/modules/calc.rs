//! macOS-style basic calculator: operator precedence (2 + 3 × 4 = 14), repeated `=`,
//! `%` relative to a pending + / −, AC/C. The engine is pure and unit-tested; the module
//! glue below only pushes its display to the windows.

use super::{Ctx, Module};
use crate::{CalcState, NotchWindow};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::{Rc, Weak};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
}

impl Op {
    fn prec(self) -> u8 {
        match self {
            Op::Add | Op::Sub => 1,
            Op::Mul | Op::Div => 2,
        }
    }

    fn apply(self, a: f64, b: f64) -> f64 {
        match self {
            Op::Add => a + b,
            Op::Sub => a - b,
            Op::Mul => a * b,
            Op::Div => a / b,
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "+" => Some(Op::Add),
            "-" | "−" => Some(Op::Sub),
            "*" | "×" | "x" => Some(Op::Mul),
            "/" | "÷" => Some(Op::Div),
            _ => None,
        }
    }
}

/// Max digits typed into one number, like the macOS calculator.
const MAX_DIGITS: usize = 9;

#[derive(Debug, Clone, Default)]
pub struct Engine {
    /// The number being typed or the last result, as shown.
    entry: String,
    /// True while the user is typing `entry`; the next digit appends.
    typing: bool,
    /// Pending operands and operators, lowest precedence first (at most 2 deep).
    stack: Vec<(f64, Op)>,
    /// Last operator + operand, for repeated `=`.
    repeat: Option<(Op, f64)>,
    /// The operator just pressed (highlighted in the UI) while no new number is typed.
    selected: Option<Op>,
    error: bool,
}

impl Engine {
    pub fn display(&self) -> String {
        if self.error {
            return "Error".into();
        }
        if self.entry.is_empty() {
            "0".into()
        } else {
            self.entry.clone()
        }
    }

    /// "C" while there is an entry to clear, "AC" otherwise.
    pub fn clear_label(&self) -> &'static str {
        if self.typing && self.entry != "0" {
            "C"
        } else {
            "AC"
        }
    }

    pub fn selected(&self) -> Option<Op> {
        self.selected
    }

    fn value(&self) -> f64 {
        self.entry.parse().unwrap_or(0.0)
    }

    fn show(&mut self, v: f64) {
        if v.is_finite() {
            self.entry = format(v);
        } else {
            *self = Engine {
                error: true,
                ..Engine::default()
            };
        }
    }

    /// Handle one key: digits, ".", + − × ÷ (or + - * /), "=", "%", "±", "AC", "C",
    /// "Backspace". Returns false for keys the calculator doesn't use.
    pub fn press(&mut self, key: &str) -> bool {
        if self.error && key != "Backspace" {
            *self = Engine::default();
        }
        match key {
            "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" => self.digit(key),
            "." | "," => self.dot(),
            "=" | "Enter" => self.equals(),
            "%" => self.percent(),
            "±" | "neg" => self.negate(),
            "AC" => *self = Engine::default(),
            "C" | "Delete" | "Clear" => self.clear(),
            "Backspace" => self.backspace(),
            k => match Op::parse(k) {
                Some(op) => self.op(op),
                None => return false,
            },
        }
        true
    }

    fn digit(&mut self, d: &str) {
        if !self.typing || self.entry == "0" {
            self.entry = d.into();
            self.typing = true;
        } else if self.entry.chars().filter(char::is_ascii_digit).count() < MAX_DIGITS {
            self.entry.push_str(d);
        }
        self.selected = None;
    }

    fn dot(&mut self) {
        if !self.typing {
            self.entry = "0.".into();
            self.typing = true;
        } else if !self.entry.contains('.') {
            self.entry.push('.');
        }
        self.selected = None;
    }

    fn op(&mut self, op: Op) {
        // Changing your mind about the operator: replace it.
        if !self.typing
            && self.selected.is_some()
            && let Some(top) = self.stack.last_mut()
        {
            top.1 = op;
            // A lower-precedence op may now fold what's below it.
            let (v, _) = self.stack.pop().unwrap_or((0.0, op));
            return self.push(v, op);
        }
        let v = self.value();
        self.push(v, op);
    }

    fn push(&mut self, mut v: f64, op: Op) {
        while let Some(&(a, top)) = self.stack.last()
            && top.prec() >= op.prec()
        {
            self.stack.pop();
            v = top.apply(a, v);
        }
        self.stack.push((v, op));
        self.show(v);
        self.typing = false;
        self.selected = Some(op);
    }

    fn equals(&mut self) {
        let mut v = self.value();
        if self.stack.is_empty() {
            if let Some((op, b)) = self.repeat {
                v = op.apply(v, b);
            }
        } else {
            let operand = v;
            let last = self.stack.last().map(|&(_, op)| op);
            while let Some((a, op)) = self.stack.pop() {
                v = op.apply(a, v);
            }
            self.repeat = last.map(|op| (op, operand));
        }
        self.show(v);
        self.typing = false;
        self.selected = None;
    }

    fn percent(&mut self) {
        let v = self.value();
        let p = match self.stack.last() {
            Some(&(a, Op::Add | Op::Sub)) => a * v / 100.0,
            _ => v / 100.0,
        };
        self.show(p);
        self.typing = true;
    }

    fn negate(&mut self) {
        if let Some(rest) = self.entry.strip_prefix('-') {
            self.entry = rest.into();
        } else {
            self.entry = format!("-{}", self.display());
        }
        // Negating a result keeps it editable as a new entry.
        self.typing = true;
        self.selected = None;
    }

    fn clear(&mut self) {
        self.entry = "0".into();
        self.typing = false;
    }

    fn backspace(&mut self) {
        if self.error {
            *self = Engine::default();
        } else if self.typing {
            self.entry.pop();
            if self.entry.is_empty() || self.entry == "-" {
                self.entry = "0".into();
            }
        }
    }
}

/// At most 10 significant digits, no trailing zeros; scientific when huge or tiny.
pub fn format(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    let a = v.abs();
    if !(1e-9..1e15).contains(&a) {
        let s = format!("{v:.6e}");
        let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
        let m = m.trim_end_matches('0').trim_end_matches('.');
        return format!("{m}e{e}");
    }
    let int_digits = a.log10().floor() as i32 + 1;
    let decimals = (10 - int_digits).max(0) as usize;
    let s = format!("{v:.decimals$}");
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        &s
    };
    if s == "-0" { "0".into() } else { s.to_string() }
}

pub struct Calc {
    me: Weak<Calc>,
    engine: RefCell<Engine>,
    windows: RefCell<Vec<slint::Weak<NotchWindow>>>,
}

impl Calc {
    pub fn new(_ctx: Ctx) -> Rc<Self> {
        Rc::new_cyclic(|me| Self {
            me: me.clone(),
            engine: RefCell::default(),
            windows: RefCell::default(),
        })
    }

    fn sync(&self) {
        let e = self.engine.borrow();
        let display = e.display();
        let selected = match e.selected() {
            Some(Op::Add) => "+",
            Some(Op::Sub) => "−",
            Some(Op::Mul) => "×",
            Some(Op::Div) => "÷",
            None => "",
        };
        self.windows.borrow_mut().retain(|w| {
            let Some(ui) = w.upgrade() else { return false };
            let g = ui.global::<CalcState>();
            g.set_display(display.as_str().into());
            g.set_clear_label(e.clear_label().into());
            g.set_selected_op(selected.into());
            true
        });
    }
}

impl Module for Calc {
    fn id(&self) -> &'static str {
        "calc"
    }

    fn title(&self) -> &'static str {
        "Calculator"
    }

    fn attach(&self, ui: &NotchWindow) {
        let me = self.me.clone();
        ui.global::<CalcState>().on_press(move |key| {
            if let Some(c) = me.upgrade()
                && c.engine.borrow_mut().press(&key)
            {
                c.sync();
            }
        });
        self.windows.borrow_mut().push(ui.as_weak());
        self.sync();
    }

    fn key(&self, key: &super::KeyInput) -> bool {
        let Some(key) = key.plain() else { return false };
        let handled = self.engine.borrow_mut().press(key);
        if handled {
            self.sync();
        }
        handled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(keys: &str) -> String {
        let mut e = Engine::default();
        for k in keys.split_whitespace() {
            assert!(e.press(k), "key {k}");
        }
        e.display()
    }

    #[test]
    fn basics() {
        assert_eq!(run("1 2 + 3 0 ="), "42");
        assert_eq!(run("7 ÷ 2 ="), "3.5");
        assert_eq!(run("0 . 1 + 0 . 2 ="), "0.3");
        assert_eq!(run("5 - 8 ="), "-3");
    }

    #[test]
    fn precedence() {
        assert_eq!(run("2 + 3 × 4 ="), "14");
        assert_eq!(run("2 × 3 + 4 ="), "10");
        assert_eq!(run("1 0 - 2 × 3 - 1 ="), "3");
    }

    #[test]
    fn intermediate_results_show_like_macos() {
        assert_eq!(run("2 + 3 +"), "5");
        assert_eq!(run("2 + 3 ×"), "3", "× binds tighter: nothing to fold yet");
    }

    #[test]
    fn repeated_equals() {
        assert_eq!(run("2 + 3 = ="), "8");
        assert_eq!(run("2 × 3 = = ="), "54");
    }

    #[test]
    fn operator_change() {
        assert_eq!(run("6 + × 2 ="), "12");
        assert_eq!(run("2 × 3 × + 1 ="), "7");
    }

    #[test]
    fn percent() {
        assert_eq!(run("5 0 + 1 0 % ="), "55");
        assert_eq!(run("5 0 %"), "0.5");
        assert_eq!(run("2 0 0 × 1 0 % ="), "20");
    }

    #[test]
    fn negate_clear_backspace() {
        assert_eq!(run("5 ±"), "-5");
        assert_eq!(run("5 ± ±"), "5");
        assert_eq!(run("1 2 3 Backspace"), "12");
        assert_eq!(run("9 + 4 C 1 ="), "10", "C clears only the entry");
        assert_eq!(run("9 + 4 AC 1 ="), "1");
    }

    #[test]
    fn errors_and_limits() {
        assert_eq!(run("1 ÷ 0 ="), "Error");
        assert_eq!(run("1 ÷ 0 = 7"), "7", "next digit recovers");
        assert_eq!(run("1 2 3 4 5 6 7 8 9 9 9"), "123456789");
        assert_eq!(run("1 . . 5"), "1.5");
    }

    #[test]
    fn formatting() {
        assert_eq!(format(1.0 / 3.0), "0.3333333333");
        assert_eq!(format(1e20), "1e20");
        assert_eq!(format(123456789.0 * 1000.0), "123456789000");
        assert_eq!(format(-0.0), "0");
        assert_eq!(format(2.5e-12), "2.5e-12");
    }

    #[test]
    fn clear_label() {
        let mut e = Engine::default();
        assert_eq!(e.clear_label(), "AC");
        e.press("4");
        assert_eq!(e.clear_label(), "C");
    }
}
