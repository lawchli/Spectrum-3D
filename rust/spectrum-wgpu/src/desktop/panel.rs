//! The docked interface, laid out as web/index.html and drawn with the sizes and colours
//! of web/style.css: the control panel on the left, the monitor along the bottom of the
//! picture. Every word comes from the shared strings.json.
use super::{text, App};
use crate::monitor::{self, ink::*, Part};
use egui::{
    pos2, vec2, Align, Align2, Color32, FontId, Frame, Id, Layout, Margin, Rect, Response, Sense,
    Shape, Stroke, StrokeKind, Ui,
};
use spectrum_wgpu::gpu::Gpu;

/// `--panel-w` and `--label-w`.
const PANEL_W: f32 = 264.;
const RAIL_W: f32 = 44.;
const LABEL_W: f32 = 52.;
const HELP: f32 = 15.;

fn sans(size: f32) -> FontId {
    FontId::proportional(size)
}
fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}
fn hairline() -> Stroke {
    Stroke::new(1., BORDER_SUBTLE)
}

/// egui's own widgets (the two drop-down lists and their pop-ups, tooltips, the scroll
/// bar) in the page's colours.
pub fn apply_visuals(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        let v = &mut style.visuals;
        *v = egui::Visuals::dark();
        v.panel_fill = BG_PANEL;
        v.window_fill = BG_CONTROL;
        v.extreme_bg_color = BG_WELL;
        v.faint_bg_color = BG_CONTROL;
        v.window_stroke = Stroke::new(1., BORDER_STRONG);
        v.window_corner_radius = 3.into();
        v.menu_corner_radius = 3.into();
        v.window_shadow = egui::Shadow::NONE;
        v.popup_shadow = egui::Shadow::NONE;
        v.selection.bg_fill = BG_RAISED;
        v.selection.stroke = Stroke::new(1., INK_1);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = BG_PANEL;
        w.noninteractive.weak_bg_fill = BG_PANEL;
        w.noninteractive.bg_stroke = hairline();
        w.noninteractive.fg_stroke = Stroke::new(1., INK_1);
        for (state, fill, border, text) in [
            (&mut w.inactive, BG_CONTROL, BORDER_MID, INK_1),
            (&mut w.hovered, BG_RAISED, BORDER_STRONG, INK_1),
            (&mut w.active, BG_RAISED, BORDER_STRONG, INK_1),
            (&mut w.open, BG_RAISED, BORDER_STRONG, INK_1),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::new(1., border);
            state.fg_stroke = Stroke::new(1.3, text);
            state.corner_radius = 3.into();
            state.expansion = 0.;
        }
        style.spacing.tooltip_width = 248.;
        style.interaction.tooltip_delay = 0.15;
        style.interaction.show_tooltips_only_when_still = false;
    });
}

// ---- Widgets -----------------------------------------------------------------------

/// One button of a segmented control (`.seg > button`).
struct Seg {
    label: String,
    kbd: Option<String>,
    on: bool,
    enabled: bool,
}
impl Seg {
    fn new(label: impl Into<String>, on: bool) -> Self {
        Self {
            label: label.into(),
            kbd: None,
            on,
            enabled: true,
        }
    }
    fn kbd(mut self, key: &str) -> Self {
        self.kbd = Some(key.into());
        self
    }
    fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// A key hint (`.kbd`) ending at `x`; returns its width.
fn kbd(ui: &Ui, left: f32, y: f32, key: &str, on: bool) -> f32 {
    let p = ui.painter();
    let g = p.layout_no_wrap(key.into(), mono(10.), Color32::PLACEHOLDER);
    let w = (g.size().x + 6.).max(15.);
    let r = Rect::from_min_size(pos2(left, y - 7.5), vec2(w, 15.));
    p.rect(
        r,
        2.,
        if on { BG_PANEL } else { BG_WELL },
        Stroke::new(1., BORDER_MID),
        StrokeKind::Inside,
    );
    p.galley(
        r.center() - g.size() / 2.,
        g,
        if on { INK_2 } else { INK_3 },
    );
    w
}
fn kbd_width(ui: &Ui, key: &str) -> f32 {
    let g = ui
        .painter()
        .layout_no_wrap(key.into(), mono(10.), Color32::PLACEHOLDER);
    (g.size().x + 6.).max(15.)
}

/// Equal buttons in a well, each with a dot that is amber when on. Returns the clicked one.
fn seg(ui: &mut Ui, id: &str, items: &[Seg], width: f32) -> Option<usize> {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 34.), Sense::hover());
    let p = ui.painter().clone();
    p.rect(rect, 4., BG_WELL, hairline(), StrokeKind::Inside);
    let inner = rect.shrink(3.);
    let gap = 2.;
    let n = items.len() as f32;
    let w = (inner.width() - gap * (n - 1.)) / n;
    let mut clicked = None;
    for (i, item) in items.iter().enumerate() {
        let r = Rect::from_min_size(
            pos2(inner.left() + i as f32 * (w + gap), inner.top()),
            vec2(w, inner.height()),
        );
        let sense = if item.enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let resp = ui.interact(r, Id::new(id).with(i), sense);
        let hover = resp.hovered() && item.enabled && !item.on;
        if item.on {
            p.rect(
                r,
                3.,
                BG_RAISED,
                Stroke::new(1., BORDER_STRONG),
                StrokeKind::Inside,
            );
        } else if hover {
            p.rect_filled(r, 3., Color32::from_white_alpha(8));
        }
        let color = if !item.enabled {
            INK_3.gamma_multiply(0.42)
        } else if item.on || hover {
            INK_1
        } else {
            INK_2
        };
        let g = p.layout_no_wrap(item.label.clone(), sans(12.), Color32::PLACEHOLDER);
        let key_w = item.kbd.as_ref().map_or(0., |k| 4. + kbd_width(ui, k));
        let mut x = r.center().x - (8. + g.size().x + key_w) / 2.;
        let y = r.center().y;
        let dot = if item.on {
            ACCENT
        } else {
            INK_3.gamma_multiply(if item.enabled { 0.42 } else { 0.16 })
        };
        p.circle_filled(pos2(x + 2., y), 2., dot);
        x += 8.;
        let text_w = g.size().x;
        p.galley(pos2(x, y - g.size().y / 2.), g, color);
        if let Some(k) = &item.kbd {
            kbd(ui, x + text_w + 4., y, k, item.on);
        }
        if resp.clicked() {
            clicked = Some(i);
        }
    }
    clicked
}

/// A labelled check box (`.check`): 13 px box, 8 px gap, the label brightening when on.
fn check(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
    let g = ui
        .painter()
        .layout_no_wrap(label.into(), sans(12.), Color32::PLACEHOLDER);
    let (rect, mut resp) = ui.allocate_exact_size(vec2(21. + g.size().x, 24.), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    paint_check(ui, rect, *on, resp.hovered(), g, 1.);
    resp
}
fn paint_check(
    ui: &Ui,
    rect: Rect,
    on: bool,
    hover: bool,
    g: std::sync::Arc<egui::Galley>,
    opacity: f32,
) {
    let p = ui.painter();
    let b = Rect::from_center_size(pos2(rect.left() + 6.5, rect.center().y), vec2(13., 13.));
    p.rect(
        b,
        2.,
        if on { BG_RAISED } else { BG_WELL },
        Stroke::new(
            1.,
            if on || hover {
                BORDER_STRONG
            } else {
                BORDER_MID
            },
        ),
        StrokeKind::Inside,
    );
    if on {
        let o = b.center() - vec2(4.5, 4.5);
        let points = [(1.8, 5.2), (4.0, 7.4), (8.3, 2.6)]
            .map(|(x, y)| o + vec2(x * 0.9, y * 0.9))
            .to_vec();
        p.add(Shape::line(points, Stroke::new(1.5, INK_1)));
    }
    let color = if on || hover { INK_1 } else { INK_2 };
    p.galley(
        pos2(rect.left() + 21., rect.center().y - g.size().y / 2.),
        g,
        color.gamma_multiply(opacity),
    );
}

/// The small `?` with the explanation as its tooltip (`.help`, `#tooltip`).
fn help(ui: &mut Ui, tip: &str) {
    let (r, resp) = ui.allocate_exact_size(vec2(HELP, HELP), Sense::hover());
    let hover = resp.hovered();
    let p = ui.painter();
    p.rect(
        r,
        2.,
        if hover {
            BG_RAISED
        } else {
            Color32::TRANSPARENT
        },
        Stroke::new(1., if hover { BORDER_STRONG } else { BORDER_MID }),
        StrokeKind::Inside,
    );
    p.text(
        r.center(),
        Align2::CENTER_CENTER,
        "?",
        mono(10.),
        if hover { INK_1 } else { INK_2 },
    );
    resp.on_hover_ui(|ui| {
        ui.label(egui::RichText::new(tip).color(INK_1).size(12.));
    });
}

/// A bordered button: `.btn-compact` (22 px, label then key) or `.util-btn` (26 px, label
/// left and key right, filling `width`).
fn button(ui: &mut Ui, label: &str, key: &str, on: bool, width: Option<f32>) -> Response {
    let g = ui
        .painter()
        .layout_no_wrap(label.into(), sans(10.), Color32::PLACEHOLDER);
    let key_w = kbd_width(ui, key);
    let util = width.is_some();
    let size = match width {
        Some(w) => vec2(w, 26.),
        None => vec2(6. + g.size().x + 4. + key_w + 6., 22.),
    };
    let (r, resp) = ui.allocate_exact_size(size, Sense::click());
    let hover = resp.hovered();
    let (fill, border) = if on {
        (BG_RAISED, BORDER_STRONG)
    } else if hover && util {
        (BG_CONTROL, BORDER_MID)
    } else if hover {
        (BG_RAISED, BORDER_STRONG)
    } else if util {
        (BG_PANEL, BORDER_SUBTLE)
    } else {
        (BG_CONTROL, BORDER_MID)
    };
    let p = ui.painter();
    p.rect(r, 3., fill, Stroke::new(1., border), StrokeKind::Inside);
    let pad = if util { 8. } else { 6. };
    let y = r.center().y;
    p.galley(
        pos2(r.left() + pad, y - g.size().y / 2.),
        g,
        if on || hover { INK_1 } else { INK_2 },
    );
    kbd(ui, r.right() - pad - key_w, y, key, on);
    resp
}

/// A monitor column switch (`.chip`).
fn chip(ui: &mut Ui, label: &str, on: &mut bool, width: f32, dim: bool) {
    let (r, resp) = ui.allocate_exact_size(vec2(width, 22.), Sense::click());
    if resp.clicked() {
        *on = !*on;
    }
    let hover = resp.hovered();
    let (fill, border, color) = if *on {
        (BG_RAISED, BORDER_MID, INK_1)
    } else if hover {
        (BG_PANEL, BORDER_MID, INK_2)
    } else {
        (BG_PANEL, BORDER_SUBTLE, INK_3)
    };
    let opacity = if dim { 0.38 } else { 1. };
    let p = ui.painter();
    p.rect(
        r,
        2.,
        fill.gamma_multiply(opacity),
        Stroke::new(1., border.gamma_multiply(opacity)),
        StrokeKind::Inside,
    );
    p.text(
        r.center(),
        Align2::CENTER_CENTER,
        label,
        sans(10.),
        color.gamma_multiply(opacity),
    );
}

/// The "画面内" slider: a thin track filled up to the thumb, whole steps.
fn slider(ui: &mut Ui, value: &mut f64, range: (f64, f64), width: f32) {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 24.), Sense::click_and_drag());
    let travel = rect.width() - 10.;
    if let Some(pos) = resp.interact_pointer_pos() {
        let f = ((pos.x - rect.left() - 5.) / travel).clamp(0., 1.) as f64;
        *value = (range.0 + f * (range.1 - range.0)).round();
    }
    let f = ((*value - range.0) / (range.1 - range.0)).clamp(0., 1.) as f32;
    let x = rect.left() + 5. + f * travel;
    let p = ui.painter();
    let track = Rect::from_center_size(rect.center(), vec2(rect.width(), 4.));
    p.rect_filled(track, 2., BG_WELL);
    p.rect_filled(
        Rect::from_min_max(track.min, pos2(x, track.max.y)),
        2.,
        INK_3,
    );
    p.rect_stroke(track, 2., Stroke::new(1., BORDER_MID), StrokeKind::Inside);
    p.rect(
        Rect::from_center_size(pos2(x, rect.center().y), vec2(10., 16.)),
        2.,
        INK_1,
        Stroke::new(1., BG_WELL),
        StrokeKind::Inside,
    );
}

/// A drop-down list in a 28 px control filling the row (`select`).
fn select(ui: &mut Ui, id: &str, selected: &str, width: f32, add: impl FnOnce(&mut Ui)) {
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = 28.;
        ui.spacing_mut().button_padding = vec2(8., 4.);
        ui.style_mut().visuals.widgets.inactive.fg_stroke.color = INK_1;
        egui::ComboBox::from_id_salt(id)
            .selected_text(egui::RichText::new(selected).size(12.))
            .width(width)
            .height(400.)
            .show_ui(ui, |ui| {
                ui.spacing_mut().button_padding = vec2(8., 4.);
                add(ui)
            });
    });
}

/// A label column of fixed width, then the control (`.field-row`). `height` is the
/// control's, so the label centres on it: 34 for a segmented control, 28 otherwise.
fn field_row(ui: &mut Ui, label: &str, height: f32, add: impl FnOnce(&mut Ui)) {
    let text_w = ui
        .painter()
        .layout_no_wrap(label.into(), sans(12.), Color32::PLACEHOLDER)
        .size()
        .x
        .ceil();
    let label_w = text_w.max(LABEL_W);
    ui.horizontal(|ui| {
        ui.set_min_height(height);
        let (r, _) = ui.allocate_exact_size(vec2(label_w, height), Sense::hover());
        ui.painter().text(
            r.left_center(),
            Align2::LEFT_CENTER,
            label,
            sans(12.),
            INK_2,
        );
        add(ui);
    });
}

/// A section title with tools on the right (`.sec-head`).
fn sec_head(ui: &mut Ui, title: &str, tools: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.set_min_height(18.);
        ui.label(egui::RichText::new(title).size(10.).color(INK_3));
        ui.with_layout(Layout::right_to_left(Align::Center), tools);
    });
    ui.add_space(8.);
}

/// A section with the page's padding and the hairline under all but the last.
fn section(ui: &mut Ui, first: bool, last: bool, add: impl FnOnce(&mut Ui)) {
    ui.add_space(if first { 4. } else { 8. });
    add(ui);
    ui.add_space(if last { 4. } else { 12. });
    if !last {
        let r = ui.available_rect_before_wrap();
        ui.painter().hline(r.x_range(), r.top(), hairline());
    }
}

/// A row with something on the left and something on the right (`.check-row-action`).
fn row_with_action(ui: &mut Ui, left: impl FnOnce(&mut Ui), right: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        left(ui);
        ui.with_layout(Layout::right_to_left(Align::Center), right);
    });
}

#[derive(Clone, Copy)]
enum Chevron {
    Left,
    Right,
    Up,
    Down,
}

/// The 28 px dock toggle with its chevron (`.dock-toggle`).
fn dock_toggle(ui: &mut Ui, r: Rect, chevron: Chevron, label: &str) -> bool {
    let resp = ui.interact(r, Id::new(("dock-toggle", label)), Sense::click());
    let hover = resp.hovered();
    let p = ui.painter();
    p.rect(
        r,
        3.,
        if hover { BG_RAISED } else { BG_CONTROL },
        Stroke::new(1., if hover { BORDER_STRONG } else { BORDER_MID }),
        StrokeKind::Inside,
    );
    // The page's icons: 24-unit paths drawn at 16 px.
    let points: [(f32, f32); 3] = match chevron {
        Chevron::Left => [(14., 7.), (9., 12.), (14., 17.)],
        Chevron::Right => [(10., 7.), (15., 12.), (10., 17.)],
        Chevron::Down => [(7., 9.), (12., 14.), (17., 9.)],
        Chevron::Up => [(7., 15.), (12., 10.), (17., 15.)],
    };
    let c = r.center();
    let points = points
        .map(|(x, y)| c + vec2(x - 12., y - 12.) * (16. / 24.))
        .to_vec();
    p.add(Shape::line(
        points,
        Stroke::new(1.5, if hover { INK_1 } else { INK_2 }),
    ));
    resp.on_hover_text(label).clicked()
}

/// The strip on top of a dock: its title and the collapse toggle (`.dock-head`).
/// `badge` is a state shown in amber after the title, as the monitor's "frozen".
fn dock_head(ui: &mut Ui, title: &str, badge: Option<&str>, chevron: Chevron, label: &str) -> bool {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 0., BG_WELL);
    p.hline(rect.x_range(), rect.bottom() - 0.5, hairline());
    let title = p.text(
        pos2(rect.left() + 12., rect.center().y),
        Align2::LEFT_CENTER,
        title,
        sans(10.),
        INK_2,
    );
    if let Some(badge) = badge {
        let dot = pos2(title.right() + 12., rect.center().y);
        p.circle_filled(dot, 2.5, ACCENT);
        p.text(
            dot + vec2(7., 0.),
            Align2::LEFT_CENTER,
            badge,
            sans(10.),
            ACCENT,
        );
    }
    let r = Rect::from_min_size(
        pos2(rect.right() - 4. - 28., rect.top() + 2.),
        vec2(28., 28.),
    );
    dock_toggle(ui, r, chevron, label)
}

// ---- The panel ---------------------------------------------------------------------

impl App {
    /// The docks: the panel on the left over the full height, the monitor below the
    /// picture. With the interface hidden (`H`) only the restore button is left.
    pub(super) fn docks(&mut self, ctx: &egui::Context, g: &mut Gpu, view: Option<&monitor::View>) {
        let strings = self.strings.clone();
        let lang = self.lang.clone();
        let t = |key: &str| text(&strings, &lang, key);
        if self.options.no_ui {
            self.restore_button(ctx, &t);
            return;
        }
        let frame = Frame::NONE.fill(BG_PANEL);
        if self.panel_collapsed {
            egui::SidePanel::left("panel")
                .exact_width(RAIL_W)
                .resizable(false)
                .frame(frame)
                .show(ctx, |ui| {
                    let r = Rect::from_min_size(ui.max_rect().min + vec2(8., 2.), vec2(28., 28.));
                    if dock_toggle(ui, r, Chevron::Right, &t("expandPanel")) {
                        self.panel_collapsed = false;
                    }
                });
        } else {
            egui::SidePanel::left("panel")
                .exact_width(PANEL_W)
                .resizable(false)
                .frame(frame)
                .show(ctx, |ui| {
                    ui.spacing_mut().item_spacing = vec2(8., 0.);
                    if dock_head(ui, &t("controls"), None, Chevron::Left, &t("collapsePanel")) {
                        self.panel_collapsed = true;
                    }
                    egui::ScrollArea::vertical()
                        .auto_shrink(false)
                        .show(ui, |ui| {
                            Frame::NONE
                                .inner_margin(Margin::symmetric(12, 8))
                                .show(ui, |ui| self.panel_main(ui, g, &t));
                            // The auxiliary part sits at the bottom while there is room
                            // (`margin-top: auto`) and scrolls with the rest when not.
                            let room = ui.clip_rect().bottom() - ui.cursor().top();
                            // A pixel short of the bottom, so that rounding never asks
                            // for a scroll bar.
                            ui.add_space((room - self.aux_height - 1.).max(0.));
                            let top = ui.cursor().top();
                            self.panel_aux(ui, g, &t);
                            self.aux_height = ui.cursor().top() - top;
                        });
                });
        }
        if let Some(view) = view {
            egui::TopBottomPanel::bottom("hud")
                .resizable(false)
                .frame(frame)
                .show(ctx, |ui| {
                    ui.spacing_mut().item_spacing = vec2(0., 0.);
                    let (chevron, label) = if self.hud_collapsed {
                        (Chevron::Up, t("expandHud"))
                    } else {
                        (Chevron::Down, t("collapseHud"))
                    };
                    if dock_head(ui, &t("monitor"), view.frozen.as_deref(), chevron, &label) {
                        self.hud_collapsed = !self.hud_collapsed;
                    }
                    if !self.hud_collapsed {
                        let parts: Vec<Part> = Part::ALL
                            .into_iter()
                            .zip(self.monitor_parts)
                            .filter_map(|(p, on)| on.then_some(p))
                            .collect();
                        Frame::NONE
                            .inner_margin(Margin::symmetric(12, 8))
                            .show(ui, |ui| monitor::dock(ui, view, &parts));
                    }
                });
        }
    }

    fn restore_button(&mut self, ctx: &egui::Context, t: &dyn Fn(&str) -> String) {
        egui::Area::new(Id::new("restore-ui"))
            .fixed_pos(pos2(8., 8.))
            .show(ctx, |ui| {
                let label = t("restoreUi");
                let g = ui
                    .painter()
                    .layout_no_wrap(label, sans(12.), Color32::PLACEHOLDER);
                let (r, resp) = ui.allocate_exact_size(
                    vec2(12. + 16. + 8. + g.size().x + 12., 44.),
                    Sense::click(),
                );
                let hover = resp.hovered();
                let p = ui.painter();
                p.rect(
                    r,
                    3.,
                    if hover { BG_RAISED } else { BG_CONTROL },
                    Stroke::new(1., if hover { BORDER_STRONG } else { BORDER_MID }),
                    StrokeKind::Inside,
                );
                let color = if hover { INK_1 } else { INK_2 };
                // The page's icon: a window with the panel on its left.
                let icon =
                    Rect::from_center_size(pos2(r.left() + 20., r.center().y), vec2(16., 16.));
                let s = Stroke::new(1.5, color);
                let u = |x: f32, y: f32| icon.min + vec2(x, y) * (16. / 24.);
                p.rect_stroke(
                    Rect::from_min_max(u(4., 4.), u(20., 20.)),
                    1.,
                    s,
                    StrokeKind::Middle,
                );
                p.line_segment([u(10., 4.), u(10., 20.)], s);
                p.line_segment([u(10., 15.), u(20., 15.)], s);
                p.galley(
                    pos2(r.left() + 36., r.center().y - g.size().y / 2.),
                    g,
                    color,
                );
                if resp.clicked() {
                    self.options.no_ui = false;
                    self.hide_ui = false;
                }
            });
    }

    fn panel_main(&mut self, ui: &mut Ui, g: &mut Gpu, t: &dyn Fn(&str) -> String) {
        let full = ui.available_width();
        let tiers_w = full - LABEL_W - 8. - 8. - HELP;
        let scene = g.scene.as_mut().unwrap();
        let s = &mut scene.settings;

        // Source
        section(ui, true, false, |ui| {
            sec_head(ui, &t("source"), |ui| self.lang_switch(ui));
            let testing = matches!(self.source.as_str(), "test" | "test1" | "test2");
            let test_label = match self.source.as_str() {
                "test1" => t("test").replace("{n}", "1"),
                "test2" => t("test").replace("{n}", "2"),
                _ => t("synth"),
            };
            let items = [
                Seg::new(test_label, testing),
                Seg::new(t("input"), self.source == "microphone"),
                Seg::new(t("file"), self.source == "file"),
            ];
            let picked = seg(ui, "source", &items, full);
            if picked.is_some() {
                self.cli_source = false;
            }
            match picked {
                Some(0) => {
                    self.loading = None;
                    self.source = match self.source.as_str() {
                        "test1" => "test2",
                        "test2" => "off",
                        _ => "test1",
                    }
                    .into();
                    self.error = None;
                }
                Some(1) if self.source != "microphone" => {
                    self.loading = None;
                    self.source = "microphone".into();
                    self.error = None;
                }
                Some(2) => self.choose_file(None),
                _ => {}
            }
            ui.add_space(8.);
            if let Some(error) = self.worker.as_ref().and_then(|w| w.output_error()) {
                ui.label(t("outputUnavailable").replace("{msg}", &error));
            }
            if self.source == "microphone" {
                ui.add_space(8.);
                self.device_select(ui, full, t);
            }
            if self.source == "microphone" {
                if let Some(key) = self.worker.as_ref().and_then(|w| w.input_notice()) {
                    ui.label(t(key));
                }
            }
            ui.add_space(8.);
            ui.horizontal(|ui| {
                ui.set_min_height(22.);
                let status = if self.loading.is_some() {
                    t("openingFile")
                } else if let Some(error) = &self.error {
                    t("cannotOpen").replace("{msg}", error)
                } else {
                    match self.source.as_str() {
                        "file" => self.file_name.clone().unwrap_or_else(|| t("file")),
                        "test1" => t("test").replace("{n}", "1"),
                        "test2" => t("test").replace("{n}", "2"),
                        "test" => t("synth"),
                        "microphone"
                            if self.worker.as_ref().is_some_and(|w| {
                                w.input_status() == Some(spectrum_wgpu::audio::InputStatus::Waiting)
                            }) =>
                        {
                            t("waitingDevice")
                        }
                        "microphone"
                            if self.worker.as_ref().is_some_and(|w| {
                                w.input_status() == Some(spectrum_wgpu::audio::InputStatus::Opening)
                            }) =>
                        {
                            t("openingInput")
                        }
                        "microphone" => t("input"),
                        _ => t("stopped"),
                    }
                };
                let right = if self.source == "microphone" {
                    let label = t("refreshDevices");
                    ui.painter()
                        .layout_no_wrap(label, sans(10.), Color32::PLACEHOLDER)
                        .size()
                        .x
                        + 12.
                        + 8.
                } else {
                    0.
                };
                let listen_width = ui
                    .painter()
                    .layout_no_wrap(t("listen"), sans(12.), INK_2)
                    .size()
                    .x
                    + 28.;
                let room = (ui.available_width() - right - listen_width).max(20.);
                let g = ui.painter().layout(status.clone(), sans(12.), INK_2, room);
                let (r, resp) = ui.allocate_exact_size(vec2(room, 22.), Sense::hover());
                let clipped = g.size().y > 20.;
                let line = ui
                    .painter()
                    .layout_no_wrap(status.clone(), sans(12.), INK_2);
                ui.painter().with_clip_rect(r).galley(
                    pos2(r.left(), r.center().y - line.size().y / 2.),
                    line,
                    INK_2,
                );
                if clipped {
                    resp.on_hover_text(status);
                }
                ui.checkbox(&mut self.listen, t("listen"));
                if self.source == "microphone" {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let enabled = self.device_enumeration.is_none();
                        let label = t("refreshDevices");
                        let g = ui
                            .painter()
                            .layout_no_wrap(label, sans(10.), Color32::PLACEHOLDER);
                        let (r, resp) = ui.allocate_exact_size(
                            vec2(g.size().x + 12., 22.),
                            if enabled {
                                Sense::click()
                            } else {
                                Sense::hover()
                            },
                        );
                        let hover = resp.hovered() && enabled;
                        let p = ui.painter();
                        p.rect(
                            r,
                            3.,
                            if hover { BG_RAISED } else { BG_CONTROL },
                            Stroke::new(1., if hover { BORDER_STRONG } else { BORDER_MID }),
                            StrokeKind::Inside,
                        );
                        let color = match (enabled, hover) {
                            (false, _) => INK_3,
                            (true, true) => INK_1,
                            _ => INK_2,
                        };
                        p.galley(r.center() - g.size() / 2., g, color);
                        if resp.clicked() {
                            self.devices_requested = false;
                            if self.worker.as_ref().is_some_and(|w| {
                                w.input_status() == Some(spectrum_wgpu::audio::InputStatus::Failed)
                            }) {
                                self.worker_config.0.clear();
                            }
                        }
                    });
                }
            });
        });

        // Display
        section(ui, false, false, |ui| {
            sec_head(ui, &t("display"), |ui| help(ui, &t("layersHint")));
            let items = [
                Seg::new(t("layerSpectrum"), s.spectrum),
                Seg::new(t("layerWaterfall"), s.waterfall),
            ];
            match seg(ui, "layers", &items, full) {
                // Either alone is fine; turning the last one off turns the other on.
                Some(0) => {
                    s.spectrum = !s.spectrum;
                    if !s.spectrum && !s.waterfall {
                        s.waterfall = true;
                    }
                }
                Some(1) => {
                    s.waterfall = !s.waterfall;
                    if !s.spectrum && !s.waterfall {
                        s.spectrum = true;
                    }
                }
                _ => {}
            }
            ui.add_space(8.);
            field_row(ui, &t("axis"), 34., |ui| {
                let items = [
                    Seg::new(t("axisLog"), s.axis == "log"),
                    Seg::new("Mel", s.axis == "mel"),
                ];
                if let Some(i) = seg(ui, "axis", &items, tiers_w) {
                    s.axis = ["log", "mel"][i].into();
                }
                help(ui, &t("axisHint"));
            });
        });

        // View
        section(ui, false, false, |ui| {
            sec_head(ui, &t("view"), |ui| help(ui, &t("desktop.viewHint")));
            // Spectrum alone holds the camera at the front.
            let flat = s.spectrum && !s.waterfall;
            let items = [
                Seg::new(t("front"), s.view == "front").kbd("1"),
                Seg::new(t("top"), s.view == "top" && !flat)
                    .kbd("2")
                    .enabled(!flat),
                Seg::new(t("perspective"), s.view == "perspective" && !flat)
                    .kbd("3")
                    .enabled(!flat),
            ];
            if let Some(i) = seg(ui, "view", &items, full) {
                s.view = ["front", "top", "perspective"][i].into();
            }
        });

        // Spectrum
        let mut clear = false;
        section(ui, false, false, |ui| {
            sec_head(ui, &t("spectrum"), |_| {});
            check(ui, &mut s.peaks, &t("peaks"));
            ui.add_space(4.);
            row_with_action(
                ui,
                |ui| {
                    check(ui, &mut s.max_hold, &t("maxHold"));
                },
                |ui| clear = button(ui, &t("clear"), "C", false, None).clicked(),
            );
            ui.add_space(4.);
            check(ui, &mut s.keys, &t("piano"));
            ui.add_space(4.);
            row_with_action(
                ui,
                |ui| {
                    check(ui, &mut s.tilt, &t("tilt"));
                },
                |ui| help(ui, &t("tiltHint")),
            );
        });

        // Look
        section(ui, false, false, |ui| {
            sec_head(ui, &t("look"), |ui| help(ui, &t("lookHint")));
            let control_w = full - LABEL_W - 8.;
            field_row(ui, &t("visualStyle"), 28., |ui| {
                let current = t(&format!("palette.{}", scene.settings.palette));
                let mut palette = scene.settings.palette.clone();
                let has_dadaspec = scene.has_dadaspec();
                select(ui, "palette", &current, control_w, |ui| {
                    let names: &[&str] = if has_dadaspec {
                        &["tungsten", "mel", "green", "dadaspec"]
                    } else {
                        &["tungsten", "mel", "green"]
                    };
                    for &name in names {
                        ui.selectable_value(
                            &mut palette,
                            name.into(),
                            t(&format!("palette.{name}")),
                        );
                    }
                });
                if palette != scene.settings.palette {
                    scene.set_palette(&palette);
                }
            });
            ui.add_space(8.);
            field_row(ui, &t("style"), 28., |ui| {
                let current = scene.look["styles"][&scene.settings.style]["label"]
                    .as_str()
                    .unwrap_or(&scene.settings.style)
                    .to_owned();
                select(ui, "style", &current, control_w, |ui| {
                    if let Some(styles) = scene.look["styles"].as_object() {
                        for (key, value) in styles {
                            ui.selectable_value(
                                &mut scene.settings.style,
                                key.clone(),
                                value["label"].as_str().unwrap_or(key),
                            );
                        }
                    }
                });
            });
        });

        let s = &mut scene.settings;
        // Time
        section(ui, false, true, |ui| {
            // The web slider runs over whole seconds from 1 to 12.
            s.seconds = s.seconds.round().clamp(1., 12.);
            let rows = (s.seconds * s.row_rate as f64).round();
            let tail = scene.look["size"]["tail"].as_f64().unwrap_or(4.5);
            let cap = ((spectrum_core::HISTORY_ROWS - 2) << (spectrum_core::LEVELS - 1)) as f64;
            let total = (rows * tail).round().min(cap) / s.row_rate as f64;
            let vars = |text: String| {
                text.replace("{s}", &format!("{}", s.seconds))
                    .replace("{total}", &format!("{total:.0}"))
            };
            let hint = vars(t("timeHint"));
            let value = vars(t("secondsValue"));
            sec_head(ui, &t("time"), |ui| help(ui, &hint));
            field_row(ui, &t("inFrame"), 28., |ui| {
                let w = 30.;
                slider(ui, &mut s.seconds, (1., 12.), ui.available_width() - w - 8.);
                let (r, _) = ui.allocate_exact_size(vec2(w, 24.), Sense::hover());
                // The fonts here have no narrow no-break space, and a monospace space is
                // a full digit wide: the value is set in the text face.
                ui.painter().text(
                    r.right_center(),
                    Align2::RIGHT_CENTER,
                    value.replace('\u{202f}', " "),
                    sans(12.),
                    INK_1,
                );
            });
        });
        if clear {
            if let Some(w) = &self.worker {
                w.clear_maximum();
            }
            g.clear_maximum();
        }
    }

    fn lang_switch(&mut self, ui: &mut Ui) {
        // Right to left: the last language first.
        for (i, (code, label)) in [("en", "EN"), ("zh", "\u{4e2d}")].into_iter().enumerate() {
            if i > 0 {
                let (r, _) = ui.allocate_exact_size(vec2(6., 16.), Sense::hover());
                ui.painter()
                    .text(r.center(), Align2::CENTER_CENTER, "/", sans(10.), INK_3);
            }
            let on = self.lang == code;
            let g = ui
                .painter()
                .layout_no_wrap(label.into(), sans(10.), Color32::PLACEHOLDER);
            let (r, resp) = ui.allocate_exact_size(g.size() + vec2(8., 4.), Sense::click());
            let hover = resp.hovered();
            if on {
                ui.painter().rect(
                    r,
                    2.,
                    BG_RAISED,
                    Stroke::new(1., BORDER_MID),
                    StrokeKind::Inside,
                );
            }
            let color = if on {
                INK_1
            } else if hover {
                INK_2
            } else {
                INK_3
            };
            ui.painter().galley(r.center() - g.size() / 2., g, color);
            if resp.clicked() {
                self.lang = code.into();
            }
        }
    }

    fn device_select(&mut self, ui: &mut Ui, width: f32, t: &dyn Fn(&str) -> String) {
        let previous = self.device_id.clone();
        let name = self
            .devices
            .iter()
            .find(|d| Some(&d.id) == self.device_id.as_ref())
            .map(|d| d.name.clone())
            .or_else(|| self.device_name.clone())
            .unwrap_or_else(|| t("deviceDefault"));
        let mut selected = previous.clone();
        select(ui, "device", &name, width, |ui| {
            ui.selectable_value(&mut selected, None, t("deviceDefault"));
            for d in &self.devices {
                ui.selectable_value(&mut selected, Some(d.id.clone()), &d.name);
            }
        });
        if selected != previous {
            self.cli_source = false;
            self.worker_config.0.clear();
            let info = self
                .devices
                .iter()
                .find(|d| Some(&d.id) == selected.as_ref());
            self.device = info.map(|d| d.index);
            self.device_name = info.map(|d| d.raw_name.clone());
            self.device_id = selected;
            self.error = None;
        }
    }

    fn panel_aux(&mut self, ui: &mut Ui, g: &mut Gpu, t: &dyn Fn(&str) -> String) {
        let full = ui.available_width();
        let rect = ui.available_rect_before_wrap();
        // The well behind the whole auxiliary part, painted under what follows.
        let backdrop = ui.painter().add(Shape::Noop);
        ui.painter().hline(rect.x_range(), rect.top(), hairline());
        let top = rect.top();

        // Analysis drawer
        let (head, resp) = ui.allocate_exact_size(vec2(full, 32.), Sense::click());
        if resp.clicked() {
            self.advanced_open = !self.advanced_open;
        }
        let hover = resp.hovered();
        let p = ui.painter();
        if hover {
            p.rect_filled(head, 0., Color32::from_white_alpha(5));
        }
        let color = if hover { INK_1 } else { INK_2 };
        let c = pos2(head.left() + 12. + 6., head.center().y);
        let points: Vec<_> = if self.advanced_open {
            [(2.5, 4.2), (6., 7.8), (9.5, 4.2)]
        } else {
            [(4.2, 2.5), (7.8, 6.), (4.2, 9.5)]
        }
        .into_iter()
        .map(|(x, y)| c + vec2(x - 6., y - 6.))
        .collect();
        p.add(Shape::line(points, Stroke::new(1.3, color)));
        p.text(
            pos2(head.left() + 12. + 12. + 6., head.center().y),
            Align2::LEFT_CENTER,
            t("advanced"),
            sans(12.),
            color,
        );
        if self.advanced_open {
            Frame::NONE
                .inner_margin(Margin {
                    left: 12,
                    right: 12,
                    top: 4,
                    bottom: 12,
                })
                .show(ui, |ui| {
                    field_row(ui, &t("renderScale"), 28., |ui| {
                        let w = (ui.available_width() - 8. - HELP).max(40.);
                        let current_pct = (self.render_scale.scale() * 100.0).round() as u32;
                        let selected = if self.render_scale.is_auto() {
                            format!("{} ({current_pct}%)", t("renderAuto"))
                        } else {
                            format!("{current_pct}%")
                        };
                        let mut mode = self.render_scale.mode().to_owned();
                        let choices = [
                            ("auto", t("renderAuto")),
                            ("1", "100%".into()),
                            ("0.85", "85%".into()),
                            ("0.7", "70%".into()),
                            ("0.5", "50%".into()),
                        ];
                        select(ui, "render-scale", &selected, w, |ui| {
                            for (value, label) in &choices {
                                ui.selectable_value(&mut mode, (*value).into(), label);
                            }
                        });
                        if mode != self.render_scale.mode() {
                            self.render_scale.set_mode(&mode);
                            g.set_render_scale(self.render_scale.scale());
                        }
                        help(ui, &t("tip.renderScale"));
                    });
                    ui.add_space(8.);
                    field_row(ui, &t("antiAliasing"), 34., |ui| {
                        let w = (ui.available_width() - 8. - HELP).max(40.);
                        let options = [(4u32, "4×".to_owned()), (1u32, t("msaaOff"))];
                        let items = [
                            Seg::new(&options[0].1, self.msaa == 4),
                            Seg::new(&options[1].1, self.msaa == 1),
                        ];
                        if let Some(i) = seg(ui, "msaa", &items, w) {
                            let next = options[i].0;
                            if next != self.msaa {
                                self.msaa = next;
                                g.set_msaa(next);
                            }
                        }
                        help(ui, &t("tip.msaa"));
                    });
                    ui.add_space(8.);
                    let s = &mut g.scene.as_mut().unwrap().settings;
                    field_row(ui, &t("rowRate"), 34., |ui| {
                        let w = (ui.available_width() - 8. - HELP).max(40.);
                        let rates = [60, 120, 240, 480];
                        let items = rates.map(|r| Seg::new(r.to_string(), s.row_rate == r));
                        if let Some(i) = seg(ui, "rows", &items, w) {
                            s.row_rate = rates[i];
                        }
                        help(ui, &t("rowRateHint"));
                    });
                    ui.add_space(8.);
                    field_row(ui, &t("bandSub"), 34., |ui| {
                        let w = (ui.available_width() - 8. - HELP).max(40.);
                        let subs = [1, 2, 4, 8];
                        let items = subs.map(|n| Seg::new(n.to_string(), s.sub == n));
                        if let Some(i) = seg(ui, "sub", &items, w) {
                            s.sub = subs[i];
                        }
                        help(ui, &t("bandSubHint"));
                    });
                    ui.add_space(8.);
                    row_with_action(
                        ui,
                        |ui| {
                            check(ui, &mut s.low_level, &t("lowLevel"));
                        },
                        |ui| help(ui, &t("lowLevelHint")),
                    );
                });
        }
        let r = ui.available_rect_before_wrap();
        ui.painter().hline(r.x_range(), r.top(), hairline());

        // Monitor switches
        Frame::NONE
            .inner_margin(Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.set_min_height(18.);
                    check(ui, &mut self.monitor, &t("monitor"));
                });
                ui.add_space(6.);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.;
                    let w = (ui.available_width() - 4. * 4.) / 5.;
                    let labels = [
                        t("monFrame"),
                        "GPU".into(),
                        "CPU".into(),
                        t("monMemory"),
                        t("monAudio"),
                    ];
                    for (on, label) in self.monitor_parts.iter_mut().zip(labels) {
                        chip(ui, &label, on, w, !self.monitor);
                    }
                });
            });
        let r = ui.available_rect_before_wrap();
        ui.painter().hline(r.x_range(), r.top(), hairline());

        // Freeze and hide
        let s = &mut g.scene.as_mut().unwrap().settings;
        Frame::NONE
            .inner_margin(Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let w = (ui.available_width() - 8.) / 2.;
                    if button(ui, &t("freeze"), &t("spaceKey"), s.hold, Some(w)).clicked() {
                        s.hold = !s.hold;
                    }
                    if button(ui, &t("hideUi"), "H", false, Some(w)).clicked() {
                        self.options.no_ui = true;
                        self.hide_ui = true;
                    }
                });
            });
        let area = Rect::from_min_max(
            pos2(rect.left(), top),
            pos2(rect.right(), ui.cursor().top()),
        );
        ui.painter()
            .set(backdrop, Shape::rect_filled(area, 0., BG_WELL));
    }
}
