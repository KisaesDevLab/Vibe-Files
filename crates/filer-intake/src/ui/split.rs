//! The Split screen (UI_SPEC §10): page thumbnails with cut toggles in the gaps between pages, a
//! preview of the selected page with what was matched on it, and one card per segment (client,
//! rule, file name, copies). It takes the Preview tab's body while open.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use egui::{Align, Color32, CornerRadius, Frame, Layout, Margin, RichText, Stroke};

use crate::clients::{Client, ClientList};
use crate::intake::QueuedFile;
use crate::parse::Parsed;
use crate::pdf::{self, Thumbnailer};
use crate::relationships::Relationships;
use crate::rules::{CompiledRules, OnSplit, Resolver, Rule};
use crate::split::{self, Finder, Found, PageClient};
use crate::ui::theme::{PillKind, label_caps, mono, pal, pill, primary_button, strong};
use crate::ui::widgets::{PickerOutcome, client_picker};
use crate::worker;

const THUMB_WIDTH: u32 = 92;
/// Thumbnails kept in memory (spec: LRU of 50).
pub const THUMB_CACHE: usize = 50;
const PREVIEW_WIDTH: u32 = 520;
const PREVIEW_CACHE: usize = 8;
/// Left borders of the segment cards, in turn; amber marks a segment without a client.
const SEGMENT_COLORS: [Color32; 4] = [
    Color32::from_rgb(0x0F, 0x6B, 0x8E),
    Color32::from_rgb(0x6E, 0x56, 0xCF),
    Color32::from_rgb(0x2F, 0x85, 0x5A),
    Color32::from_rgb(0xB8, 0x32, 0x80),
];

pub enum SplitOutcome {
    Open,
    Cancel,
    Apply(Vec<PieceSpec>),
}

/// One piece to write: its pages, its client and a file name typed on the screen.
#[derive(Debug, Clone)]
pub struct PieceSpec {
    pub pages: Range<usize>,
    pub client_id: Option<String>,
    pub filename: Option<String>,
}

/// What the original row knows, so each segment is named the way Preview will name its piece.
pub struct SplitContext {
    pub clients: Arc<ClientList>,
    pub relationships: Arc<Relationships>,
    pub rules: Vec<Rule>,
    pub parsed: Parsed,
    pub year: Option<u16>,
    pub rule_override: Option<String>,
    pub keep_original_name: bool,
    pub sniff: bool,
    pub id_pattern: String,
    pub fuzzy_suggest: f32,
    pub fuzzy_auto: f64,
    pub current_year: i32,
}

/// Pages, their text, detected clients and proposed cuts, read on a worker thread.
struct Analysis {
    total: usize,
    texts: Vec<String>,
    page_clients: Vec<Option<PageClient>>,
    cuts: BTreeSet<usize>,
    has_text: bool,
}

/// A segment as shown: its client (or the best name match to offer) and how it will be filed.
struct SegView {
    pages: Range<usize>,
    client: Option<Client>,
    found_on: Option<(usize, Found)>,
    chosen: bool,
    suggestion: Option<(f64, Client)>,
    /// Rule and whether it came from the segment's first page text.
    rule: Option<(Rule, bool)>,
    filename: String,
}

/// Rendered pages for one size, with a small LRU.
struct Pages {
    renderer: Option<Thumbnailer>,
    images: HashMap<usize, egui::TextureHandle>,
    order: VecDeque<usize>,
    requested: HashSet<usize>,
    keep: usize,
    name: &'static str,
}

impl Pages {
    fn new(renderer: Option<Thumbnailer>, keep: usize, name: &'static str) -> Self {
        Self {
            renderer,
            images: HashMap::new(),
            order: VecDeque::new(),
            requested: HashSet::new(),
            keep,
            name,
        }
    }

    fn poll(&mut self, ctx: &egui::Context, file: &str) {
        let Some(r) = &self.renderer else { return };
        for (page, image) in r.finished() {
            if let Ok(image) = image {
                let tex = ctx.load_texture(
                    format!("{}-{file}-{page}", self.name),
                    image,
                    egui::TextureOptions::LINEAR,
                );
                self.images.insert(page, tex);
                self.order.push_back(page);
                while self.order.len() > self.keep {
                    if let Some(old) = self.order.pop_front() {
                        self.images.remove(&old);
                        self.requested.remove(&old);
                    }
                }
            }
        }
    }

    /// The page's texture, asking for it if it isn't there yet.
    fn get(&mut self, page: usize) -> Option<&egui::TextureHandle> {
        if !self.images.contains_key(&page)
            && let Some(r) = &self.renderer
            && self.requested.insert(page)
        {
            r.request(page);
        }
        self.images.get(&page)
    }
}

pub struct SplitDialog {
    pub file: QueuedFile,
    cx: SplitContext,
    compiled: Option<CompiledRules>,
    job: Option<Receiver<Result<Analysis, String>>>,
    analysis: Option<Analysis>,
    error: Option<String>,
    cuts: BTreeSet<usize>,
    /// Client chosen by hand for the segment starting at this page.
    chosen: HashMap<usize, String>,
    /// File name typed for the segment starting at this page.
    typed: HashMap<usize, String>,
    selected: usize,
    thumbs: Pages,
    preview: Pages,
    picker: Option<(usize, String)>,
}

impl SplitDialog {
    pub fn open(
        ctx: &egui::Context,
        file: QueuedFile,
        cx: SplitContext,
        pdfium_dir: Option<PathBuf>,
    ) -> Self {
        let path = file.path.clone();
        let list = Arc::clone(&cx.clients);
        let dir = pdfium_dir.clone();
        let (id_pattern, threshold, year) =
            (cx.id_pattern.clone(), cx.fuzzy_suggest, cx.current_year);
        let job = worker::background(ctx, move || {
            let total = pdf::page_count(&path)?;
            let texts = match &dir {
                Some(d) => pdf::pdfium(d)
                    .and_then(|p| pdf::page_texts(p, &path))
                    .unwrap_or_else(|_| vec![String::new(); total]),
                None => vec![String::new(); total],
            };
            let has_text = texts.iter().any(|t| !t.trim().is_empty());
            let finder = Finder::new(&list, &id_pattern, threshold, year)?;
            let (page_clients, segments) = split::propose(&texts, &finder);
            let cuts = segments.iter().skip(1).map(|s| s.pages.start).collect();
            Ok(Analysis {
                total,
                texts,
                page_clients,
                cuts,
                has_text,
            })
        });
        let renderer = |width| {
            pdfium_dir
                .clone()
                .map(|d| Thumbnailer::new(ctx, d, file.path.clone(), width))
        };
        let (thumbs, preview) = (renderer(THUMB_WIDTH), renderer(PREVIEW_WIDTH));
        Self {
            compiled: CompiledRules::new(&cx.rules).ok(),
            cx,
            file,
            job: Some(job),
            analysis: None,
            error: None,
            cuts: BTreeSet::new(),
            chosen: HashMap::new(),
            typed: HashMap::new(),
            selected: 0,
            thumbs: Pages::new(thumbs, THUMB_CACHE, "thumb"),
            preview: Pages::new(preview, PREVIEW_CACHE, "page"),
            picker: None,
        }
    }

    #[cfg(test)]
    pub fn ready(&self) -> bool {
        self.analysis.is_some() || self.error.is_some()
    }

    #[cfg(test)]
    pub fn pieces(&self) -> Vec<(Range<usize>, Option<String>)> {
        self.segments()
            .iter()
            .map(|s| (s.pages.clone(), s.client.as_ref().map(|c| c.id.clone())))
            .collect()
    }

    /// Thumbnails in memory, for the footer ("thumbnail cache 12 / 50").
    pub fn thumb_count(&self) -> usize {
        self.thumbs.images.len()
    }

    fn file_name(&self) -> String {
        self.file
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Current segments with clients, rules and names.
    fn segments(&self) -> Vec<SegView> {
        let Some(a) = &self.analysis else {
            return Vec::new();
        };
        let list = &self.cx.clients;
        split::segments_from_cuts(a.total, &self.cuts, &a.page_clients)
            .into_iter()
            .map(|s| {
                let by_id = |id: &str| list.match_by_id(id).cloned();
                let chosen = self.chosen.get(&s.pages.start);
                // A close name below auto-resolve is offered, not taken (as in Preview).
                let (client, suggestion) = match (chosen, &s.found_on, &s.client_id) {
                    (Some(id), ..) => (by_id(id), None),
                    (None, Some((_, Found::Similar(score))), Some(id))
                        if *score < self.cx.fuzzy_auto =>
                    {
                        (None, by_id(id).map(|c| (*score, c)))
                    }
                    (None, _, Some(id)) => (by_id(id), None),
                    (None, _, None) => (None, None),
                };
                let rule = self.rule_for(&a.texts, s.pages.start);
                let filename = self.name_for(client.as_ref(), rule.as_ref().map(|r| &r.0));
                SegView {
                    chosen: chosen.is_some(),
                    pages: s.pages,
                    client,
                    found_on: s.found_on,
                    suggestion,
                    rule,
                    filename,
                }
            })
            .collect()
    }

    /// The rule Preview will use for a piece starting at `start`: the row's own choice, else the
    /// file name, else the piece's first page text, else the catch-all.
    fn rule_for(&self, texts: &[String], start: usize) -> Option<(Rule, bool)> {
        if let Some(r) = self
            .cx
            .rule_override
            .as_deref()
            .and_then(|id| self.cx.rules.iter().find(|r| r.id == id))
        {
            return Some((r.clone(), false));
        }
        let c = self.compiled.as_ref()?;
        let rest = &self.cx.parsed.rest;
        let ut = self.cx.parsed.ultratax.is_some();
        if let Some(r) = c.first_specific_match(rest, ut) {
            return Some((r.clone(), false));
        }
        if self.cx.sniff
            && let Some(r) = texts.get(start).and_then(|t| c.first_specific_match(t, ut))
        {
            return Some((r.clone(), true));
        }
        Some((c.first_match(rest, ut).clone(), false))
    }

    fn name_for(&self, client: Option<&Client>, rule: Option<&Rule>) -> String {
        if self.cx.keep_original_name {
            return self.file_name();
        }
        let (Some(client), Some(rule)) = (client, rule) else {
            return String::new();
        };
        match crate::rules::evaluate_rule(rule, &self.cx.parsed, client, self.cx.year).outcome {
            Ok(r) => r.filename,
            Err(_) => String::new(),
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.job
            && let Ok(result) = rx.try_recv()
        {
            self.job = None;
            match result {
                Ok(a) => {
                    self.cuts = a.cuts.clone();
                    self.analysis = Some(a);
                }
                Err(e) => self.error = Some(e),
            }
        }
        let name = self.file.path.display().to_string();
        self.thumbs.poll(ctx, &name);
        self.preview.poll(ctx, &name);
    }

    pub fn show(&mut self, ui: &mut egui::Ui) -> SplitOutcome {
        self.poll(ui.ctx());
        let mut outcome = SplitOutcome::Open;
        let p = pal(ui);
        let segs = self.segments();
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("← Preview").clicked() {
                outcome = SplitOutcome::Cancel;
            }
            ui.add_space(6.0);
            ui.label(strong("Split").size(15.0));
            ui.label(mono(self.file_name()).color(p.text));
            if let Some(a) = &self.analysis {
                ui.label(
                    RichText::new(format!(
                        "· {} pages · {}",
                        a.total,
                        match (a.has_text, a.cuts.is_empty()) {
                            (true, false) => "auto-proposed from text layer",
                            (true, true) => "text layer present",
                            (false, _) => "no text layer — manual cuts",
                        }
                    ))
                    .color(p.muted),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if !a.has_text {
                        pill(ui, PillKind::Warn, "click the gaps to cut");
                    } else if a.cuts.is_empty() {
                        pill(
                            ui,
                            PillKind::Neutral,
                            "no other client found in the page text",
                        );
                    } else {
                        let n = a.cuts.len();
                        pill(
                            ui,
                            PillKind::Info,
                            format!(
                                "{n} cut{} proposed from client IDs and names in page text",
                                if n == 1 { "" } else { "s" }
                            ),
                        );
                    }
                });
            }
        });
        ui.add_space(6.0);
        if let Some(e) = &self.error {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("Cannot read this PDF: {e}"),
            );
            return outcome;
        }
        let Some(total) = self.analysis.as_ref().map(|a| a.total) else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Reading pages…");
            });
            return outcome;
        };
        self.strip_ui(ui, total);
        ui.add_space(10.0);
        let body = (ui.available_height() - 40.0).max(240.0);
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), body),
            Layout::left_to_right(Align::Min),
            |ui| {
                let left = (ui.available_width() * 0.34).clamp(300.0, 460.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(left, body),
                    Layout::top_down(Align::Min),
                    |ui| self.preview_ui(ui, body),
                );
                ui.add_space(12.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), body),
                    Layout::top_down(Align::Min),
                    |ui| self.segments_ui(ui, &segs, total),
                );
            },
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!(
                    "Writes {} PDFs to data\\tmp\\; a rule with \"Whole\" copies keeps the original as that copy instead of deleting it.",
                    segs.len()
                ))
                .size(12.0)
                .color(p.muted),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let ok = segs.len() >= 2;
                let apply = ui.add_enabled(
                    ok,
                    primary_button(ui, format!("Apply split · {} rows", segs.len())),
                );
                if !ok {
                    apply.clone().on_disabled_hover_text("Add a cut first.");
                }
                if apply.clicked() {
                    outcome = SplitOutcome::Apply(
                        segs.iter()
                            .map(|s| PieceSpec {
                                pages: s.pages.clone(),
                                client_id: s.client.as_ref().map(|c| c.id.clone()),
                                filename: self
                                    .typed
                                    .get(&s.pages.start)
                                    .filter(|t| !t.trim().is_empty() && **t != s.filename)
                                    .cloned(),
                            })
                            .collect(),
                    );
                }
                if ui.button("Cancel").clicked() {
                    outcome = SplitOutcome::Cancel;
                }
            });
        });
        self.picker_ui(ui.ctx());
        outcome
    }

    fn strip_ui(&mut self, ui: &mut egui::Ui, total: usize) {
        let p = pal(ui);
        let mut toggle: Option<usize> = None;
        let page_clients: Vec<Option<PageClient>> = self
            .analysis
            .as_ref()
            .map(|a| a.page_clients.clone())
            .unwrap_or_default();
        crate::ui::theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                label_caps(ui, "Pages");
                ui.label(
                    RichText::new(
                        "Click the gap between two pages to add or remove a cut. Click a page to preview it.",
                    )
                    .size(12.0)
                    .color(p.muted),
                );
            });
            ui.add_space(8.0);
            let size = egui::vec2(THUMB_WIDTH as f32, THUMB_WIDTH as f32 * 1.3);
            egui::ScrollArea::horizontal()
                .id_salt("split_strip")
                .show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        for page in 0..total {
                            if page > 0 {
                                let (rect, resp) = ui.allocate_exact_size(
                                    egui::vec2(18.0, size.y),
                                    egui::Sense::click(),
                                );
                                let label = format!("Cut before page {}", page + 1);
                                let on = self.cuts.contains(&page);
                                resp.widget_info(|| {
                                    egui::WidgetInfo::selected(egui::Role::Button, true, on, &label)
                                });
                                let x = rect.center().x;
                                if on {
                                    ui.painter().rect_filled(
                                        egui::Rect::from_center_size(
                                            rect.center(),
                                            egui::vec2(4.0, size.y),
                                        ),
                                        2.0,
                                        p.attention,
                                    );
                                } else {
                                    let color = if resp.hovered() { p.muted } else { p.border };
                                    ui.painter().vline(
                                        x,
                                        rect.y_range().shrink(8.0),
                                        Stroke::new(1.0, color),
                                    );
                                }
                                if resp.on_hover_text(&label).clicked() {
                                    toggle = Some(page);
                                }
                            }
                            ui.vertical(|ui| {
                                ui.set_width(size.x + 12.0);
                                ui.spacing_mut().item_spacing.y = 4.0;
                                let (rect, resp) = ui.allocate_exact_size(
                                    egui::vec2(size.x + 12.0, size.y),
                                    egui::Sense::click(),
                                );
                                let r = egui::Rect::from_center_size(rect.center(), size);
                                if ui.is_rect_visible(rect)
                                    && let Some(tex) = self.thumbs.get(page)
                                {
                                    egui::Image::new(tex).paint_at(ui, r);
                                }
                                let selected = self.selected == page;
                                ui.painter().rect_stroke(
                                    r,
                                    2.0,
                                    if selected {
                                        Stroke::new(2.0, p.accent)
                                    } else {
                                        Stroke::new(1.0, p.border)
                                    },
                                    egui::StrokeKind::Outside,
                                );
                                let mut job = egui::text::LayoutJob::default();
                                let font = egui::FontId::proportional(12.0);
                                job.append(
                                    &(page + 1).to_string(),
                                    0.0,
                                    egui::TextFormat::simple(font.clone(), p.muted),
                                );
                                match page_clients.get(page).cloned().flatten() {
                                    Some(PageClient {
                                        client_id,
                                        found: Found::Id,
                                        ..
                                    }) => {
                                        job.append(
                                            " · ",
                                            0.0,
                                            egui::TextFormat::simple(font.clone(), p.muted),
                                        );
                                        job.append(
                                            &client_id,
                                            0.0,
                                            egui::TextFormat::simple(font, p.accent),
                                        );
                                    }
                                    Some(_) => {
                                        job.append(
                                            " · name",
                                            0.0,
                                            egui::TextFormat::simple(font, p.muted),
                                        );
                                    }
                                    None => {}
                                }
                                let label = ui.vertical_centered(|ui| ui.label(job)).inner;
                                if resp.labelled_by(label.id).clicked() {
                                    self.selected = page;
                                }
                            });
                        }
                    });
                });
        });
        if let Some(p) = toggle {
            if !self.cuts.remove(&p) {
                self.cuts.insert(p);
            }
            // Clients and names chosen by hand stay with the segment that still starts there.
            let starts: HashSet<usize> = std::iter::once(0)
                .chain(self.cuts.iter().copied())
                .collect();
            self.chosen.retain(|start, _| starts.contains(start));
            self.typed.retain(|start, _| starts.contains(start));
        }
    }

    fn preview_ui(&mut self, ui: &mut egui::Ui, height: f32) {
        let p = pal(ui);
        let page = self.selected;
        let matched = self
            .analysis
            .as_ref()
            .and_then(|a| a.page_clients.get(page).cloned().flatten());
        Frame::new()
            .fill(p.card)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::same(6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_height(height - 2.0);
                ui.spacing_mut().item_spacing.y = 0.0;
                Frame::new()
                    .inner_margin(Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            label_caps(ui, &format!("Page {} preview", page + 1));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(
                                    RichText::new("rendered by pdfium · cached")
                                        .size(12.0)
                                        .color(p.muted),
                                );
                            });
                        });
                    });
                let footer = 38.0;
                let area = egui::vec2(
                    ui.available_width(),
                    (ui.available_height() - footer).max(80.0),
                );
                let (rect, _) = ui.allocate_exact_size(area, egui::Sense::hover());
                ui.painter().rect_filled(rect, 0.0, p.paper);
                ui.painter()
                    .hline(rect.x_range(), rect.top(), Stroke::new(1.0, p.border));
                ui.painter()
                    .hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, p.border));
                match self.preview.get(page) {
                    Some(tex) => {
                        let img = tex.size_vec2();
                        let scale = ((rect.width() - 40.0) / img.x)
                            .min((rect.height() - 40.0) / img.y)
                            .max(0.05);
                        let r = egui::Rect::from_center_size(rect.center(), img * scale);
                        egui::Image::new(tex).paint_at(ui, r);
                        ui.painter().rect_stroke(
                            r,
                            0.0,
                            Stroke::new(1.0, p.border),
                            egui::StrokeKind::Outside,
                        );
                    }
                    None => {
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            if self.preview.renderer.is_some() {
                                "Rendering…"
                            } else {
                                "No preview without pdfium"
                            },
                            egui::FontId::proportional(12.0),
                            p.muted,
                        );
                    }
                }
                Frame::new()
                    .inner_margin(Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            let muted = |t: &str| RichText::new(t).size(12.0).color(p.muted);
                            match &matched {
                                Some(m) => {
                                    ui.label(muted("Matched on this page:"));
                                    let client = self.cx.clients.match_by_id(&m.client_id);
                                    if m.found == Found::Id {
                                        ui.label(
                                            RichText::new(&m.client_id).size(12.0).color(p.accent),
                                        );
                                        ui.label(muted("·"));
                                    }
                                    let mut what = client
                                        .map_or_else(String::new, |c| format!("\"{}\"", c.name));
                                    if let Found::Similar(s) = m.found {
                                        what.push_str(&format!(" (similar {s:.2})"));
                                    }
                                    if let Some(line) = m.line {
                                        what.push_str(&format!(" (line {line})"));
                                    }
                                    ui.label(RichText::new(what).size(12.0).color(p.text));
                                }
                                None => {
                                    ui.label(muted("Nothing matched on this page."));
                                }
                            }
                        });
                    });
            });
    }

    fn segments_ui(&mut self, ui: &mut egui::Ui, segs: &[SegView], total: usize) {
        let p = pal(ui);
        ui.horizontal(|ui| {
            label_caps(ui, "Segments");
            ui.label(
                RichText::new(format!(
                    "· each becomes its own row in Preview; the original is removed after all {} commit",
                    segs.len()
                ))
                .size(12.0)
                .color(p.muted),
            );
        });
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .id_salt("split_segments")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (k, s) in segs.iter().enumerate() {
                    self.segment_card(ui, k, s, total);
                    ui.add_space(8.0);
                }
            });
    }

    fn segment_card(&mut self, ui: &mut egui::Ui, k: usize, s: &SegView, total: usize) {
        let p = pal(ui);
        let color = if s.client.is_some() {
            SEGMENT_COLORS[k % SEGMENT_COLORS.len()]
        } else {
            p.attention
        };
        let resp = Frame::new()
            .fill(p.card)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin {
                left: 16,
                right: 14,
                top: 12,
                bottom: 12,
            })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(strong(format!("Segment {}", k + 1)));
                    let pages = if s.pages.len() == 1 {
                        format!("page {}", s.pages.start + 1)
                    } else {
                        format!("pages {}–{}", s.pages.start + 1, s.pages.end)
                    };
                    ui.label(RichText::new(pages).color(p.muted));
                    match (&s.found_on, s.chosen) {
                        (_, true) => {
                            pill(ui, PillKind::Info, "chosen");
                        }
                        (Some((page, Found::Id)), false) => {
                            let id = s.client.as_ref().map_or("", |c| c.id.as_str());
                            pill(ui, PillKind::Ok, format!("ID {id} on page {}", page + 1));
                        }
                        (Some((page, Found::Name)), false) => {
                            pill(ui, PillKind::Ok, format!("name on page {}", page + 1));
                        }
                        (Some((_, Found::Similar(score))), false) => {
                            pill(ui, PillKind::Warn, format!("name only · {score:.2}"));
                        }
                        (None, false) => {
                            pill(ui, PillKind::Warn, "no client found");
                        }
                    }
                });
                ui.add_space(6.0);
                egui::Grid::new(("segment_grid", k))
                    .num_columns(2)
                    .spacing([16.0, 8.0])
                    .min_col_width(80.0)
                    .show(ui, |ui| {
                        label_caps(ui, "Client");
                        let text = match (&s.client, &s.suggestion) {
                            (Some(c), _) => format!("{} · {}", c.name, c.id),
                            (None, Some((score, c))) => {
                                format!("Choose… (best: {} · {score:.2})", c.name)
                            }
                            (None, None) => "Choose…".to_string(),
                        };
                        let stroke = if s.client.is_some() {
                            Stroke::new(1.0, p.button_border)
                        } else {
                            Stroke::new(1.5, p.attention)
                        };
                        // A left-aligned field like a dropdown (egui buttons center their text).
                        let width = (ui.available_width() - 4.0).clamp(200.0, 460.0);
                        let (rect, field) =
                            ui.allocate_exact_size(egui::vec2(width, 28.0), egui::Sense::click());
                        field.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::Button, true, &text)
                        });
                        let fill = if field.hovered() { p.hover } else { p.card };
                        ui.painter()
                            .rect(rect, 4.0, fill, stroke, egui::StrokeKind::Inside);
                        ui.painter().text(
                            rect.left_center() + egui::vec2(10.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            &text,
                            egui::FontId::proportional(13.5),
                            p.text,
                        );
                        if field.clicked() {
                            let query = s
                                .suggestion
                                .as_ref()
                                .map(|(_, c)| c.name.clone())
                                .unwrap_or_default();
                            self.picker = Some((s.pages.start, query));
                        }
                        ui.end_row();

                        label_caps(ui, "Rule");
                        ui.horizontal(|ui| match (&s.client, &s.rule) {
                            (Some(_), Some((rule, sniffed))) => {
                                ui.label(&rule.name);
                                if *sniffed {
                                    pill(ui, PillKind::Neutral, "sniffed");
                                }
                            }
                            _ => {
                                ui.label(
                                    RichText::new("computed after client is chosen").color(p.muted),
                                );
                            }
                        });
                        ui.end_row();

                        let label = label_caps(ui, "Filename");
                        let mut name = self
                            .typed
                            .get(&s.pages.start)
                            .cloned()
                            .unwrap_or_else(|| s.filename.clone());
                        let width = (ui.available_width() - 4.0).clamp(200.0, 460.0);
                        let edit = ui.add(
                            egui::TextEdit::singleline(&mut name)
                                .desired_width(width)
                                .hint_text(if s.client.is_some() {
                                    "set in Preview"
                                } else {
                                    "after a client is chosen"
                                }),
                        );
                        if edit.labelled_by(label.id).changed() {
                            self.typed.insert(s.pages.start, name);
                        }
                        ui.end_row();

                        label_caps(ui, "Copies");
                        ui.horizontal_wrapped(|ui| self.copies_summary(ui, s, total));
                        ui.end_row();
                    });
            })
            .response;
        let r = resp.rect;
        ui.painter().rect_filled(
            egui::Rect::from_min_max(r.min, egui::pos2(r.min.x + 4.0, r.max.y)),
            CornerRadius {
                nw: 6,
                sw: 6,
                ne: 0,
                se: 0,
            },
            color,
        );
    }

    /// Who gets copies of this segment, from the rule's copies (resolved for real in Preview).
    fn copies_summary(&self, ui: &mut egui::Ui, s: &SegView, total: usize) {
        let p = pal(ui);
        let muted = |t: String| RichText::new(t).size(12.0).color(p.muted);
        let Some((rule, _)) = &s.rule else {
            ui.label(muted("none for this rule".into()));
            return;
        };
        if rule.copies.is_empty() {
            ui.label(muted("none for this rule".into()));
            return;
        }
        let Some(client) = &s.client else {
            ui.label(muted("after a client is chosen".into()));
            return;
        };
        let names = |ids: Vec<String>| -> String {
            let mut v: Vec<String> = ids
                .iter()
                .map(|id| {
                    self.cx
                        .clients
                        .match_by_id(id)
                        .map_or_else(|| id.clone(), |c| c.name.clone())
                })
                .collect();
            if v.len() > 3 {
                let more = v.len() - 3;
                v.truncate(3);
                v.push(format!("+{more} more"));
            }
            v.join(", ")
        };
        let captures = self
            .compiled
            .as_ref()
            .map(|c| c.captures(&rule.id, &self.cx.parsed.rest))
            .unwrap_or_default();
        let n = s.pages.len();
        for cd in &rule.copies {
            let on = match cd.on_split {
                OnSplit::Segment => "Segment",
                OnSplit::Whole => "Whole",
            };
            let what = match cd.on_split {
                OnSplit::Segment => format!(
                    "only {} {n} page{} go to",
                    if n == 1 { "this" } else { "these" },
                    if n == 1 { "" } else { "s" }
                ),
                OnSplit::Whole => format!("the whole PDF ({total} pages) goes to"),
            };
            let (tag, who) = match &cd.resolver {
                Resolver::Related { roles } => {
                    let kids: Vec<String> = self
                        .cx
                        .relationships
                        .children(&client.id, roles)
                        .into_iter()
                        .map(|r| r.child_id.clone())
                        .collect();
                    let n = kids.len();
                    let who = if kids.is_empty() {
                        "no related clients".to_string()
                    } else {
                        format!("{what} {}", names(kids))
                    };
                    (format!("{n} related"), who)
                }
                Resolver::RelatedByName { group } => {
                    let who = match captures.get(group).filter(|v| !v.is_empty()) {
                        Some(name) => format!("{what} \"{name}\" (matched in Preview)"),
                        None => "no name in the file name; picked in Preview".into(),
                    };
                    ("by name".to_string(), who)
                }
                Resolver::Capture { group } => {
                    let who = match captures.get(group).filter(|v| !v.is_empty()) {
                        Some(id) => format!("{what} {}", names(vec![id.clone()])),
                        None => "no ID in the file name; picked in Preview".into(),
                    };
                    ("by ID".to_string(), who)
                }
                Resolver::Manual => ("picked in Preview".to_string(), String::new()),
            };
            pill(ui, PillKind::Info, format!("{tag} · on split: {on}"));
            if !who.is_empty() {
                ui.label(muted(who));
            }
        }
    }

    fn picker_ui(&mut self, ctx: &egui::Context) {
        let Some((start, query)) = &mut self.picker else {
            return;
        };
        let title = format!("Client for the segment starting on page {}", *start + 1);
        let clients = Arc::clone(&self.cx.clients);
        match client_picker(ctx, &title, query, &clients) {
            PickerOutcome::Open => {}
            PickerOutcome::Cancel => self.picker = None,
            PickerOutcome::NewClient(_) => {}
            PickerOutcome::Chosen(c) => {
                let start = *start;
                self.chosen.insert(start, c.id);
                self.picker = None;
            }
        }
    }
}
