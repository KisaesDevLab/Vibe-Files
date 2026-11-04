//! Ten-key tape calculator engine (PHASES PA-14 step 1): adding-machine semantics, not
//! algebraic. Amounts are integer cents (no binary floating point), shown with two decimals and
//! thousands separators.
//!
//! - Type an amount, then `+` / `−` posts it to the running total (a tape line each).
//! - `S` prints the subtotal and keeps running; `T` prints the total, adds it to the grand total
//!   and starts again at zero; `GT` prints the grand total of the totals and clears it.
//! - `×` / `÷` business-calculator style: `12 × 3 +` prints `12.00 ×`, `3.00 =`, then posts
//!   `36.00 +`; `=` instead of `+`/`−` leaves the result as the entry, to post or keep working.
//! - `CE` clears the entry, `C` clears the tape and totals, `⌫` deletes a digit, `±` flips the
//!   entry's sign, `00` types two zeros. Every key that changes the tape can be undone and
//!   redone.

/// A key on the keypad.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Digit(u8),
    DoubleZero,
    Point,
    Plus,
    Minus,
    Times,
    Divide,
    Equals,
    Subtotal,
    Total,
    GrandTotal,
    ClearEntry,
    Clear,
    Backspace,
    Sign,
}

/// What a tape line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Plus,
    Minus,
    /// The left side of a multiplication.
    Times,
    /// The left side of a division.
    Divide,
    /// The right side of a multiplication or division.
    Equals,
    Subtotal,
    Total,
    GrandTotal,
}

impl Op {
    /// The mark printed beside the amount.
    pub fn mark(self) -> &'static str {
        match self {
            Op::Plus => "+",
            Op::Minus => "−",
            Op::Times => "×",
            Op::Divide => "÷",
            Op::Equals => "=",
            Op::Subtotal => "S",
            Op::Total => "T",
            Op::GrandTotal => "GT",
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Line {
    /// Cents.
    pub amount: i64,
    pub op: Op,
    /// Where it came from (`Interest income · p.1 box 1`), editable; empty when typed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// The number on the page it was picked from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
}

/// Where a picked amount is on its page: page (0-based) and its box in PDF user space.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Source {
    pub page: u32,
    pub rect: [f32; 4],
}

/// The entry being typed: digits, whether a point was typed, decimals typed, sign.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Entry {
    whole: String,
    frac: String,
    point: bool,
    negative: bool,
    /// A value placed by `=` or a pick (not typed digit by digit).
    set: Option<i64>,
}

impl Entry {
    fn is_empty(&self) -> bool {
        self.set.is_none() && self.whole.is_empty() && self.frac.is_empty() && !self.point
    }

    fn cents(&self) -> i64 {
        if let Some(v) = self.set {
            return v;
        }
        let whole: i64 = self.whole.parse().unwrap_or(0);
        let mut frac = self.frac.clone();
        while frac.len() < 2 {
            frac.push('0');
        }
        let frac: i64 = frac[..2].parse().unwrap_or(0);
        let v = whole.saturating_mul(100).saturating_add(frac);
        if self.negative { -v } else { v }
    }

    fn digit(&mut self, d: char) {
        if let Some(v) = self.set.take() {
            // Typing over a placed value starts a new amount.
            let _ = v;
            *self = Entry::default();
        }
        if self.point {
            if self.frac.len() < 2 {
                self.frac.push(d);
            }
        } else if self.whole == "0" {
            // A leading zero is replaced, but a lone 0 is still an amount (`10 ÷ 0`).
            self.whole = d.to_string();
        } else if self.whole.len() < 13 {
            self.whole.push(d);
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
struct State {
    lines: Vec<Line>,
    running: i64,
    grand: i64,
    entry: Entry,
    /// A `×` / `÷` waiting for its right side: (left, op).
    pending: Option<(i64, Op)>,
    /// Division by zero (shown until the next key).
    error: Option<String>,
}

/// A ten-key tape.
#[derive(Debug, Clone, Default)]
pub struct Tape {
    pub name: String,
    state: State,
    undo: Vec<State>,
    redo: Vec<State>,
}

/// `1234567` cents → `12,345.67`; negatives with a leading `-`.
pub fn format_cents(c: i64) -> String {
    let neg = c < 0;
    let a = c.unsigned_abs();
    let (whole, frac) = (a / 100, a % 100);
    let digits = whole.to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    format!("{}{out}.{frac:02}", if neg { "-" } else { "" })
}

/// Parses an amount as printed on a form: `1,284.17`, `$45`, `(45.00)` and `45.00-` negative.
/// `None` when it is not an amount.
pub fn parse_amount(text: &str) -> Option<i64> {
    let mut t = text.trim();
    let mut neg = false;
    if let Some(inner) = t.strip_prefix('(').and_then(|x| x.strip_suffix(')')) {
        t = inner.trim();
        neg = true;
    }
    if let Some(x) = t.strip_suffix('-') {
        t = x.trim_end();
        neg = !neg;
    }
    if let Some(x) = t.strip_prefix('-') {
        t = x.trim_start();
        neg = !neg;
    }
    t = t.strip_prefix('$').unwrap_or(t).trim_start();
    if t.is_empty() {
        return None;
    }
    let (whole, frac) = match t.split_once('.') {
        Some((w, f)) => (w, f),
        None => (t, ""),
    };
    if frac.len() > 2 || !frac.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    // Thousands separators only in groups of three.
    let groups: Vec<&str> = whole.split(',').collect();
    if groups
        .iter()
        .any(|g| g.is_empty() || !g.chars().all(|c| c.is_ascii_digit()))
        || (groups.len() > 1 && (groups[0].len() > 3 || groups[1..].iter().any(|g| g.len() != 3)))
    {
        return None;
    }
    let whole: i64 = groups.concat().parse().ok()?;
    let mut f = frac.to_string();
    while f.len() < 2 {
        f.push('0');
    }
    let v = whole
        .checked_mul(100)?
        .checked_add(f.parse::<i64>().ok()?)?;
    Some(if neg { -v } else { v })
}

/// `a × b` in cents, rounded half away from zero; `None` when it does not fit.
fn mul(a: i64, b: i64) -> Option<i64> {
    let p = i128::from(a) * i128::from(b);
    let r = (p.abs() + 50) / 100;
    i64::try_from(if p < 0 { -r } else { r }).ok()
}

/// `a ÷ b` in cents, rounded half away from zero; `None` for ÷ 0.
fn div(a: i64, b: i64) -> Option<i64> {
    if b == 0 {
        return None;
    }
    let n = i128::from(a) * 100;
    let d = i128::from(b);
    let q = (n.abs() * 2 + d.abs()) / (d.abs() * 2);
    i64::try_from(if (n < 0) != (d < 0) { -q } else { q }).ok()
}

const TOO_LARGE: &str = "Too large for the tape";

impl Tape {
    pub fn new(name: impl Into<String>) -> Self {
        Tape {
            name: name.into(),
            ..Default::default()
        }
    }

    pub fn lines(&self) -> &[Line] {
        &self.state.lines
    }

    /// Lines can be relabelled on the tape.
    pub fn set_label(&mut self, i: usize, label: impl Into<String>) {
        if let Some(l) = self.state.lines.get_mut(i) {
            l.label = label.into();
        }
    }

    /// The running total (cents).
    pub fn running(&self) -> i64 {
        self.state.running
    }

    pub fn grand(&self) -> i64 {
        self.state.grand
    }

    pub fn error(&self) -> Option<&str> {
        self.state.error.as_deref()
    }

    /// The display: the entry being typed, else the running total.
    pub fn display(&self) -> String {
        let e = &self.state.entry;
        if e.set.is_none() && !e.is_empty() {
            let whole = if e.whole.is_empty() {
                "0".to_string()
            } else {
                format_cents(e.whole.parse::<i64>().unwrap_or(0) * 100)
                    .trim_end_matches(".00")
                    .to_string()
            };
            let sign = if e.negative { "-" } else { "" };
            return if e.point {
                format!("{sign}{whole}.{}", e.frac)
            } else {
                format!("{sign}{whole}")
            };
        }
        format_cents(e.set.unwrap_or(self.state.running))
    }

    /// The pending entry in cents, when there is one.
    pub fn entry(&self) -> Option<i64> {
        (!self.state.entry.is_empty()).then(|| self.state.entry.cents())
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) {
        if let Some(s) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.state, s));
        }
    }

    pub fn redo(&mut self) {
        if let Some(s) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.state, s));
        }
    }

    /// Adds (or subtracts) an amount picked off a page, with its label: as if typed then `+`.
    pub fn pick(&mut self, cents: i64, minus: bool, label: impl Into<String>) {
        self.pick_at(cents, minus, label, None);
    }

    /// [`Tape::pick`] remembering where on the page the amount is.
    pub fn pick_at(
        &mut self,
        cents: i64,
        minus: bool,
        label: impl Into<String>,
        source: Option<Source>,
    ) {
        self.save();
        self.state.entry = Entry::default();
        let op = if minus { Op::Minus } else { Op::Plus };
        if self.post(cents, op, label.into())
            && let Some(l) = self.state.lines.last_mut()
        {
            l.source = source;
        }
    }

    /// The line picked from `rect` on `page`, when there is one.
    pub fn line_from(&self, page: u32, rect: [f32; 4]) -> Option<usize> {
        self.state.lines.iter().position(|l| {
            l.source.is_some_and(|s| {
                s.page == page && s.rect.iter().zip(rect).all(|(a, b)| (a - b).abs() < 0.5)
            })
        })
    }

    /// A tape read back from its lines (History, a pasted tape): the totals replayed.
    pub fn from_lines(name: impl Into<String>, lines: Vec<Line>) -> Tape {
        let mut t = Tape::new(name);
        t.state.lines = lines;
        t.replay();
        t
    }

    /// Recomputes the running total, the grand total and every S / T / GT line from the posted
    /// lines (after a sign flip, or a tape read back).
    fn replay(&mut self) {
        let (mut running, mut grand) = (0i64, 0i64);
        for l in &mut self.state.lines {
            match l.op {
                Op::Plus => running = running.saturating_add(l.amount),
                Op::Minus => running = running.saturating_sub(l.amount),
                Op::Subtotal => l.amount = running,
                Op::Total => {
                    l.amount = running;
                    grand = grand.saturating_add(running);
                    running = 0;
                }
                Op::GrandTotal => {
                    l.amount = grand;
                    grand = 0;
                }
                _ => {}
            }
        }
        self.state.running = running;
        self.state.grand = grand;
    }

    /// Replaces the lines with `lines` (a tape from History), named `name`, as one undoable step.
    pub fn load(&mut self, name: impl Into<String>, lines: Vec<Line>) {
        self.save();
        self.name = name.into();
        self.state = State {
            lines,
            ..State::default()
        };
        self.replay();
    }

    /// Forgets where on a page each line was picked (another document is shown: its pages are
    /// not the ones the picks came from). Amounts and labels stay.
    pub fn forget_sources(&mut self) {
        for l in &mut self.state.lines {
            l.source = None;
        }
        for s in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            for l in &mut s.lines {
                l.source = None;
            }
        }
    }

    /// Sets a picked line's sign (`+` ↔ `−`), keeping the totals right.
    pub fn flip(&mut self, i: usize) {
        let Some(l) = self.state.lines.get(i) else {
            return;
        };
        if !matches!(l.op, Op::Plus | Op::Minus) {
            return;
        }
        self.save();
        let l = &mut self.state.lines[i];
        l.op = if l.op == Op::Plus {
            Op::Minus
        } else {
            Op::Plus
        };
        // Later S / T / GT lines and the totals follow.
        self.replay();
    }

    fn save(&mut self) {
        self.undo.push(self.state.clone());
        self.redo.clear();
        if self.undo.len() > 500 {
            self.undo.remove(0);
        }
    }

    /// Posts a line; `false` (and the error shown, the press undone) when the total would not fit.
    fn post(&mut self, cents: i64, op: Op, label: String) -> bool {
        let r = match op {
            Op::Plus => self.state.running.checked_add(cents),
            Op::Minus => self.state.running.checked_sub(cents),
            _ => Some(self.state.running),
        };
        let Some(r) = r else {
            self.fail(TOO_LARGE);
            return false;
        };
        self.state.running = r;
        self.state.lines.push(Line {
            amount: cents,
            op,
            label,
            source: None,
        });
        true
    }

    /// The key just pressed failed: the tape goes back to before it, the pending `×` / `÷` (and
    /// its printed line) and the entry are dropped, so nothing half done is left, and `msg` shows.
    fn fail(&mut self, msg: &str) {
        if let Some(s) = self.undo.pop() {
            self.state = s;
        }
        if self.state.pending.take().is_some() {
            self.state.lines.pop();
        }
        self.state.entry = Entry::default();
        self.state.error = Some(msg.into());
    }

    /// The value a posting key works on: a pending `×` / `÷` finished with what was typed (its
    /// right side printed `=`), else what was typed. `Err` when the press failed (shown, undone).
    fn resolve(&mut self, typed: Option<i64>) -> Result<Option<i64>, ()> {
        match (self.state.pending.take(), typed) {
            (Some((left, pop)), Some(right)) => {
                let r = if pop == Op::Times {
                    mul(left, right).ok_or(TOO_LARGE)
                } else if right == 0 {
                    Err("Cannot divide by zero")
                } else {
                    div(left, right).ok_or(TOO_LARGE)
                };
                match r {
                    Ok(v) => {
                        self.line(right, Op::Equals);
                        Ok(Some(v))
                    }
                    Err(m) => {
                        self.fail(m);
                        Err(())
                    }
                }
            }
            // `12 × +`: the left side alone.
            (Some((left, _)), None) => Ok(Some(left)),
            (None, v) => Ok(v),
        }
    }

    fn line(&mut self, amount: i64, op: Op) {
        self.state.lines.push(Line {
            amount,
            op,
            label: String::new(),
            source: None,
        });
    }

    /// Presses a key.
    pub fn press(&mut self, k: Key) {
        // Typing only edits the entry: not an undo step of its own.
        match k {
            Key::Digit(d) => {
                self.state.error = None;
                self.state.entry.digit((b'0' + d.min(9)) as char);
                return;
            }
            Key::DoubleZero => {
                self.state.error = None;
                self.state.entry.digit('0');
                self.state.entry.digit('0');
                return;
            }
            Key::Point => {
                let e = &mut self.state.entry;
                if e.set.is_some() {
                    *e = Entry::default();
                }
                e.point = true;
                return;
            }
            Key::Backspace => {
                let e = &mut self.state.entry;
                if e.set.is_some() {
                    *e = Entry::default();
                } else if !e.frac.is_empty() {
                    e.frac.pop();
                } else if e.point {
                    e.point = false;
                } else {
                    e.whole.pop();
                }
                return;
            }
            Key::Sign => {
                let e = &mut self.state.entry;
                match &mut e.set {
                    Some(v) => *v = -*v,
                    None => e.negative = !e.negative,
                }
                return;
            }
            Key::ClearEntry => {
                self.state.entry = Entry::default();
                self.state.error = None;
                return;
            }
            _ => {}
        }
        self.save();
        self.state.error = None;
        let typed = (!self.state.entry.is_empty()).then(|| self.state.entry.cents());
        self.state.entry = Entry::default();
        match k {
            Key::Clear => {
                let name = std::mem::take(&mut self.name);
                self.state = State::default();
                self.name = name;
            }
            Key::Plus | Key::Minus | Key::Equals => {
                let op = match k {
                    Key::Plus => Op::Plus,
                    Key::Minus => Op::Minus,
                    _ => Op::Equals,
                };
                let Ok(value) = self.resolve(typed) else {
                    return;
                };
                let Some(v) = value else {
                    return; // nothing typed: nothing to post
                };
                if op == Op::Equals {
                    // The result waits as the entry.
                    self.state.entry.set = Some(v);
                } else {
                    self.post(v, op, String::new());
                }
            }
            Key::Times | Key::Divide => {
                let op = if k == Key::Times {
                    Op::Times
                } else {
                    Op::Divide
                };
                // A chained `2 × 3 × 4`: the product so far is the new left side; `×` with
                // nothing typed works on the running total.
                let Ok(left) = self.resolve(typed) else {
                    return;
                };
                let left = left.unwrap_or(self.state.running);
                self.line(left, op);
                self.state.pending = Some((left, op));
            }
            // S and T finish a pending `×` / `÷` first, as `+` does.
            Key::Subtotal => {
                let Ok(value) = self.resolve(typed) else {
                    return;
                };
                if let Some(v) = value
                    && !self.post(v, Op::Plus, String::new())
                {
                    return;
                }
                let r = self.state.running;
                self.line(r, Op::Subtotal);
            }
            Key::Total => {
                let Ok(value) = self.resolve(typed) else {
                    return;
                };
                if let Some(v) = value
                    && !self.post(v, Op::Plus, String::new())
                {
                    return;
                }
                let r = self.state.running;
                let Some(g) = self.state.grand.checked_add(r) else {
                    self.fail(TOO_LARGE);
                    return;
                };
                self.line(r, Op::Total);
                self.state.grand = g;
                self.state.running = 0;
            }
            Key::GrandTotal => {
                let g = self.state.grand;
                self.line(g, Op::GrandTotal);
                self.state.grand = 0;
            }
            _ => {}
        }
    }

    /// Presses each key of a sequence written as on a written table: digits, `.`, `00`, and
    /// `+ - * / = S T GT CE C BS ±` separated by spaces (`"12.50 + 3 + T"`).
    pub fn keys(&mut self, seq: &str) {
        for tok in seq.split_whitespace() {
            match tok {
                "+" => self.press(Key::Plus),
                "-" => self.press(Key::Minus),
                "*" | "x" | "×" => self.press(Key::Times),
                "/" | "÷" => self.press(Key::Divide),
                "=" => self.press(Key::Equals),
                "S" => self.press(Key::Subtotal),
                "T" => self.press(Key::Total),
                "GT" => self.press(Key::GrandTotal),
                "CE" => self.press(Key::ClearEntry),
                "C" => self.press(Key::Clear),
                "BS" => self.press(Key::Backspace),
                "±" => self.press(Key::Sign),
                "UNDO" => self.undo(),
                "REDO" => self.redo(),
                num => {
                    let mut rest = num;
                    while !rest.is_empty() {
                        if let Some(r) = rest.strip_prefix("00") {
                            self.press(Key::DoubleZero);
                            rest = r;
                            continue;
                        }
                        let c = rest.chars().next().expect("not empty");
                        match c {
                            '.' => self.press(Key::Point),
                            '0'..='9' => self.press(Key::Digit(c as u8 - b'0')),
                            _ => {}
                        }
                        rest = &rest[c.len_utf8()..];
                    }
                }
            }
        }
    }

    /// The tape as text, one line each (`label  1,234.56 +`), then the total: for Copy.
    pub fn text(&self) -> String {
        let mut out = String::new();
        if !self.name.trim().is_empty() {
            out.push_str(self.name.trim());
            out.push('\n');
        }
        for l in &self.state.lines {
            if l.label.is_empty() {
                out.push_str(&format!("{:>16} {}\n", format_cents(l.amount), l.op.mark()));
            } else {
                out.push_str(&format!(
                    "{}  {:>16} {}\n",
                    l.label,
                    format_cents(l.amount),
                    l.op.mark()
                ));
            }
        }
        out
    }

    /// The tape as tab-separated values (label, amount, mark) for Excel.
    pub fn tsv(&self) -> String {
        self.state
            .lines
            .iter()
            .map(|l| {
                format!(
                    "{}\t{}\t{}",
                    l.label,
                    format_cents(l.amount).replace(',', ""),
                    l.op.mark()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The written table (PA-14 done-when 1): sequence → (tape as `amount mark` pairs, running
    /// total after, grand total after, display).
    #[test]
    fn written_table_of_ten_key_sequences() {
        /// (keys, tape as (amount, mark), running total, grand total).
        type Row<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a str, &'a str);
        #[rustfmt::skip]
        let table: &[Row] = &[
            // 1 plain adding
            ("12.50 + 3 +", &[("12.50", "+"), ("3.00", "+")], "15.50", "0.00"),
            // 2 subtract
            ("100 + 25.25 -", &[("100.00", "+"), ("25.25", "−")], "74.75", "0.00"),
            // 3 subtotal keeps running
            ("10 + 5 + S 1 +", &[("10.00", "+"), ("5.00", "+"), ("15.00", "S"), ("1.00", "+")], "16.00", "0.00"),
            // 4 total resets and feeds GT
            ("10 + 5 + T", &[("10.00", "+"), ("5.00", "+"), ("15.00", "T")], "0.00", "15.00"),
            // 5 two totals and GT
            ("10 + T 20 + T GT", &[("10.00", "+"), ("10.00", "T"), ("20.00", "+"), ("20.00", "T"), ("30.00", "GT")], "0.00", "0.00"),
            // 6 00 key
            ("1 00 +", &[("100.00", "+")], "100.00", "0.00"),
            // 7 decimals cut at two
            ("1.239 +", &[("1.23", "+")], "1.23", "0.00"),
            // 8 thousands
            ("1234567.89 +", &[("1,234,567.89", "+")], "1,234,567.89", "0.00"),
            // 9 sign flip of the entry
            ("50 ± +", &[("-50.00", "+")], "-50.00", "0.00"),
            // 10 minus a negative
            ("50 ± -", &[("-50.00", "−")], "50.00", "0.00"),
            // 11 multiply and post
            ("12 * 3 +", &[("12.00", "×"), ("3.00", "="), ("36.00", "+")], "36.00", "0.00"),
            // 12 divide and post
            ("10 / 4 +", &[("10.00", "÷"), ("4.00", "="), ("2.50", "+")], "2.50", "0.00"),
            // 13 divide rounding half up
            ("10 / 3 +", &[("10.00", "÷"), ("3.00", "="), ("3.33", "+")], "3.33", "0.00"),
            // 14 divide rounding up
            ("20 / 3 +", &[("20.00", "÷"), ("3.00", "="), ("6.67", "+")], "6.67", "0.00"),
            // 15 multiply cents rounding
            ("1.25 * 1.5 +", &[("1.25", "×"), ("1.50", "="), ("1.88", "+")], "1.88", "0.00"),
            // 16 multiply then subtract
            ("100 + 4 * 2.5 -", &[("100.00", "+"), ("4.00", "×"), ("2.50", "="), ("10.00", "−")], "90.00", "0.00"),
            // 17 equals leaves the result as the entry, then posted
            ("6 * 7 = +", &[("6.00", "×"), ("7.00", "="), ("42.00", "+")], "42.00", "0.00"),
            // 18 chained multiply
            ("2 * 3 * 4 +", &[("2.00", "×"), ("3.00", "="), ("6.00", "×"), ("4.00", "="), ("24.00", "+")], "24.00", "0.00"),
            // 19 times on the running total
            ("10 + * 3 +", &[("10.00", "+"), ("10.00", "×"), ("3.00", "="), ("30.00", "+")], "40.00", "0.00"),
            // 20 percent-style: 200 × .05
            ("200 * .05 +", &[("200.00", "×"), ("0.05", "="), ("10.00", "+")], "10.00", "0.00"),
            // 21 clear entry
            ("12 CE 5 +", &[("5.00", "+")], "5.00", "0.00"),
            // 22 backspace
            ("123 BS +", &[("12.00", "+")], "12.00", "0.00"),
            // 23 backspace through the point
            ("1.5 BS BS 7 +", &[("17.00", "+")], "17.00", "0.00"),
            // 24 clear
            ("10 + 20 + C 5 +", &[("5.00", "+")], "5.00", "0.00"),
            // 25 undo the last posting
            ("10 + 20 + UNDO", &[("10.00", "+")], "10.00", "0.00"),
            // 26 undo then redo
            ("10 + 20 + UNDO REDO", &[("10.00", "+"), ("20.00", "+")], "30.00", "0.00"),
            // 27 undo a total
            ("10 + T UNDO", &[("10.00", "+")], "10.00", "0.00"),
            // 28 subtotal with an amount typed posts it first
            ("10 + 5 S", &[("10.00", "+"), ("5.00", "+"), ("15.00", "S")], "15.00", "0.00"),
            // 29 total with an amount typed
            ("10 + 5 T", &[("10.00", "+"), ("5.00", "+"), ("15.00", "T")], "0.00", "15.00"),
            // 30 plus with nothing typed does nothing
            ("10 + +", &[("10.00", "+")], "10.00", "0.00"),
            // 31 negative total into GT
            ("5 + 20 - T 10 + T GT", &[("5.00", "+"), ("20.00", "−"), ("-15.00", "T"), ("10.00", "+"), ("10.00", "T"), ("-5.00", "GT")], "0.00", "0.00"),
            // 32 a 1099-INT tie-out (boxes 1, 3, 8, 9; design close-up)
            ("1284.17 + 45 + 2540 + 222.50 + T", &[("1,284.17", "+"), ("45.00", "+"), ("2,540.00", "+"), ("222.50", "+"), ("4,091.67", "T")], "0.00", "4,091.67"),
            // 33 leading zeros ignored
            ("0007 +", &[("7.00", "+")], "7.00", "0.00"),
            // 34 point first
            (".75 +", &[("0.75", "+")], "0.75", "0.00"),
        ];
        assert!(table.len() >= 30);
        for (i, (seq, lines, running, grand)) in table.iter().enumerate() {
            let mut t = Tape::new("t");
            t.keys(seq);
            let got: Vec<(String, &str)> = t
                .lines()
                .iter()
                .map(|l| (format_cents(l.amount), l.op.mark()))
                .collect();
            let want: Vec<(String, &str)> =
                lines.iter().map(|(a, m)| (a.to_string(), *m)).collect();
            assert_eq!(got, want, "row {} `{seq}`", i + 1);
            assert_eq!(format_cents(t.running()), *running, "row {} running", i + 1);
            assert_eq!(format_cents(t.grand()), *grand, "row {} grand", i + 1);
        }
    }

    #[test]
    fn divide_by_zero_is_an_error_not_a_line() {
        let mut t = Tape::new("t");
        t.keys("10 / 0 +");
        assert_eq!(t.error(), Some("Cannot divide by zero"));
        assert_eq!(t.running(), 0);
        assert!(t.lines().is_empty(), "no half-done lines: {:?}", t.lines());
        t.keys("5 +");
        assert_eq!(t.error(), None);
        assert_eq!(t.running(), 500);
    }

    /// S and T finish a pending × / ÷ first (QA 2026-11-04).
    #[test]
    fn subtotal_and_total_finish_a_multiplication() {
        let mut t = Tape::new("t");
        t.keys("12 * 3 T");
        assert_eq!(t.grand(), 3600);
        assert_eq!(t.running(), 0);
        t.keys("5 +");
        assert_eq!(t.running(), 500, "no multiplication left pending");
        let mut t = Tape::new("t");
        t.keys("10 + 12 * 3 S");
        assert_eq!(t.running(), 4600);
        assert_eq!(
            t.lines().last().map(|l| (l.op, l.amount)),
            Some((Op::Subtotal, 4600))
        );
    }

    /// Flipping a pick after a total recomputes the total lines (QA 2026-11-04).
    #[test]
    fn flip_after_total_recomputes() {
        let mut t = Tape::new("t");
        t.pick(1000, false, "a");
        t.pick(250, false, "b");
        t.keys("T");
        t.flip(1);
        let total = t.lines().iter().find(|l| l.op == Op::Total).unwrap().amount;
        assert_eq!(total, 750);
        assert_eq!((t.running(), t.grand()), (0, 750));
        let again = Tape::from_lines("t", t.lines().to_vec());
        assert_eq!((again.running(), again.grand()), (0, 750));
    }

    /// Amounts too large for the tape are refused, never wrapped (QA 2026-11-04).
    #[test]
    fn overflow_is_refused() {
        let mut t = Tape::new("t");
        t.keys("9999999999999 * 9999999999999 +");
        assert_eq!(t.error(), Some(TOO_LARGE));
        assert_eq!(t.running(), 0);
        assert!(t.lines().is_empty());
    }

    #[test]
    fn display_shows_the_entry_then_the_total() {
        let mut t = Tape::new("t");
        t.keys("1234.5");
        assert_eq!(t.display(), "1,234.5");
        t.keys("+");
        assert_eq!(t.display(), "1,234.50");
        t.keys("6 * 7 =");
        assert_eq!(t.display(), "42.00");
    }

    #[test]
    fn picks_flip_and_label() {
        let mut t = Tape::new("1099-INT tie-out");
        t.pick(128417, false, "Interest income · p.1 box 1");
        t.pick(4500, false, "p.1 box 3");
        t.flip(1);
        assert_eq!(t.running(), 128417 - 4500);
        assert_eq!(t.lines()[1].op, Op::Minus);
        t.undo();
        assert_eq!(t.running(), 128417 + 4500);
        t.set_label(1, "Interest on US bonds · p.1 box 3");
        assert!(t.text().contains("Interest on US bonds · p.1 box 3"));
        assert!(t.tsv().contains("1284.17\t+"));
    }

    #[test]
    fn sources_and_replay() {
        let mut t = Tape::new("t");
        let src = Source {
            page: 0,
            rect: [10.0, 20.0, 50.0, 30.0],
        };
        t.pick_at(128417, false, "box 1", Some(src));
        t.keys("45 + T");
        assert_eq!(t.line_from(0, [10.2, 20.0, 50.0, 30.0]), Some(0));
        assert_eq!(t.line_from(1, src.rect), None);
        let back = Tape::from_lines("t", t.lines().to_vec());
        assert_eq!(back.running(), t.running());
        assert_eq!(back.grand(), 132917);
        let json = serde_json::to_string(t.lines()).unwrap();
        let lines: Vec<Line> = serde_json::from_str(&json).unwrap();
        assert_eq!(lines, t.lines());
    }

    #[test]
    fn amounts_parse_as_printed_on_forms() {
        assert_eq!(parse_amount("1,284.17"), Some(128417));
        assert_eq!(parse_amount("$45"), Some(4500));
        assert_eq!(parse_amount("(45.00)"), Some(-4500));
        assert_eq!(parse_amount("45.00-"), Some(-4500));
        assert_eq!(parse_amount("2540"), Some(254000));
        assert_eq!(parse_amount("12,34"), None);
        assert_eq!(parse_amount("1.234"), None);
        assert_eq!(parse_amount("box"), None);
        assert_eq!(parse_amount(""), None);
    }
}
