//! The lists of Admin › Portal › Balances, Deliveries and Organizers (owner 2026-11-26): every
//! column resized by dragging its edge, sorted by a click on its heading (again to reverse),
//! and a Filter box keeping the rows whose text has every word typed. Widths and sort are kept
//! on this PC, `<data>/grids.json`; the filter starts empty.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use egui::RichText;
use egui_extras::{Column, TableBuilder, TableRow};
use serde::{Deserialize, Serialize};

use crate::ui::{theme, widgets};

static FILE: OnceLock<PathBuf> = OnceLock::new();

/// Where the widths and sorts are kept (set once at start).
pub fn set_data_dir(data: &Path) {
    let _ = FILE.set(data.join("grids.json"));
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    widths: Vec<f32>,
    /// The column sorted, and whether it goes up.
    #[serde(default)]
    sort: Option<(usize, bool)>,
}

fn read_all() -> BTreeMap<String, Saved> {
    FILE.get()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn store(id: &str, s: &Saved) {
    let Some(p) = FILE.get() else { return };
    let mut all = read_all();
    all.insert(id.to_string(), s.clone());
    if let Ok(b) = serde_json::to_vec_pretty(&all) {
        let _ = std::fs::write(p, b);
    }
}

/// A column: its heading and starting width. A `fixed` one (the tick) has that width, no
/// heading to sort by and no edge to drag.
#[derive(Debug, Clone, Copy)]
pub struct Col {
    pub title: &'static str,
    pub width: f32,
    pub fixed: bool,
}

impl Col {
    pub const fn new(title: &'static str, width: f32) -> Self {
        Self {
            title,
            width,
            fixed: false,
        }
    }
    pub const fn tick() -> Self {
        Self {
            title: "",
            width: 24.0,
            fixed: true,
        }
    }
}

/// What a cell sorts by: amounts, days and years as numbers, the rest as text (an ISO date
/// sorts as text). `None` (an empty cell) goes last either way.
#[derive(Debug, Clone, PartialEq)]
pub enum Key {
    Text(String),
    Num(f64),
    None,
}

fn compare(a: &Key, b: &Key) -> Ordering {
    match (a, b) {
        (Key::Num(x), Key::Num(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (Key::Text(x), Key::Text(y)) => x.to_lowercase().cmp(&y.to_lowercase()),
        (Key::Num(_), Key::Text(_)) => Ordering::Less,
        (Key::Text(_), Key::Num(_)) => Ordering::Greater,
        (Key::None, Key::None) => Ordering::Equal,
        (Key::None, _) => Ordering::Greater,
        (_, Key::None) => Ordering::Less,
    }
}

/// The rows of `n` kept by `filter` (every word, any column, any case), in the order sorted.
/// Ties keep the list's own order.
fn order_of(
    n: usize,
    filter: &str,
    sort: Option<(usize, bool)>,
    text: impl Fn(usize) -> String,
    key: impl Fn(usize, usize) -> Key,
) -> Vec<usize> {
    let words: Vec<String> = filter.split_whitespace().map(str::to_lowercase).collect();
    let mut v: Vec<usize> = (0..n)
        .filter(|i| {
            if words.is_empty() {
                return true;
            }
            let t = text(*i).to_lowercase();
            words.iter().all(|w| t.contains(w.as_str()))
        })
        .collect();
    if let Some((c, up)) = sort {
        v.sort_by(|a, b| {
            let (ka, kb) = (key(*a, c), key(*b, c));
            // Empty cells last whichever way it goes.
            match (&ka, &kb) {
                (Key::None, Key::None) => Ordering::Equal,
                (Key::None, _) => Ordering::Greater,
                (_, Key::None) => Ordering::Less,
                _ if up => compare(&ka, &kb),
                _ => compare(&kb, &ka),
            }
        });
    }
    v
}

/// One page's grid: its id in `grids.json`, the filter typed, the widths and sort.
pub struct Grid {
    id: &'static str,
    loaded: bool,
    saved: Saved,
    pub filter: String,
}

impl Grid {
    pub fn new(id: &'static str) -> Self {
        Self {
            id,
            loaded: false,
            saved: Saved::default(),
            filter: String::new(),
        }
    }

    fn ensure(&mut self) {
        if !self.loaded {
            self.loaded = true;
            self.saved = read_all().remove(self.id).unwrap_or_default();
        }
    }

    /// "Filter [ … ] Clear  3 of 41".
    pub fn filter_bar(&mut self, ui: &mut egui::Ui, shown: usize, total: usize) {
        ui.horizontal(|ui| {
            let l = ui.label("Filter");
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("Any column")
                    .desired_width(220.0),
            )
            .labelled_by(l.id);
            if !self.filter.is_empty()
                && ui
                    .small_button("Clear")
                    .on_hover_text("Clear the filter")
                    .clicked()
            {
                self.filter.clear();
            }
            if !self.filter.trim().is_empty() {
                ui.label(widgets::caption(format!("{shown} of {total}")));
            }
        });
    }

    /// The rows of `n` shown, filtered and sorted: `text(row)` is the row's words for the
    /// filter, `key(row, column)` what a cell sorts by.
    pub fn order(
        &mut self,
        n: usize,
        text: impl Fn(usize) -> String,
        key: impl Fn(usize, usize) -> Key,
    ) -> Vec<usize> {
        self.ensure();
        order_of(n, &self.filter, self.saved.sort, text, key)
    }

    /// The table with these columns, at the widths last dragged (the last column takes what
    /// is left).
    pub fn table<'a>(&mut self, ui: &'a mut egui::Ui, cols: &[Col]) -> TableBuilder<'a> {
        self.ensure();
        let mut t = TableBuilder::new(ui)
            .id_salt(self.id)
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .max_scroll_height(f32::INFINITY);
        for (i, c) in cols.iter().enumerate() {
            let last = i + 1 == cols.len();
            let w = self
                .saved
                .widths
                .get(i)
                .copied()
                .filter(|w| *w >= 30.0)
                .unwrap_or(c.width);
            t = t.column(if c.fixed {
                Column::exact(c.width)
            } else if last {
                Column::remainder().at_least(60.0).clip(true)
            } else {
                Column::initial(w).at_least(40.0).clip(true).resizable(true)
            });
        }
        t
    }

    /// The headings: a click sorts by the column (again to reverse). Widths dragged are kept
    /// once the mouse is let go.
    pub fn header(&mut self, h: &mut TableRow, cols: &[Col]) {
        let mut widths = Vec::with_capacity(cols.len());
        let mut clicked = None;
        let mut idle = true;
        for (i, c) in cols.iter().enumerate() {
            let sort = self.saved.sort;
            let (_, cell) = h.col(|ui| {
                idle = ui.ctx().input(|x| !x.pointer.any_down());
                if c.fixed || c.title.is_empty() {
                    return;
                }
                let r = ui
                    .add(
                        egui::Button::new(RichText::new(c.title).small().color(theme::ink_muted()))
                            .frame(false),
                    )
                    .on_hover_text("Sort by this column (click again to reverse)");
                if r.clicked() {
                    clicked = Some(i);
                }
                // Painted, as Browse's (the bundled fonts have no ▲/▼).
                if let Some((s, up)) = sort
                    && s == i
                {
                    crate::ui::browse::sort_arrow(ui, up);
                }
            });
            // The cell's own rect is the column's width (the first is what was drawn).
            widths.push(cell.rect.width());
        }
        let mut changed = false;
        if let Some(i) = clicked {
            self.saved.sort = match self.saved.sort {
                Some((s, up)) if s == i => Some((i, !up)),
                _ => Some((i, true)),
            };
            changed = true;
        }
        let moved = widths.len() != self.saved.widths.len()
            || widths
                .iter()
                .zip(&self.saved.widths)
                .take(cols.len().saturating_sub(1))
                .any(|(a, b)| (a - b).abs() > 1.0);
        if moved && idle {
            self.saved.widths = widths;
            changed = true;
        }
        if changed {
            store(self.id, &self.saved);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<(&'static str, Option<i64>)> {
        vec![
            ("Smith, Pat", Some(450)),
            ("anders LLC", None),
            ("Smithe LLC", Some(120)),
            ("Baker", Some(75)),
        ]
    }

    fn order(filter: &str, sort: Option<(usize, bool)>) -> Vec<&'static str> {
        let r = rows();
        order_of(
            r.len(),
            filter,
            sort,
            |i| {
                format!(
                    "{} {}",
                    r[i].0,
                    r[i].1.map(|n| n.to_string()).unwrap_or_default()
                )
            },
            |i, c| match c {
                0 => Key::Text(r[i].0.into()),
                _ => r[i].1.map_or(Key::None, |n| Key::Num(n as f64)),
            },
        )
        .into_iter()
        .map(|i| r[i].0)
        .collect()
    }

    #[test]
    fn rows_are_filtered_by_every_word_and_sorted_either_way() {
        assert_eq!(order("", None).len(), 4);
        assert_eq!(order("SMITH", None), ["Smith, Pat", "Smithe LLC"]);
        assert_eq!(order("smith llc", None), ["Smithe LLC"]);
        assert_eq!(order("450", None), ["Smith, Pat"]);
        // Text without regard to case.
        assert_eq!(
            order("", Some((0, true))),
            ["anders LLC", "Baker", "Smith, Pat", "Smithe LLC"]
        );
        // Numbers as numbers; the empty one last both ways.
        assert_eq!(
            order("", Some((1, true))),
            ["Baker", "Smithe LLC", "Smith, Pat", "anders LLC"]
        );
        assert_eq!(
            order("", Some((1, false))),
            ["Smith, Pat", "Smithe LLC", "Baker", "anders LLC"]
        );
    }
}
