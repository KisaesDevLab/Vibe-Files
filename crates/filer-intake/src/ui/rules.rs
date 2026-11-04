//! Rules tab (UI_SPEC §7): the active profile's ordered rule list (drag to reorder, right-click
//! for Move / Duplicate / Delete), the edit card, and a test box that shows which rule a filename
//! would hit. Edits a draft; Save writes rules.json.

use egui::{Align, CornerRadius, Frame, Layout, Margin, RichText, Stroke};

use crate::app::AppState;
use crate::clients::{self, Client};
use crate::config::Level;
use crate::parse::{self, Parsed};
use crate::rulebuild::{self, Form};
use crate::rules::{self, CompiledRules, CopyDest, HitProblem, OnSplit, Resolver, Rule, RulesFile};
use crate::template::{self, BUILTIN_TOKENS};
use crate::ui::setup::set_aside;
use crate::ui::theme::banner;
use crate::ui::theme::{
    self, PillKind, card, label_caps, mono, pal, pill, primary_button, strong, token_pill,
};

pub struct RulesScreen {
    draft: RulesFile,
    /// Index of the rule shown in the edit form, in the active profile.
    selected: usize,
    test_input: String,
    /// Profile name being typed after Rename.
    renaming_profile: Option<String>,
    /// Copy whose roles are being typed (index, comma-separated text).
    roles_edit: Option<(usize, String)>,
    message: Option<(Level, String)>,
    /// When rules.json was last written (footer).
    saved_at: Option<chrono::DateTime<chrono::Local>>,
    builder: Builder,
    /// Worked out from the draft; kept until it changes (compiling every pattern each frame
    /// was most of this tab's cost).
    checked: std::cell::RefCell<Option<Checked>>,
    /// The test name read like an inbox file name, for one name and setup.
    test_parsed: std::cell::RefCell<Option<(u64, Parsed)>>,
    /// Queued file names and the text rules match on, for one queue and setup.
    queue_rests: Option<(u64, Vec<(String, String)>)>,
}

/// Validation and compiled rules for one draft and set of CSV tokens.
struct Checked {
    draft: RulesFile,
    tokens: Vec<String>,
    file_errors: Vec<String>,
    profile_errors: Vec<(usize, String)>,
    compiled: Result<CompiledRules, String>,
}

/// What reading a file name depends on besides the name: the ID pattern, UltraTax layout,
/// client list and suggest threshold.
fn name_setup_key(state: &AppState, h: &mut impl std::hash::Hasher) {
    use std::hash::Hash;
    state.settings.id_pattern.hash(h);
    state.settings.ultratax.hash(h);
    state.settings.fuzzy_suggest.to_bits().hash(h);
    state
        .clients
        .list()
        .map_or(0, |l| std::sync::Arc::as_ptr(l) as usize)
        .hash(h);
}

/// Which list a clicked word goes to (V10).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Target {
    #[default]
    Any,
    All,
    None,
    Starts,
    Ends,
}

/// The rule builder card (V10).
#[derive(Debug, Default)]
struct Builder {
    form: Form,
    target: Target,
    typed: String,
    /// "Edit regex by hand": these replace the generated ones.
    by_hand: bool,
    hand_pattern: String,
    hand_exclude: String,
    name: String,
    /// Destination folder below the client, as a template part (`Income Tax/{Year}`).
    dest: String,
    filename: String,
}

enum RowAction {
    Select(usize),
    Toggle(usize),
    Move(usize, usize),
    Delete(usize),
    Duplicate(usize),
}

impl RulesScreen {
    pub fn new(state: &AppState) -> Self {
        Self {
            draft: state.rules.clone(),
            selected: 0,
            test_input: String::new(),
            renaming_profile: None,
            roles_edit: None,
            message: None,
            builder: Builder {
                filename: "{Year} {DocType}.{Ext}".into(),
                ..Default::default()
            },
            saved_at: std::fs::metadata(rules::path(&state.data_dir))
                .and_then(|m| m.modified())
                .ok()
                .map(chrono::DateTime::from),
            checked: Default::default(),
            test_parsed: Default::default(),
            queue_rests: None,
        }
    }

    /// Validation and compiled rules for the current draft, worked out again only after it
    /// (or the CSV tokens) changed.
    fn checked(&self, state: &AppState) -> std::cell::RefMut<'_, Checked> {
        let tokens = clients::extra_tokens(&state.settings.column_map);
        let mut c = self.checked.borrow_mut();
        if c.as_ref()
            .is_none_or(|c| c.draft != self.draft || c.tokens != tokens)
        {
            *c = Some(Checked {
                file_errors: rules::validate_file(&self.draft, &tokens),
                profile_errors: rules::validate_profile(self.draft.active_rules(), &tokens),
                compiled: CompiledRules::new(self.draft.active_rules()),
                draft: self.draft.clone(),
                tokens,
            });
        }
        std::cell::RefMut::map(c, |c| c.as_mut().expect("set above"))
    }

    /// The test name read like an inbox file name, kept while the name and setup are the same.
    fn parsed_test(&self, state: &AppState) -> Parsed {
        use std::hash::{Hash, Hasher};
        let input = self.test_input.trim();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        input.hash(&mut h);
        name_setup_key(state, &mut h);
        let key = h.finish();
        let mut c = self.test_parsed.borrow_mut();
        match c.as_ref() {
            Some((k, p)) if *k == key => p.clone(),
            _ => {
                let p = test_parsed(state, input);
                *c = Some((key, p.clone()));
                p
            }
        }
    }

    pub fn saved_at(&self) -> Option<chrono::DateTime<chrono::Local>> {
        self.saved_at
    }

    /// Header Profile dropdown (X120): makes `name` the active profile and saves rules.json at
    /// once. Refused while there are unsaved edits. Returns true when saved.
    pub fn switch_profile(&mut self, state: &mut AppState, name: String) -> bool {
        if self.is_dirty(state) || !self.draft.profiles.contains_key(&name) {
            return false;
        }
        self.draft.active = name;
        if self.save(state) {
            return true;
        }
        self.draft = state.rules.clone();
        false
    }

    #[cfg(test)]
    pub fn draft_rules(&self) -> &[Rule] {
        self.draft.active_rules()
    }

    pub fn is_dirty(&self, state: &AppState) -> bool {
        self.draft != state.rules
    }

    fn errors(&self, state: &AppState) -> Vec<String> {
        self.checked(state).file_errors.clone()
    }

    pub fn can_save(&self, state: &AppState) -> bool {
        self.errors(state).is_empty()
    }

    /// Writes rules.json. Returns true when saved (or nothing needed saving).
    pub fn save(&mut self, state: &mut AppState) -> bool {
        if !self.is_dirty(state) && state.rules_savable {
            return true;
        }
        if !state.rules_savable {
            if let Err(e) = set_aside(&rules::path(&state.data_dir)) {
                self.message = Some((
                    Level::Error,
                    format!("Could not set aside the unreadable rules.json: {e}"),
                ));
                return false;
            }
            state.rules_savable = true;
            state.rules_error = None;
        }
        match rules::save(&state.data_dir, &self.draft) {
            Ok(()) => {
                tracing::info!("rules saved");
                state.rules = self.draft.clone();
                self.saved_at = Some(chrono::Local::now());
                self.message = Some((Level::Info, "Rules saved.".into()));
                true
            }
            Err(e) => {
                tracing::error!(error = %e, "saving rules failed");
                self.message = Some((Level::Error, format!("Saving failed: {e}")));
                false
            }
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, state: &mut AppState) {
        if let Some(name) = state.rule_test.take() {
            self.test_input = name;
        }
        if let Some(e) = &state.rules_error {
            banner(
                ui,
                Level::Error,
                &format!(
                    "{e} Saving will keep the old file as rules.json.corrupt-<time> and write a new one."
                ),
            );
        }
        let extra_tokens = clients::extra_tokens(&state.settings.column_map);
        let rule_errors = self.checked(state).profile_errors.clone();
        ui.add_space(4.0);
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(400.0);
                self.list_side(ui, &rule_errors);
            });
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                egui::ScrollArea::vertical()
                    .id_salt("rules_right")
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        card(ui, |ui| {
                            ui.set_width(ui.available_width());
                            self.form_ui(ui, state, &rule_errors, &extra_tokens);
                        });
                        ui.add_space(10.0);
                        card(ui, |ui| {
                            ui.set_width(ui.available_width());
                            self.test_ui(ui, state);
                        });
                        ui.add_space(10.0);
                        card(ui, |ui| {
                            ui.set_width(ui.available_width());
                            self.builder_ui(ui, state);
                        });
                    });
            });
        });
    }

    fn list_side(&mut self, ui: &mut egui::Ui, rule_errors: &[(usize, String)]) {
        let p = pal(ui);
        // Profile line.
        ui.horizontal(|ui| {
            match &mut self.renaming_profile {
                Some(name) => {
                    let edit = ui.add(egui::TextEdit::singleline(name).desired_width(180.0));
                    edit.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Profile name")
                    });
                    if !edit.has_focus() && !edit.lost_focus() {
                        edit.request_focus();
                    }
                    if edit.lost_focus() {
                        let new_name = name.trim().to_string();
                        let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
                        if !escape
                            && !new_name.is_empty()
                            && new_name != self.draft.active
                            && !self.draft.profiles.contains_key(&new_name)
                            && let Some(rules) = self.draft.profiles.remove(&self.draft.active)
                        {
                            self.draft.profiles.insert(new_name.clone(), rules);
                            self.draft.active = new_name;
                        }
                        self.renaming_profile = None;
                    }
                }
                None => {
                    ui.label(strong(format!("Profile: {}", self.draft.active)).size(15.0));
                    ui.label(
                        RichText::new(" · first match wins")
                            .size(12.0)
                            .color(p.muted),
                    );
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let can_delete = self.draft.profiles.len() > 1;
                if ui
                    .add_enabled(can_delete, egui::Button::new("Delete").small())
                    .on_hover_text("Delete this profile")
                    .on_disabled_hover_text("There must be at least one profile.")
                    .clicked()
                {
                    self.draft.profiles.remove(&self.draft.active);
                    self.draft.normalize();
                    self.selected = 0;
                }
                if ui
                    .add(egui::Button::new("Rename").small())
                    .on_hover_text("Rename this profile")
                    .clicked()
                {
                    self.renaming_profile = Some(self.draft.active.clone());
                }
                if ui
                    .add(egui::Button::new("New").small())
                    .on_hover_text("New profile (a copy of this one)")
                    .clicked()
                {
                    self.new_profile();
                }
            });
        });
        ui.add_space(4.0);

        // Rules card, catch-all pinned at the bottom.
        let card_height = (ui.available_height() - 44.0).max(240.0);
        let mut action = None;
        // Bounded, so the pinned catch-all row sits at the card's bottom, not the window's.
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), card_height),
            Layout::top_down(Align::Min),
            |ui| {
                Frame::new()
                    .fill(p.card)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(CornerRadius::same(6))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.set_min_height(card_height - 2.0);
                        header_row(ui);
                        let rules = self.draft.active_rules();
                        let count = rules.len();
                        let mut catch_all = None;
                        egui::ScrollArea::vertical()
                            .id_salt("rules_list")
                            .max_height(card_height - 80.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                for (i, rule) in rules.iter().enumerate() {
                                    if rule.catch_all {
                                        catch_all = Some(i);
                                        continue;
                                    }
                                    let has_error = rule_errors.iter().any(|(j, _)| *j == i);
                                    if let Some(a) =
                                        rule_row(ui, i, count, rule, self.selected == i, has_error)
                                    {
                                        action = Some(a);
                                    }
                                }
                            });
                        if let Some(i) = catch_all {
                            let rule = &rules[i];
                            ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                if let Some(a) = catch_all_row(ui, i, rule, self.selected == i) {
                                    action = Some(a);
                                }
                                ui.separator();
                            });
                        }
                    });
            },
        );
        self.apply_row_action(action);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Add rule").clicked() {
                self.selected = rules::add_rule(self.draft.active_rules_mut());
            }
            let editable = self
                .draft
                .active_rules()
                .get(self.selected)
                .is_some_and(|r| !r.catch_all);
            if ui
                .add_enabled(editable, egui::Button::new("Duplicate"))
                .clicked()
            {
                self.apply_row_action(Some(RowAction::Duplicate(self.selected)));
            }
            if ui
                .add_enabled(editable, egui::Button::new("Delete rule"))
                .on_hover_text("Deletes the selected rule (Catch-all stays). Save to keep the change.")
                .clicked()
            {
                self.apply_row_action(Some(RowAction::Delete(self.selected)));
            }
            if ui
                .button("Add default profile")
                .on_hover_text(
                    "Adds the default Firm rules as a new profile; your profiles are not changed. Save to keep it.",
                )
                .clicked()
            {
                self.add_default_profile();
            }
        });
        ui.label(
            RichText::new("Drag the handle to reorder.")
                .size(12.0)
                .color(p.muted),
        );
    }

    #[cfg(test)]
    pub fn draft_profiles(&self) -> Vec<String> {
        self.draft.profiles.keys().cloned().collect()
    }

    /// V11: the default Firm rules as another profile, named "Firm" or "Firm 2"…
    pub fn add_default_profile(&mut self) {
        let name = (1..)
            .map(|n| {
                if n == 1 {
                    rules::DEFAULT_PROFILE.to_string()
                } else {
                    format!("{} {n}", rules::DEFAULT_PROFILE)
                }
            })
            .find(|n| !self.draft.profiles.contains_key(n))
            .expect("unbounded");
        self.draft
            .profiles
            .insert(name.clone(), rules::firm_rules());
        self.draft.active = name;
        self.selected = 0;
    }

    fn new_profile(&mut self) {
        let name = (1..)
            .map(|n| {
                if n == 1 {
                    "New profile".to_string()
                } else {
                    format!("New profile {n}")
                }
            })
            .find(|n| !self.draft.profiles.contains_key(n))
            .expect("unbounded");
        let copy: Vec<Rule> = self
            .draft
            .active_rules()
            .iter()
            .map(|r| Rule {
                id: uuid::Uuid::new_v4().to_string(),
                ..r.clone()
            })
            .collect();
        self.draft.profiles.insert(name.clone(), copy);
        self.draft.active = name;
        self.selected = 0;
    }

    fn apply_row_action(&mut self, action: Option<RowAction>) {
        let Some(action) = action else { return };
        let rules = self.draft.active_rules_mut();
        let selected_id = rules.get(self.selected).map(|r| r.id.clone());
        match action {
            RowAction::Select(i) => {
                self.selected = i;
                return;
            }
            RowAction::Toggle(i) => {
                if let Some(r) = rules.get_mut(i)
                    && !r.catch_all
                {
                    r.enabled = !r.enabled;
                }
                return;
            }
            RowAction::Move(from, to) => rules::move_rule(rules, from, to),
            RowAction::Delete(i) => {
                if rules.get(i).is_none_or(|r| r.catch_all) {
                    return;
                }
                rules.remove(i);
                self.selected = self.selected.min(rules.len().saturating_sub(1));
                return;
            }
            RowAction::Duplicate(i) => {
                if let Some(r) = rules.get(i).filter(|r| !r.catch_all).cloned() {
                    let copy = Rule {
                        id: uuid::Uuid::new_v4().to_string(),
                        name: format!("{} copy", r.name),
                        ..r
                    };
                    rules.insert(i + 1, copy);
                    self.selected = i + 1;
                }
                return;
            }
        }
        if let Some(id) = selected_id {
            self.selected = rules.iter().position(|r| r.id == id).unwrap_or(0);
        }
    }

    fn form_ui(
        &mut self,
        ui: &mut egui::Ui,
        state: &mut AppState,
        rule_errors: &[(usize, String)],
        extra_tokens: &[String],
    ) {
        let p = pal(ui);
        let selected = self.selected;
        if let Some(rule) = self.draft.active_rules_mut().get_mut(selected) {
            let title = if rule.catch_all {
                "Catch-all rule".to_string()
            } else {
                format!("Edit rule — {}", rule.name)
            };
            ui.label(strong(title));
            ui.add_space(6.0);
            let valid = rules::validate_rule(rule, extra_tokens)
                .iter()
                .all(|e| !e.to_lowercase().contains("pattern"));
            ui.columns(2, |cols| {
                field(&mut cols[0], "Name", "", |ui, id| {
                    ui.add_enabled(
                        !rule.catch_all,
                        egui::TextEdit::singleline(&mut rule.name).desired_width(f32::INFINITY),
                    )
                    .labelled_by(id);
                });
                field(&mut cols[1], "Pattern", "(regex on rest)", |ui, id| {
                    ui.horizontal(|ui| {
                        ui.add_enabled(
                            !rule.catch_all,
                            egui::TextEdit::singleline(&mut rule.pattern)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(ui.available_width() - 60.0),
                        )
                        .labelled_by(id)
                        .on_hover_text(
                            "Regular expression, case-insensitive, tested against the part of \
                             the filename after the ID.",
                        );
                        if valid {
                            pill(ui, PillKind::Ok, "valid");
                        } else {
                            pill(ui, PillKind::Err, "invalid");
                        }
                    });
                });
                field(
                    &mut cols[0],
                    "Except when",
                    "(optional regex: no match if this matches)",
                    |ui, id| {
                        ui.add_enabled(
                            !rule.catch_all,
                            egui::TextEdit::singleline(&mut rule.exclude)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY),
                        )
                        .labelled_by(id);
                    },
                );
                cols[1].add_space(0.0);
                field(&mut cols[0], "Primary destination", "(moved)", |ui, id| {
                    ui.add(
                        egui::TextEdit::singleline(&mut rule.dest_template)
                            .desired_width(f32::INFINITY),
                    )
                    .labelled_by(id);
                });
                field(&mut cols[1], "Filename template", "", |ui, id| {
                    ui.add(
                        egui::TextEdit::singleline(&mut rule.filename_template)
                            .desired_width(f32::INFINITY),
                    )
                    .labelled_by(id);
                });
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut rule.requires_year, "Requires year");
                ui.checkbox(&mut rule.next_year_folder, "Also create next year's folder")
                    .on_hover_text(
                        "Filing into a year folder also creates the following year's empty folder \
                         next to it (removed by Undo if still empty).",
                    );
                ui.add_enabled(
                    !rule.catch_all,
                    egui::Checkbox::new(&mut rule.enabled, "Enabled"),
                );
                ui.add_enabled(
                    !rule.catch_all,
                    egui::Checkbox::new(&mut rule.whole_words, "Whole words only"),
                )
                .on_hover_text(
                    "Matches only where the pattern is a separate word. Spaces, _ and punctuation \
                     count as separators, so k-?1 matches \"2025_K-1\" but not \"K-10\".",
                );
                ui.add_enabled(
                    !rule.catch_all,
                    egui::Checkbox::new(&mut rule.ultratax_only, "Only UltraTax file names"),
                )
                .on_hover_text(
                    "Matches only files whose name was read with the UltraTax layout (Setup). \
                     The pattern is then tried on \"{EntityType} {Document}\" plus the K-1 \
                     recipient.",
                );
            });
            if !rule.catch_all {
                copies_ui(ui, rule, &mut self.roles_edit);
            }
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                label_caps(ui, "Tokens");
                for t in BUILTIN_TOKENS {
                    token_pill(ui, PillKind::Neutral, format!("{{{t}}}"));
                }
                for t in extra_tokens {
                    token_pill(ui, PillKind::Info, format!("{{{t}}}"));
                }
                if !rule.copies.is_empty() {
                    ui.label(RichText::new("copies also:").size(11.0).color(p.muted));
                    for t in rules::copy_tokens(extra_tokens) {
                        token_pill(ui, PillKind::Info, format!("{{{t}}}"));
                    }
                }
            });
            for (_, e) in rule_errors.iter().filter(|(i, _)| *i == selected) {
                banner(ui, Level::Error, e);
            }
        }
        ui.add_space(6.0);
        let errors = self.errors(state);
        for e in errors
            .iter()
            .filter(|e| !rule_errors.iter().any(|(_, r)| e.contains(r.as_str())))
        {
            banner(ui, Level::Error, e);
        }
        let dirty = self.is_dirty(state) || !state.rules_savable;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(dirty && errors.is_empty(), primary_button(ui, "Save"))
                .clicked()
            {
                self.save(state);
            }
            if ui.add_enabled(dirty, egui::Button::new("Revert")).clicked() {
                self.draft = state.rules.clone();
                self.selected = 0;
                self.message = None;
            }
            if dirty {
                ui.label(RichText::new("Not saved yet").color(p.muted));
            }
        });
        if let Some((level, msg)) = &self.message {
            ui.add_space(4.0);
            banner(ui, *level, msg);
        }
    }

    /// V10: words from the test name fill a guided form; the form writes the regex; Create rule
    /// adds it above Catch-all (saved with Save).
    fn builder_ui(&mut self, ui: &mut egui::Ui, state: &AppState) {
        let p = pal(ui);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.label(strong("Build a rule"));
            ui.label(RichText::new(" · click words of the test name, or type them").color(p.muted));
        });
        ui.add_space(4.0);
        let rest = self.parsed_test(state).rest;
        let b = &mut self.builder;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Click adds to").color(p.muted));
            theme::segmented(
                ui,
                &mut b.target,
                &[
                    (Target::Any, "Any of"),
                    (Target::All, "All of"),
                    (Target::None, "Not"),
                    (Target::Starts, "Starts with"),
                    (Target::Ends, "Ends with"),
                ],
            );
        });
        let mut add: Option<String> = None;
        if self.test_input.trim().is_empty() {
            ui.label(
                RichText::new("Type a file name in Test a filename above to get its words here.")
                    .size(12.0)
                    .color(p.muted),
            );
        } else {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                for w in rulebuild::words_in(&rest) {
                    let resp = ui.button(mono(&w));
                    let name = format!("Word: {w}");
                    resp.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::Button, true, name.clone())
                    });
                    if resp.clicked() {
                        add = Some(w);
                    }
                }
            });
        }
        ui.horizontal(|ui| {
            let label = ui.label("Word or phrase");
            let resp = ui
                .add(egui::TextEdit::singleline(&mut b.typed).desired_width(200.0))
                .labelled_by(label.id);
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.button("Add").clicked() || enter) && !b.typed.trim().is_empty() {
                add = Some(b.typed.trim().to_string());
                b.typed.clear();
            }
        });
        if let Some(w) = add {
            let list = match b.target {
                Target::Any => Some(&mut b.form.any_of),
                Target::All => Some(&mut b.form.all_of),
                Target::None => Some(&mut b.form.none_of),
                Target::Starts => {
                    b.form.starts_with = w.clone();
                    None
                }
                Target::Ends => {
                    b.form.ends_with = w.clone();
                    None
                }
            };
            if let Some(list) = list
                && !list.iter().any(|x| x.eq_ignore_ascii_case(&w))
            {
                list.push(w);
            }
            if b.name.trim().is_empty() || b.target != Target::None {
                let suggested = b.form.suggested_name();
                if !suggested.is_empty() && b.name.trim().is_empty() {
                    b.name = suggested;
                }
            }
        }
        ui.add_space(4.0);
        egui::Grid::new("builder_form")
            .num_columns(2)
            .spacing([16.0, 6.0])
            .show(ui, |ui| {
                for (title, list) in [
                    ("Contains any of", &mut b.form.any_of),
                    ("Contains all of", &mut b.form.all_of),
                    ("Does not contain", &mut b.form.none_of),
                ] {
                    label_caps(ui, title);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        if list.is_empty() {
                            ui.label(RichText::new("—").color(p.muted));
                        }
                        let mut remove = None;
                        for (i, w) in list.iter().enumerate() {
                            ui.label(mono(w));
                            let x = ui.small_button("×");
                            let name = format!("Remove {w} from {title}");
                            x.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::Role::Button, true, name.clone())
                            });
                            if x.clicked() {
                                remove = Some(i);
                            }
                        }
                        if let Some(i) = remove {
                            list.remove(i);
                        }
                    });
                    ui.end_row();
                }
                for (title, text) in [
                    ("Starts with", &mut b.form.starts_with),
                    ("Ends with", &mut b.form.ends_with),
                ] {
                    let l = label_caps(ui, title);
                    ui.add(egui::TextEdit::singleline(text).desired_width(200.0))
                        .labelled_by(l.id);
                    ui.end_row();
                }
                label_caps(ui, "Numbers");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut b.form.any_year, "any year (2024 → any 4-digit year)");
                    ui.checkbox(&mut b.form.any_number, "any number");
                });
                ui.end_row();
            });
        ui.add_space(4.0);
        let built = b.form.build();
        ui.checkbox(&mut b.by_hand, "Edit regex by hand");
        if b.by_hand {
            if b.hand_pattern.is_empty()
                && let Ok(x) = &built
            {
                b.hand_pattern = x.pattern.clone();
                b.hand_exclude = x.exclude.clone();
            }
            field(ui, "Pattern", "(regex on rest)", |ui, id| {
                ui.add(
                    egui::TextEdit::singleline(&mut b.hand_pattern)
                        .font(egui::TextStyle::Monospace)
                        .desired_width(f32::INFINITY),
                )
                .labelled_by(id);
            });
            field(ui, "Except when", "(optional)", |ui, id| {
                ui.add(
                    egui::TextEdit::singleline(&mut b.hand_exclude)
                        .font(egui::TextStyle::Monospace)
                        .desired_width(f32::INFINITY),
                )
                .labelled_by(id);
            });
        } else {
            b.hand_pattern.clear();
            b.hand_exclude.clear();
        }
        let result: Result<rulebuild::Built, String> = if b.by_hand {
            if b.hand_pattern.trim().is_empty() {
                Err("Type a pattern.".into())
            } else {
                Ok(rulebuild::Built {
                    pattern: b.hand_pattern.trim().to_string(),
                    exclude: b.hand_exclude.trim().to_string(),
                })
            }
        } else {
            built
        };
        let compiled = result.and_then(|x| {
            let re = rules::compile_pattern(&x.pattern).map_err(|e| format!("Pattern: {e}"))?;
            let ex = if x.exclude.is_empty() {
                None
            } else {
                Some(rules::compile_pattern(&x.exclude).map_err(|e| format!("Except when: {e}"))?)
            };
            Ok((x, re, ex))
        });
        match &compiled {
            Err(e) => {
                ui.label(RichText::new(e).size(12.0).color(p.muted));
            }
            Ok((x, re, ex)) => {
                if !b.by_hand {
                    egui::Grid::new("builder_regex")
                        .num_columns(2)
                        .spacing([16.0, 4.0])
                        .show(ui, |ui| {
                            label_caps(ui, "Regex");
                            ui.add(egui::Label::new(mono(&x.pattern)).wrap());
                            ui.end_row();
                            if !x.exclude.is_empty() {
                                label_caps(ui, "Except when");
                                ui.add(egui::Label::new(mono(&x.exclude)).wrap());
                                ui.end_row();
                            }
                        });
                }
                let hits = |rest: &str| {
                    re.is_match(rest) && !ex.as_ref().is_some_and(|e| e.is_match(rest))
                };
                let mut line = Vec::new();
                if !self.test_input.trim().is_empty() {
                    line.push(if hits(&rest) {
                        "The test name matches.".to_string()
                    } else {
                        "The test name does not match.".to_string()
                    });
                }
                let queue = state.intake.queue();
                if !queue.is_empty() {
                    let rests = queue_rests(&mut self.queue_rests, state);
                    let names: Vec<&String> = rests
                        .iter()
                        .filter(|(_, rest)| hits(rest))
                        .map(|(name, _)| name)
                        .collect();
                    let mut t = format!("Matches {} of {} queued files", names.len(), queue.len());
                    if !names.is_empty() {
                        let shown: Vec<&str> = names.iter().take(4).map(|n| n.as_str()).collect();
                        t.push_str(&format!(": {}", shown.join(", ")));
                        if names.len() > 4 {
                            t.push_str(&format!(" +{} more", names.len() - 4));
                        }
                    }
                    line.push(t);
                }
                if !line.is_empty() {
                    ui.label(RichText::new(line.join(" · ")).size(12.0).color(p.text));
                }
            }
        }
        ui.add_space(6.0);
        let folders = crate::template::folder_templates(&state.template);
        if b.dest.is_empty() {
            b.dest = folders
                .iter()
                .find(|f| f.contains("{Year}"))
                .or(folders.first())
                .cloned()
                .unwrap_or_default();
        }
        egui::Grid::new("builder_out")
            .num_columns(2)
            .spacing([16.0, 6.0])
            .show(ui, |ui| {
                let l = label_caps(ui, "Rule name");
                ui.add(egui::TextEdit::singleline(&mut b.name).desired_width(240.0))
                    .labelled_by(l.id);
                ui.end_row();
                let l = label_caps(ui, "Folder");
                let combo = egui::ComboBox::from_id_salt("builder_dest")
                    .selected_text(mono(format!("{{Client}}/{}", b.dest)))
                    .width(320.0)
                    .show_ui(ui, |ui| {
                        for f in &folders {
                            ui.selectable_value(&mut b.dest, f.clone(), mono(f));
                        }
                    });
                combo.response.labelled_by(l.id);
                ui.end_row();
                let l = label_caps(ui, "File name");
                let combo = egui::ComboBox::from_id_salt("builder_name")
                    .selected_text(mono(&b.filename))
                    .width(320.0)
                    .show_ui(ui, |ui| {
                        for (t, what) in [
                            ("{Year} {DocType}.{Ext}", "year and rule name"),
                            ("{Original}.{Ext}", "keep the original name"),
                            ("{Rest}.{Ext}", "the part after the ID"),
                        ] {
                            ui.selectable_value(
                                &mut b.filename,
                                t.to_string(),
                                format!("{t}  ({what})"),
                            );
                        }
                    });
                combo.response.labelled_by(l.id);
                ui.end_row();
            });
        ui.add_space(6.0);
        let ready = compiled.is_ok() && !b.name.trim().is_empty() && !b.dest.is_empty();
        let create = ui
            .add_enabled(ready, theme::primary_button(ui, "Create rule"))
            .on_disabled_hover_text("Pick words (or a pattern), a rule name and a folder.");
        if create.clicked()
            && let Ok((x, _, _)) = compiled
        {
            let dest = format!("{{Client}}/{}", b.dest);
            let requires_year = dest.contains("{Year}") || b.filename.contains("{Year}");
            let rule = Rule {
                exclude: x.exclude.clone(),
                ..Rule::new(b.name.trim(), &x.pattern, &dest, &b.filename, requires_year)
            };
            let rules = self.draft.active_rules_mut();
            let at = rules
                .iter()
                .position(|r| r.catch_all)
                .unwrap_or(rules.len());
            rules.insert(at, rule);
            self.selected = at;
            self.message = Some((
                Level::Info,
                format!(
                    "Rule \"{}\" added above Catch-all. Check it in Edit rule, then Save.",
                    self.builder.name.trim()
                ),
            ));
            self.builder = Builder {
                filename: self.builder.filename.clone(),
                dest: self.builder.dest.clone(),
                ..Default::default()
            };
        }
    }

    fn test_ui(&mut self, ui: &mut egui::Ui, state: &AppState) {
        let p = pal(ui);
        let title = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let t = ui.label(strong("Test a filename"));
            ui.label(RichText::new(" · runs the whole profile in order").color(p.muted));
            t
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.test_input)
                    .desired_width(ui.available_width() - 70.0),
            )
            .labelled_by(title.inner.id);
            ui.add_enabled(false, egui::Button::new("Test"))
                .on_disabled_hover_text("Results update as you type.");
        });
        let input = self.test_input.trim();
        if input.is_empty() {
            return;
        }
        ui.add_space(6.0);
        let parsed = self.parsed_test(state);
        let result = match &self.checked(state).compiled {
            Ok(compiled) => Ok(test_result(&self.draft, compiled, parsed, state)),
            Err(e) => Err(format!("Fix the rule errors first ({e}).")),
        };
        match result {
            Err(e) => banner(ui, Level::Error, &e),
            Ok(t) => {
                egui::Grid::new("rules_test_result")
                    .num_columns(2)
                    .spacing([24.0, 6.0])
                    .show(ui, |ui| {
                        label_caps(ui, "Parsed");
                        ui.label(parsed_job(ui, &t.parsed));
                        ui.end_row();
                        if let Some(u) = &t.parsed.ultratax {
                            label_caps(ui, "UltraTax");
                            ui.label(RichText::new(ultratax_summary(u)).size(12.0).color(p.text));
                            ui.end_row();
                        }
                        label_caps(ui, "Matched rule");
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            ui.label(strong(&t.rule));
                            ui.label(
                                RichText::new(format!("({} tried)", ordinal(t.tried)))
                                    .size(12.0)
                                    .color(p.muted),
                            );
                        });
                        ui.end_row();
                        if !t.copies.is_empty() {
                            label_caps(ui, "Copies");
                            ui.vertical(|ui| {
                                for c in &t.copies {
                                    ui.label(RichText::new(c).size(12.0).color(p.text));
                                }
                            });
                            ui.end_row();
                        }
                        label_caps(ui, "Primary");
                        ui.vertical(|ui| {
                            if let Some(n) = &t.note {
                                ui.label(RichText::new(n).size(12.0).color(p.muted));
                            }
                            match &t.primary {
                                Ok((dir, file)) => {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.spacing_mut().item_spacing.x = 0.0;
                                        ui.label(mono(format!("…\\{dir}\\")).color(p.text));
                                        ui.label(theme::mono_strong(file).color(p.text));
                                    });
                                    ui.label(
                                        RichText::new(format!("for {}", t.client))
                                            .size(11.0)
                                            .color(p.muted),
                                    );
                                }
                                Err(e) => {
                                    ui.label(RichText::new(e).color(p.pill(PillKind::Err).1));
                                }
                            }
                        });
                        ui.end_row();
                    });
            }
        }
    }
}

/// The Copies section of the edit card (UI_SPEC §7).
fn copies_ui(ui: &mut egui::Ui, rule: &mut Rule, roles_edit: &mut Option<(usize, String)>) {
    let p = pal(ui);
    ui.add_space(6.0);
    ui.separator();
    ui.horizontal(|ui| {
        label_caps(ui, "Copies");
        ui.label(
            RichText::new("real byte copies, written and hash-verified before the primary move")
                .size(12.0)
                .color(p.muted),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.button("+ Add copy").clicked() {
                rule.copies.push(CopyDest::new(Resolver::Manual));
            }
        });
    });
    let mut remove = None;
    for (k, copy) in rule.copies.iter_mut().enumerate() {
        Frame::new()
            .fill(p.tint_copy)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(Margin::same(8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let total = ui.available_width() - 4.0 * 8.0;
                let w = [170.0, (total - 170.0 - 110.0 - 24.0) / 2.0, (total - 170.0 - 110.0 - 24.0) / 2.0, 110.0, 24.0];
                ui.horizontal(|ui| {
                    for (width, t) in w.iter().zip(["Resolver", "Destination", "Filename", "On split", ""]) {
                        fixed(ui, *width, |ui| {
                            label_caps(ui, t);
                        });
                    }
                });
                ui.horizontal(|ui| {
                    fixed(ui, w[0], |ui| {
                        let mut kind = std::mem::discriminant(&copy.resolver);
                        let options = [
                            Resolver::RelatedByName { group: "who".into() },
                            Resolver::Related { roles: Vec::new() },
                            Resolver::Capture { group: "id2".into() },
                            Resolver::Manual,
                        ];
                        let resp = egui::ComboBox::from_id_salt(("copy_resolver", k))
                            .width(w[0] - 8.0)
                            .selected_text(copy.resolver.label())
                            .show_ui(ui, |ui| {
                                for o in &options {
                                    let d = std::mem::discriminant(o);
                                    if ui.selectable_label(kind == d, o.label()).clicked() {
                                        kind = d;
                                        if std::mem::discriminant(&copy.resolver) != d {
                                            copy.resolver = o.clone();
                                        }
                                    }
                                }
                            });
                        let name = format!("Copy {} resolver", k + 1);
                        resp.response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, name.clone()));
                    });
                    fixed(ui, w[1], |ui| {
                        let e = ui.add(egui::TextEdit::singleline(&mut copy.dest_template).desired_width(w[1] - 8.0));
                        let name = format!("Copy {} destination", k + 1);
                        e.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, name.clone()));
                    });
                    fixed(ui, w[2], |ui| {
                        let e = ui.add(egui::TextEdit::singleline(&mut copy.filename_template).desired_width(w[2] - 8.0));
                        let name = format!("Copy {} filename", k + 1);
                        e.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, name.clone()));
                    });
                    fixed(ui, w[3], |ui| {
                        egui::ComboBox::from_id_salt(("copy_split", k))
                            .width(w[3] - 8.0)
                            .selected_text(match copy.on_split {
                                OnSplit::Segment => "Segment",
                                OnSplit::Whole => "Whole",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut copy.on_split, OnSplit::Segment, "Segment");
                                ui.selectable_value(&mut copy.on_split, OnSplit::Whole, "Whole");
                            });
                    });
                    fixed(ui, w[4], |ui| {
                        let x = ui.small_button("×").on_hover_text("Remove this copy");
                        let name = format!("Remove copy {}", k + 1);
                        x.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name.clone()));
                        if x.clicked() {
                            remove = Some(k);
                        }
                    });
                });
                ui.horizontal_wrapped(|ui| match &mut copy.resolver {
                    Resolver::RelatedByName { group } => {
                        let l = label_caps(ui, "Group");
                        ui.add(egui::TextEdit::singleline(group).desired_width(90.0)).labelled_by(l.id);
                        ui.label(
                            RichText::new("matched against this client's relationships first, then the master list")
                                .size(11.0)
                                .color(p.muted),
                        );
                    }
                    Resolver::Capture { group } => {
                        let l = label_caps(ui, "Group");
                        ui.add(egui::TextEdit::singleline(group).desired_width(90.0)).labelled_by(l.id);
                        ui.label(
                            RichText::new("a second client ID captured from the file name")
                                .size(11.0)
                                .color(p.muted),
                        );
                    }
                    Resolver::Related { roles } => {
                        label_caps(ui, "Roles");
                        match roles_edit {
                            Some((i, text)) if *i == k => {
                                let e = ui.add(egui::TextEdit::singleline(text).hint_text("shareholder, partner").desired_width(200.0));
                                e.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Roles"));
                                if !e.has_focus() && !e.lost_focus() {
                                    e.request_focus();
                                }
                                if e.lost_focus() {
                                    *roles = text
                                        .split(',')
                                        .map(|r| r.trim().to_string())
                                        .filter(|r| !r.is_empty())
                                        .collect();
                                    *roles_edit = None;
                                }
                            }
                            _ => {
                                for r in roles.iter() {
                                    pill(ui, PillKind::Info, r);
                                }
                                if ui.small_button("edit…").clicked() {
                                    *roles_edit = Some((k, roles.join(", ")));
                                }
                            }
                        }
                        ui.label(
                            RichText::new("blank = every child of the primary client")
                                .size(11.0)
                                .color(p.muted),
                        );
                    }
                    Resolver::Manual => {
                        ui.label(RichText::new("the copy client is picked in Preview").size(11.0).color(p.muted));
                    }
                });
            });
    }
    if let Some(k) = remove {
        rule.copies.remove(k);
        *roles_edit = None;
    }
}

/// The list's caps header: ON · RULE · PATTERN ON REST.
fn header_row(ui: &mut egui::Ui) {
    let p = pal(ui);
    Frame::new()
        .fill(p.tint_copy)
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add_space(22.0);
                fixed(ui, 44.0, |ui| {
                    label_caps(ui, "On");
                });
                fixed(ui, 140.0, |ui| {
                    label_caps(ui, "Rule");
                });
                label_caps(ui, "Pattern on rest");
            });
        });
}

/// One rule row: drag handle, switch, name, pattern. Right-click for Move / Duplicate / Delete.
fn rule_row(
    ui: &mut egui::Ui,
    i: usize,
    count: usize,
    rule: &Rule,
    selected: bool,
    has_error: bool,
) -> Option<RowAction> {
    let p = pal(ui);
    let mut action = None;
    // Name and pattern take the click and the right-click menu (a row-wide click area would sit
    // on top of the switch and the drag handle).
    let mut clicks: Vec<egui::Response> = Vec::new();
    let row = Frame::new()
        .fill(if selected {
            p.accent.gamma_multiply(0.08)
        } else {
            egui::Color32::TRANSPARENT
        })
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.dnd_drag_source(egui::Id::unique(("rule_drag", &rule.id)), i, |ui| {
                    drag_handle(ui);
                })
                .response
                .on_hover_text("Drag to reorder");
                ui.add_space(6.0);
                fixed(ui, 44.0, |ui| {
                    let mut on = rule.enabled;
                    if theme::switch(ui, &mut on, &format!("Enabled: {}", rule.name)).changed() {
                        action = Some(RowAction::Toggle(i));
                    }
                });
                let dim = |c: egui::Color32| {
                    if rule.enabled {
                        c
                    } else {
                        c.gamma_multiply(0.5)
                    }
                };
                fixed(ui, 140.0, |ui| {
                    let name = ui.add(
                        egui::Label::new(strong(&rule.name).color(dim(p.text)))
                            .truncate()
                            .sense(egui::Sense::click()),
                    );
                    if name.clicked() {
                        action = Some(RowAction::Select(i));
                    }
                    clicks.push(name);
                    if !rule.copies.is_empty() {
                        pill(ui, PillKind::Info, format!("+{} copy", rule.copies.len()));
                    }
                    if has_error {
                        pill(ui, PillKind::Err, "error");
                    }
                });
                let pattern = ui.add(
                    egui::Label::new(mono(&rule.pattern).color(dim(p.muted)))
                        .truncate()
                        .sense(egui::Sense::click()),
                );
                if pattern.clicked() {
                    action = Some(RowAction::Select(i));
                }
                clicks.push(pattern);
            });
        });
    let menu = |ui: &mut egui::Ui, action: &mut Option<RowAction>| {
        if ui
            .add_enabled(i > 0, egui::Button::new("Move up"))
            .clicked()
        {
            *action = Some(RowAction::Move(i, i - 1));
            ui.close();
        }
        if ui
            .add_enabled(i + 2 < count, egui::Button::new("Move down"))
            .clicked()
        {
            *action = Some(RowAction::Move(i, i + 1));
            ui.close();
        }
        if ui.button("Duplicate").clicked() {
            *action = Some(RowAction::Duplicate(i));
            ui.close();
        }
        if ui.button("Delete").clicked() {
            *action = Some(RowAction::Delete(i));
            ui.close();
        }
    };
    for c in &clicks {
        c.context_menu(|ui| menu(ui, &mut action));
    }
    if row.response.dnd_hover_payload::<usize>().is_some() {
        ui.painter().hline(
            row.response.rect.x_range(),
            row.response.rect.top(),
            Stroke::new(2.0, p.accent),
        );
    }
    if let Some(from) = row.response.dnd_release_payload::<usize>() {
        action = Some(RowAction::Move(*from, i));
    }
    ui.painter().hline(
        row.response.rect.x_range(),
        row.response.rect.bottom(),
        Stroke::new(1.0, p.border.gamma_multiply(0.6)),
    );
    action
}

/// The catch-all row: lock, name, `.*`, "always last".
fn catch_all_row(ui: &mut egui::Ui, i: usize, rule: &Rule, selected: bool) -> Option<RowAction> {
    let p = pal(ui);
    let row = Frame::new()
        .fill(if selected {
            p.accent.gamma_multiply(0.08)
        } else {
            egui::Color32::TRANSPARENT
        })
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                lock_icon(ui, p.muted);
                ui.add_space(50.0);
                fixed(ui, 140.0, |ui| {
                    ui.label(strong(&rule.name));
                });
                ui.label(mono(".*").color(p.muted));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    pill(ui, PillKind::Neutral, "always last");
                });
            });
        });
    let resp = ui.interact(
        row.response.rect,
        egui::Id::unique("catch_all_row"),
        egui::Sense::click(),
    );
    resp.clicked().then_some(RowAction::Select(i))
}

/// A fixed-width slot in a horizontal row.
fn fixed(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 22.0),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_width(width);
            add(ui);
        },
    );
}

/// Six dots, the drag handle.
fn drag_handle(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 16.0), egui::Sense::drag());
    let c = pal(ui).muted;
    for col in 0..2 {
        for row in 0..3 {
            let pos = egui::pos2(
                rect.left() + 2.5 + col as f32 * 5.0,
                rect.top() + 3.0 + row as f32 * 5.0,
            );
            ui.painter().circle_filled(pos, 1.2, c);
        }
    }
}

fn lock_icon(ui: &mut egui::Ui, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 14.0), egui::Sense::hover());
    let body = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + 6.0),
        egui::pos2(rect.right(), rect.bottom() - 1.0),
    );
    ui.painter().rect_stroke(
        body,
        CornerRadius::same(1),
        Stroke::new(1.2, color),
        egui::StrokeKind::Inside,
    );
    let c = egui::pos2(rect.center().x, rect.top() + 6.0);
    ui.painter().add(egui::Shape::line(
        (0..=8)
            .map(|k| {
                let a = std::f32::consts::PI * (k as f32 / 8.0);
                egui::pos2(c.x - 3.0 * a.cos(), c.y - 4.0 * a.sin())
            })
            .collect(),
        Stroke::new(1.2, color),
    ));
}

/// An uppercase label (with an optional muted note) above a field.
fn field(ui: &mut egui::Ui, label: &str, note: &str, add: impl FnOnce(&mut egui::Ui, egui::Id)) {
    let p = pal(ui);
    let id = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let l = label_caps(ui, label);
            if !note.is_empty() {
                ui.label(RichText::new(note).size(11.0).color(p.muted));
            }
            l.id
        })
        .inner;
    add(ui, id);
    ui.add_space(6.0);
}

fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (1, 11) | (2, 12) | (3, 13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// `name=… · id=… · rest=… · ext=… · year=…` with the values in bold monospace.
fn parsed_job(ui: &egui::Ui, parsed: &Parsed) -> egui::text::LayoutJob {
    let p = pal(ui);
    let plain = egui::TextStyle::Monospace.resolve(ui.style());
    let bold = egui::FontId::new(
        plain.size,
        egui::FontFamily::Name(theme::MONO_SEMIBOLD.into()),
    );
    let mut job = egui::text::LayoutJob::default();
    let year = parsed
        .year
        .map(|y| y.to_string())
        .unwrap_or_else(|| "—".into());
    let parts = [
        (
            "name=",
            if parsed.name.is_empty() {
                "—"
            } else {
                parsed.name.as_str()
            },
        ),
        ("id=", parsed.id.as_deref().unwrap_or("—")),
        ("rest=", parsed.rest.as_str()),
        ("ext=", parsed.ext.as_str()),
        ("year=", year.as_str()),
    ];
    for (k, (key, value)) in parts.iter().enumerate() {
        if k > 0 {
            job.append(" · ", 0.0, egui::TextFormat::simple(plain.clone(), p.muted));
        }
        job.append(key, 0.0, egui::TextFormat::simple(plain.clone(), p.muted));
        job.append(value, 0.0, egui::TextFormat::simple(bold.clone(), p.text));
    }
    job
}

struct TestResult {
    parsed: Parsed,
    rule: String,
    /// 1-based position of the matched rule among the enabled rules.
    tried: usize,
    note: Option<String>,
    /// (destination folder relative to the client root, file name), or why it can't be built.
    primary: Result<(String, String), String>,
    client: String,
    /// One line per copy: how its client is found (resolved for real in Preview).
    copies: Vec<String>,
}

/// The UltraTax parts the test box read, on one line.
fn ultratax_summary(u: &parse::UltraTaxParts) -> String {
    [
        ("Client", &u.client_name),
        ("ID", &u.client_id),
        ("Year", &u.tax_year),
        ("Entity", &u.entity_type),
        ("Document", &u.document),
        ("K-1 recipient", &u.k1_recipient),
        ("Year end", &u.year_end),
        ("Printed", &u.date_printed),
    ]
    .iter()
    .filter(|(_, v)| !v.is_empty())
    .map(|(k, v)| format!("{k} {v}"))
    .collect::<Vec<_>>()
    .join(" · ")
}

/// The queued files' names and the text rules match on, read again only when the queue or the
/// setup changed.
fn queue_rests<'a>(
    cache: &'a mut Option<(u64, Vec<(String, String)>)>,
    state: &AppState,
) -> &'a [(String, String)] {
    use std::hash::{Hash, Hasher};
    let queue = state.intake.queue();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for q in queue {
        q.path.hash(&mut h);
    }
    name_setup_key(state, &mut h);
    let key = h.finish();
    if cache.as_ref().is_none_or(|(k, _)| *k != key) {
        let re = parse::filename_regex(&state.settings.id_pattern).ok();
        let rests = queue
            .iter()
            .map(|q| {
                let name = theme::file_name(&q.path);
                let rest = test_parsed_with(state, re.as_ref(), &name).rest;
                (name, rest)
            })
            .collect();
        *cache = Some((key, rests));
    }
    &cache.as_ref().expect("set above").1
}

/// `input` read like an inbox file name (UltraTax layout or `Name_ID_rest`), or taken as the
/// remainder when it is neither.
fn test_parsed(state: &AppState, input: &str) -> Parsed {
    let re = parse::filename_regex(&state.settings.id_pattern).ok();
    test_parsed_with(state, re.as_ref(), input)
}

/// [`test_parsed`] with the filename regex compiled once by the caller.
fn test_parsed_with(state: &AppState, re: Option<&regex::Regex>, input: &str) -> Parsed {
    let current_year = chrono::Datelike::year(&chrono::Local::now());
    let parsed = crate::plan::read_name(
        input,
        re,
        &state.settings.ultratax,
        current_year,
        state.clients.list().map(|l| l.as_ref()),
        state.settings.fuzzy_suggest,
    );
    if parsed.id.is_some() {
        return parsed;
    }
    Parsed {
        name: String::new(),
        id: None,
        rest: input.to_string(),
        ext: "pdf".into(),
        year: parse::detect_year(input, current_year),
        stem: input.to_string(),
        ultratax: None,
    }
}

/// What the active profile (`compiled` from `draft`) would do with the test name, `parsed`.
fn test_result(
    draft: &RulesFile,
    compiled: &CompiledRules,
    parsed: Parsed,
    state: &AppState,
) -> TestResult {
    let current_year = chrono::Datelike::year(&chrono::Local::now());
    let list = state.clients.list();
    let client = parsed
        .id
        .as_deref()
        .and_then(|id| list.and_then(|l| l.match_by_id(id)))
        .or_else(|| list.and_then(|l| l.clients.first()))
        .cloned()
        .unwrap_or_else(|| sample_client(&clients::extra_tokens(&state.settings.column_map)));

    let rule = compiled.first_match(&parsed.rest, parsed.ultratax.is_some());
    let tried = draft
        .active_rules()
        .iter()
        .filter(|r| r.enabled || r.catch_all)
        .position(|r| r.id == rule.id)
        .map_or(1, |i| i + 1);
    let (year, note) = match parsed.year {
        Some(y) => (Some(y), None),
        None if rule.requires_year => (
            Some(current_year as u16),
            Some(format!(
                "No year found; Preview will ask. With {current_year}:"
            )),
        ),
        None => (None, None),
    };
    let primary = match rules::evaluate_rule(rule, &parsed, &client, year).outcome {
        Ok(r) => {
            let folder = template::client_folder_name(&state.template, &client)
                .unwrap_or_else(|_| client.name.clone());
            let dir = std::path::Path::new(&folder).join(&r.dest_rel);
            Ok((dir.display().to_string(), r.filename))
        }
        Err(HitProblem::NeedsYear) => Err("Needs a year.".into()),
        Err(HitProblem::Render(e)) => Err(format!("Cannot build the path: {e}.")),
    };
    let captures = regex::RegexBuilder::new(&rules::effective_pattern(rule))
        .case_insensitive(true)
        .build()
        .ok();
    let group = |g: &str| {
        captures
            .as_ref()
            .and_then(|re| re.captures(&parsed.rest))
            .and_then(|c| c.name(g).map(|m| m.as_str().trim().to_string()))
    };
    let copies = rule
        .copies
        .iter()
        .map(|c| {
            let who = match &c.resolver {
                Resolver::RelatedByName { group: g } => match group(g) {
                    Some(v) => format!("related by name \"{v}\""),
                    None => format!("related by name (no {g} in the name)"),
                },
                Resolver::Related { roles } if roles.is_empty() => "every related client".into(),
                Resolver::Related { roles } => format!("related as {}", roles.join(", ")),
                Resolver::Capture { group: g } => match group(g) {
                    Some(v) => format!("client ID {v}"),
                    None => format!("client ID (no {g} in the name)"),
                },
                Resolver::Manual => "picked in Preview".into(),
            };
            format!("{who} → {}", c.dest_template)
        })
        .collect();
    TestResult {
        parsed,
        rule: rule.name.clone(),
        tried,
        note,
        primary,
        client: format!("{} ({})", client.name, client.id),
        copies,
    }
}

fn sample_client(extra_tokens: &[String]) -> Client {
    Client {
        uuid: uuid::Uuid::new_v4(),
        id: "1234".into(),
        name: "Sample Client".into(),
        extra: extra_tokens
            .iter()
            .map(|t| (t.clone(), format!("[{t}]")))
            .collect(),
    }
}
