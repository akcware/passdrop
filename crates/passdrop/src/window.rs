//! The egui window (DESIGN.md §8). It only draws `Form` state and forwards clicks;
//! the rules live in form.rs and the core, the look in theme.rs.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Align2, Color32, FontId, Key, Margin, RichText, Sense, Stroke, vec2};
use indexmap::IndexMap;
use passdrop_core::git::GitStatus;
use passdrop_core::guards;
use passdrop_core::run::{Answer, Existing, Target};
use passdrop_core::spec::Widget;

use crate::form::{self, DropOutcome, Form};
use crate::theme::{self, Palette};

pub enum Outcome {
    Pending,
    Submitted(IndexMap<String, Answer>),
    Cancelled,
}

pub struct Options {
    pub on_top: bool,
    pub deadline: Instant,
    /// Test harness only: save a PNG of the window and close.
    pub screenshot: Option<PathBuf>,
}

struct App<'a> {
    form: Form<'a>,
    outcome: Rc<RefCell<Outcome>>,
    deadline: Instant,
    zones: Vec<Option<egui::Rect>>,
    show_plain: Vec<bool>,
    pending_drop: Option<PathBuf>,
    drag: form::DragTracker,
    /// The file field under the cursor while a file is dragged over the window.
    drag_target: Option<usize>,
    textarea_focused: bool,
    closing: bool,
    frames: u64,
    #[cfg_attr(not(feature = "test-harness"), allow(dead_code))]
    screenshot: Option<PathBuf>,
}

/// Runs the window on the main thread. Err means the window could not be opened.
pub fn run(form: Form<'_>, opts: Options) -> Result<Outcome, String> {
    let title = format!("{} — passdrop", form.plan.spec.title);
    // Screenshots (test harness) use a tall window so the whole form is captured.
    let height = if opts.screenshot.is_some() { 1080.0 } else { 700.0 };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(title)
        .with_inner_size([580.0, height])
        .with_min_inner_size([460.0, 380.0])
        .with_drag_and_drop(true)
        .with_active(true);
    if opts.on_top {
        viewport = viewport.with_always_on_top();
    }
    let native = eframe::NativeOptions { viewport, ..Default::default() };
    let outcome = Rc::new(RefCell::new(Outcome::Pending));
    let shared = outcome.clone();
    let n = form.fields.len();
    let wide = theme::needs_wide_font(&spec_text(&form.plan.spec));
    eframe::run_native(
        "passdrop",
        native,
        Box::new(move |cc| {
            theme::install(&cc.egui_ctx, wide);
            Ok(Box::new(App {
                form,
                outcome: shared,
                deadline: opts.deadline,
                zones: vec![None; n],
                show_plain: vec![false; n],
                pending_drop: None,
                drag: form::DragTracker::default(),
                drag_target: None,
                textarea_focused: false,
                closing: false,
                frames: 0,
                screenshot: opts.screenshot,
            }))
        }),
    )
    .map_err(|e| e.to_string())?;
    Ok(outcome.replace(Outcome::Pending))
}

/// All agent-written text the window shows, to decide which fonts to load.
fn spec_text(spec: &passdrop_core::spec::Spec) -> String {
    let mut s = format!("{} {} {}", spec.title, spec.purpose, spec.description.as_deref().unwrap_or(""));
    for f in spec.properties.values() {
        s.push_str(&f.title);
        s.push_str(f.description.as_deref().unwrap_or(""));
        s.push_str(f.default.as_deref().unwrap_or(""));
        for v in f.examples.iter().chain(f.enum_values.iter()).flatten() {
            s.push_str(v);
        }
    }
    s
}

/// egui keeps an undo history of a text field's contents. For masked fields, drop it
/// every frame so no extra copies of the secret linger (DESIGN.md §13.5).
pub fn clear_undo(ctx: &egui::Context, id: egui::Id) {
    if let Some(mut st) = egui::text_edit::TextEditState::load(ctx, id) {
        st.clear_undoer();
        st.store(ctx, id);
    }
}

impl App<'_> {
    fn submit(&mut self, ctx: &egui::Context) {
        if self.closing || !self.form.can_submit() {
            return;
        }
        self.closing = true;
        *self.outcome.borrow_mut() = Outcome::Submitted(self.form.take_answers());
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn cancel(&mut self, ctx: &egui::Context) {
        if self.closing {
            return;
        }
        self.closing = true;
        *self.outcome.borrow_mut() = Outcome::Cancelled;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let (hovering, pos, dropped, inner) = ctx.input(|i| {
            (
                !i.raw.hovered_files.is_empty(),
                i.pointer.latest_pos(),
                i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect::<Vec<PathBuf>>(),
                i.viewport().inner_rect,
            )
        });
        if hovering || !dropped.is_empty() {
            // Keep redrawing while a file is over the window so the highlight follows it.
            ctx.request_repaint();
        }
        // Where the cursor is, in window points. Prefer the OS's global position: during an
        // external drag egui gets no pointer events. Fall back to egui's pointer only if it
        // moved during this drag.
        let from_os = match (crate::pointer::global(), inner) {
            (Some((x, y)), Some(r)) => Some(egui::pos2(x - r.min.x, y - r.min.y)),
            _ => None,
        };
        let trusted = self.drag.update(hovering, !dropped.is_empty(), pos.map(|p| (p.x, p.y)));
        let at = from_os.or(trusted.map(|(x, y)| egui::pos2(x, y)));
        let under_cursor = at.and_then(|p| self.zones.iter().position(|z| z.is_some_and(|r| r.contains(p))));
        self.drag_target = if hovering { under_cursor } else { None };
        let Some(first) = dropped.into_iter().next() else {
            return;
        };
        if let DropOutcome::Ask(p) = self.form.route_drop(first, under_cursor) {
            self.pending_drop = Some(p);
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let (esc, enter, cmd) = ctx.input(|i| (i.key_pressed(Key::Escape), i.key_pressed(Key::Enter), i.modifiers.command));
        if esc {
            if self.pending_drop.is_some() {
                self.pending_drop = None;
            } else {
                self.cancel(ctx);
            }
        }
        if enter && (cmd || !self.textarea_focused) {
            self.submit(ctx);
        }
        self.textarea_focused = false;
    }

    #[cfg(feature = "test-harness")]
    fn handle_screenshot(&mut self, ctx: &egui::Context) {
        let Some(path) = self.screenshot.clone() else {
            return;
        };
        ctx.request_repaint();
        if self.frames == 1
            && let Some(t) = crate::STARTED.get() {
                eprintln!("first_frame_ms={}", t.elapsed().as_millis());
            }
        if self.frames == 5 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        let image = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = image {
            crate::harness::save_png(&path, &img);
            self.cancel(ctx);
        }
    }

    fn header(&self, ui: &mut egui::Ui, p: Palette) {
        let spec = &self.form.plan.spec;
        ui.label(RichText::new("PASSDROP · SECURE INPUT").small().color(p.accent));
        ui.add_space(2.0);
        ui.label(RichText::new(&spec.title).heading().color(p.text));
        ui.label(RichText::new(&spec.purpose).color(p.muted));
        if let Some(d) = &spec.description {
            ui.label(RichText::new(d).color(p.muted));
        }
        ui.add_space(6.0);
        let cwd = guards::display_path(self.form.env, &self.form.env.cwd);
        ui.horizontal_wrapped(|ui| {
            theme::chip(ui, &format!("Requested by {}", form::caller()), p.text, p.chip);
            theme::chip(ui, &cwd, p.muted, p.chip);
        });
        ui.add_space(2.0);
        ui.label(
            RichText::new(
                "Values are written straight to the destinations below. The agent only sees fields marked “Visible to agent”.",
            )
            .small()
            .color(p.muted),
        );
        for w in &self.form.plan.warnings {
            ui.add_space(4.0);
            theme::callout(ui, p.warn, p.warn_bg, &w.message, |_| {});
        }
        ui.add_space(10.0);
    }

    fn drop_chooser(&mut self, ui: &mut egui::Ui, p: Palette) {
        let Some(path) = self.pending_drop.clone() else {
            return;
        };
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let plan = self.form.plan;
        let mut chosen = None;
        let mut dismiss = false;
        theme::card(p).stroke(Stroke::new(1.5, p.accent)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(format!("Which field is {name} for?")).strong().color(p.text));
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                for (i, pf) in plan.fields.iter().enumerate() {
                    if pf.field.widget() == Widget::File && self.form.accepts_input(i) && ui.button(&pf.field.title).clicked() {
                        chosen = Some(i);
                    }
                }
                if ui.button("Cancel").clicked() {
                    dismiss = true;
                }
            });
        });
        ui.add_space(10.0);
        if let Some(i) = chosen {
            self.form.set_file(i, path);
            self.pending_drop = None;
        } else if dismiss {
            self.pending_drop = None;
        }
    }

    fn drop_zone(&mut self, ui: &mut egui::Ui, i: usize, p: Palette, accept: &str) -> bool {
        // Highlight only the zone under the cursor; if the position is unknown, all of them.
        let any_hover = ui.ctx().input(|inp| !inp.raw.hovered_files.is_empty());
        let hovering = any_hover && self.drag_target.is_none_or(|t| t == i);
        let mut clear = false;
        let st = &self.form.fields[i];
        match &st.file {
            None => {
                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 68.0), Sense::hover());
                let painter = ui.painter();
                let (fill, line) = if hovering { (p.accent.gamma_multiply(0.12), p.accent) } else { (p.input, p.border) };
                painter.rect_filled(rect, 8.0, fill);
                theme::dashed_rect(painter, rect.shrink(0.5), Stroke::new(1.2, line));
                let what = if accept.is_empty() { "a file".to_string() } else { format!("your {accept} file") };
                painter.text(
                    rect.center() - vec2(0.0, 8.0),
                    Align2::CENTER_CENTER,
                    format!("Drop {what} here"),
                    FontId::proportional(13.5),
                    if hovering { p.accent } else { p.text },
                );
                painter.text(
                    rect.center() + vec2(0.0, 11.0),
                    Align2::CENTER_CENTER,
                    "It is copied to the destination below",
                    FontId::proportional(11.5),
                    p.muted,
                );
                self.zones[i] = Some(rect);
            }
            Some(file) => {
                let resp = egui::Frame::new()
                    .fill(p.input)
                    .stroke(Stroke::new(1.0, if hovering { p.accent } else { p.border }))
                    .corner_radius(8)
                    .inner_margin(Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&file.name).monospace().color(p.text));
                            ui.label(RichText::new(form::human_size(file.size)).small().color(p.muted));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button("Remove").clicked() {
                                    clear = true;
                                }
                            });
                        });
                        if let Err(e) = &file.target {
                            ui.label(RichText::new(e).small().color(p.danger));
                        }
                    });
                self.zones[i] = Some(resp.response.rect);
            }
        }
        clear
    }

    fn field(&mut self, ui: &mut egui::Ui, i: usize, p: Palette) {
        let ctx = ui.ctx().clone();
        let plan = self.form.plan;
        let pf = &plan.fields[i];
        let f = &pf.field;
        let widget = f.widget();
        let links = self.form.links(i);
        let steps = self.form.steps(i);
        let warnings = self.form.warnings(i);
        let destination = self.form.destination(i);
        let git = self.form.git_status(i);
        let accepts = self.form.accepts_input(i);
        let generated = pf.generate;
        let accept = f.accept.as_ref().and_then(|a| a.first()).cloned().unwrap_or_default();

        theme::card(p).show(ui, |ui| {
            ui.set_width(ui.available_width());

            // Title row with chips on the right.
            ui.horizontal(|ui| {
                ui.label(RichText::new(&f.title).strong().size(14.5).color(p.text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if f.echo {
                        theme::chip(ui, "Visible to agent", p.warn, p.warn_bg);
                    }
                    if f.write_only {
                        theme::chip(ui, "Secret", p.muted, p.chip);
                    }
                    if generated.is_some() {
                        theme::chip(ui, "Generated", p.muted, p.chip);
                    }
                    if pf.required {
                        theme::chip(ui, "Required", p.accent, p.accent.gamma_multiply(0.15));
                    }
                });
            });
            if let Some(d) = &f.description {
                ui.label(RichText::new(d).color(p.muted));
            }
            if !steps.is_empty() {
                ui.add_space(2.0);
                for (n, s) in steps.iter().enumerate() {
                    ui.label(RichText::new(format!("{}.  {s}", n + 1)).small().color(p.muted));
                }
            }
            for l in &links {
                let r = ui.link(RichText::new(format!("Get it at {} ↗", l.host)).color(p.accent));
                if r.on_hover_text(&l.url).clicked() {
                    ctx.open_url(egui::OpenUrl::new_tab(&l.url));
                }
            }
            ui.add_space(6.0);

            let st = &mut self.form.fields[i];
            if pf.existing == Existing::Set {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Already set").color(p.muted));
                    ui.add_space(4.0);
                    ui.selectable_value(&mut st.replace, false, "Keep current");
                    ui.selectable_value(&mut st.replace, true, if generated.is_some() { "Regenerate" } else { "Replace" });
                });
                ui.add_space(4.0);
            }

            let mut clear_file = false;
            if accepts {
                let id = egui::Id::new(("passdrop-field", i));
                let masked = f.write_only && !self.show_plain[i];
                let hint = f.examples.as_ref().and_then(|e| e.first()).cloned().unwrap_or_default();
                match widget {
                    Widget::File => {
                        clear_file = self.drop_zone(ui, i, p, &accept);
                    }
                    Widget::Textarea => {
                        let st = &mut self.form.fields[i];
                        let resp = ui.add(
                            egui::TextEdit::multiline(&mut *st.text)
                                .id(id)
                                .password(masked)
                                .hint_text(hint)
                                .desired_rows(4)
                                .desired_width(f32::INFINITY)
                                .margin(Margin::symmetric(8, 6))
                                .font(egui::TextStyle::Monospace),
                        );
                        self.textarea_focused |= resp.has_focus();
                        if f.write_only {
                            let show = ui.add(egui::Button::new("Hold to show").small().sense(Sense::click_and_drag()));
                            self.show_plain[i] = show.is_pointer_button_down_on();
                            clear_undo(&ctx, id);
                        }
                    }
                    Widget::Text => {
                        let st = &mut self.form.fields[i];
                        if let Some(values) = &f.enum_values {
                            let shown = if st.text.is_empty() { "Choose…".to_string() } else { st.text.to_string() };
                            egui::ComboBox::from_id_salt(("passdrop-enum", i))
                                .selected_text(shown)
                                .width(ui.available_width())
                                .show_ui(ui, |ui| {
                                    for v in values {
                                        ui.selectable_value(&mut *st.text, v.clone(), v);
                                    }
                                });
                        } else {
                            ui.horizontal(|ui| {
                                let reserve = if f.write_only { 64.0 } else { 0.0 };
                                ui.add(
                                    egui::TextEdit::singleline(&mut *st.text)
                                        .id(id)
                                        .password(masked)
                                        .hint_text(hint)
                                        .margin(Margin::symmetric(8, 6))
                                        .desired_width((ui.available_width() - reserve).max(120.0)),
                                );
                                if f.write_only {
                                    let show = ui
                                        .add(egui::Button::new("Show").min_size(vec2(56.0, 28.0)).sense(Sense::click_and_drag()))
                                        .on_hover_text("Hold to show the value");
                                    self.show_plain[i] = show.is_pointer_button_down_on();
                                }
                            });
                            if f.write_only {
                                clear_undo(&ctx, id);
                            }
                        }
                    }
                }
            } else if let (Some(g), false) = (generated, self.form.is_kept(i)) {
                theme::callout(ui, p.text, p.input, &format!("Generated when you save: {}. Nobody sees it.", g.describe()), |_| {});
            }

            if !warnings.is_empty() {
                ui.add_space(2.0);
                for w in &warnings {
                    ui.label(RichText::new(format!("⚠  {}", form::warning_text(*w))).small().color(p.warn));
                }
            }

            // Destination.
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if pf.target.is_none() {
                    ui.label(RichText::new("Returned to the agent · visible in chat").small().color(p.warn));
                } else {
                    ui.label(RichText::new("Saves to").small().color(p.muted));
                    ui.label(RichText::new(&destination).monospace().color(p.text));
                }
            });

            let st = &mut self.form.fields[i];
            let writes_here = pf.target.is_some() && !matches!((pf.existing, st.replace), (Existing::Set, false));
            if writes_here {
                match git {
                    Some(GitStatus::Tracked) => {
                        ui.add_space(4.0);
                        theme::callout(ui, p.danger, p.danger_bg, "This file is tracked by git, so the value would be committed.", |ui| {
                            ui.checkbox(&mut st.allow_tracked, "Write anyway");
                        });
                    }
                    Some(GitStatus::Untracked) => {
                        ui.checkbox(&mut st.add_to_gitignore, RichText::new("Also add it to .gitignore").small());
                    }
                    Some(GitStatus::Unknown) => {
                        ui.label(RichText::new("git status unknown").small().color(p.muted));
                    }
                    _ => {}
                }
            }
            if let (Some(file), Some(Target::File { .. })) = (&st.file, &pf.target)
                && let Ok(t) = &file.target {
                    if t.is_source {
                        ui.label(RichText::new("This is already the destination file; it stays where it is.").small().color(p.muted));
                    } else {
                        if t.exists && pf.existing != Existing::Set {
                            ui.add_space(4.0);
                            theme::callout(ui, p.warn, p.warn_bg, &format!("A file named {} already exists there.", file.name), |ui| {
                                ui.checkbox(&mut st.replace_dropped, "Replace it");
                            });
                        }
                        let unexpected = passdrop_core::run::is_unexpected_file(&file.warnings);
                        if unexpected && !st.trash_original {
                            ui.label(
                                RichText::new("This doesn't look like the expected file, so the original stays where it is.")
                                    .small()
                                    .color(p.muted),
                            );
                        }
                        let label = if unexpected {
                            "Move the original to the Trash anyway"
                        } else {
                            "Move the original to the Trash after saving"
                        };
                        ui.checkbox(&mut st.trash_original, RichText::new(label).small());
                    }
                }
            if clear_file {
                self.form.clear_file(i);
            }
        });
        ui.add_space(10.0);
    }

    fn footer(&mut self, ui: &mut egui::Ui, p: Palette) {
        let ctx = ui.ctx().clone();
        let blockers = self.form.blockers();
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                let left = self.deadline.saturating_duration_since(Instant::now()).as_secs();
                match blockers.first() {
                    Some(b) => {
                        ui.label(RichText::new(b).small().color(p.warn));
                    }
                    None => {
                        ui.label(RichText::new("Ready to save").small().color(p.muted));
                    }
                }
                ui.label(RichText::new(format!("Closes in {}:{:02}", left / 60, left % 60)).small().color(p.muted));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let save = egui::Button::new(RichText::new("Save").strong().color(Color32::WHITE))
                    .fill(p.accent)
                    .stroke(Stroke::NONE)
                    .min_size(vec2(88.0, 30.0));
                if ui.add_enabled(blockers.is_empty(), save).clicked() {
                    self.submit(&ctx);
                }
                if ui.add(egui::Button::new("Cancel").min_size(vec2(80.0, 30.0))).clicked() {
                    self.cancel(&ctx);
                }
            });
        });
    }
}

impl eframe::App for App<'_> {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let p = theme::current(ui);
        self.frames += 1;
        ctx.request_repaint_after(Duration::from_millis(500));
        self.handle_drops(&ctx);
        self.handle_keys(&ctx);
        #[cfg(feature = "test-harness")]
        self.handle_screenshot(&ctx);

        egui::Panel::bottom("passdrop-footer")
            .frame(egui::Frame::new().fill(p.bg).stroke(Stroke::new(1.0, p.border)).inner_margin(Margin::symmetric(18, 12)))
            .show(ui, |ui| self.footer(ui, p));
        egui::CentralPanel::default().frame(egui::Frame::new().fill(p.bg)).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                egui::Frame::new().inner_margin(Margin { left: 18, right: 18, top: 16, bottom: 8 }).show(ui, |ui| {
                    self.header(ui, p);
                    self.drop_chooser(ui, p);
                    for i in 0..self.form.fields.len() {
                        self.field(ui, i, p);
                    }
                });
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SENTINEL: &str = "SENTINEL-4b1d9e";

    /// The latest undo snapshot that differs from an empty field, if any.
    fn undo_snapshot(ctx: &egui::Context, id: egui::Id) -> Option<String> {
        let mut undoer = egui::text_edit::TextEditState::load(ctx, id)?.undoer();
        let empty = (egui::text::CCursorRange::default(), String::new());
        undoer.undo(&empty).map(|(_, s)| s.clone())
    }

    fn frame(ctx: &egui::Context, id: egui::Id, text: &mut String, clear: bool) -> String {
        let out = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.memory_mut(|m| m.request_focus(id));
            ui.add(egui::TextEdit::singleline(text).id(id).password(true));
            if clear {
                clear_undo(ui.ctx(), id);
            }
        });
        format!("{:?} {:?}", out.platform_output.accesskit_update, out.platform_output.events)
    }

    #[test]
    fn masked_text_stays_out_of_accessibility_output_and_undo_history() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let id = egui::Id::new("masked");
        let mut text = SENTINEL.to_string();

        // Without clearing, egui's undo history holds a copy (proves the probe works).
        for _ in 0..3 {
            let platform = frame(&ctx, id, &mut text, false);
            // The accessibility tree is really produced, with a password node in it...
            assert!(platform.contains("PasswordInput"), "no accessibility output to check");
            // ...and it never carries the value.
            assert!(!platform.contains(SENTINEL), "accessibility/platform output leaked the value");
        }
        assert!(undo_snapshot(&ctx, id).is_some_and(|s| s.contains(SENTINEL)), "expected egui to keep an undo copy");

        // With clear_undo after each frame, no copy remains.
        for _ in 0..3 {
            let platform = frame(&ctx, id, &mut text, true);
            assert!(!platform.contains(SENTINEL));
        }
        assert!(!undo_snapshot(&ctx, id).is_some_and(|s| s.contains(SENTINEL)));
    }
}
