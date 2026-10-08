mod menu;

use egui::{
    Align, Align2, Atom, Button, Context, CornerRadius, FontId, Id, Layout, Order, Painter, Pos2, Rect, Sense, Stroke,
    UiBuilder, Vec2, WidgetInfo, WidgetType,
};

use crate::app::root_chrome::{
    DependenciesButton, ROOT_TOOLBAR_BUTTON_GAP, ROOT_TOOLBAR_BUTTON_HEIGHT, ROOT_TOOLBAR_FPS_WIDTH, RootToolbarLayout,
    root_toolbar_layout,
};
use crate::app::util;
use crate::app::{HorizonApp, TOOLBAR_HEIGHT};
use crate::{branding, theme};

const DEPENDENCIES_LABEL: &str = "Dependencies";
const DEPENDENCIES_MARK_SIZE: Vec2 = Vec2::new(14.0, 12.0);
const MARK_LABEL_GAP: f32 = 6.0;

impl HorizonApp {
    pub(in crate::app) fn render_toolbar(&mut self, ctx: &Context) {
        let viewport = util::viewport_local_rect(ctx);
        let layout = root_toolbar_layout(viewport);

        egui::Area::new(Id::new("toolbar"))
            .fixed_pos(viewport.min)
            .constrain(false)
            .order(Order::Tooltip)
            .show(ctx, |ui| {
                ui.set_min_size(Vec2::new(viewport.width(), TOOLBAR_HEIGHT));
                ui.set_max_size(Vec2::new(viewport.width(), TOOLBAR_HEIGHT));
                ui.painter().rect_filled(
                    Rect::from_min_size(viewport.min, Vec2::new(viewport.width(), TOOLBAR_HEIGHT)),
                    CornerRadius::ZERO,
                    theme::TITLEBAR_BG(),
                );
                ui.painter().line_segment(
                    [
                        Pos2::new(viewport.min.x, viewport.min.y + TOOLBAR_HEIGHT),
                        Pos2::new(viewport.max.x, viewport.min.y + TOOLBAR_HEIGHT),
                    ],
                    Stroke::new(1.0_f32, theme::alpha(theme::BORDER_SUBTLE(), 170)),
                );

                Self::render_toolbar_brand(ui, &layout);
                self.render_toolbar_search_rect(ui, &layout);
                self.render_toolbar_actions(ui, &layout);
            });
    }

    fn render_toolbar_brand(ui: &mut egui::Ui, layout: &RootToolbarLayout) {
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(layout.brand_rect)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.label(
                    egui::RichText::new(branding::APP_NAME)
                        .color(theme::FG())
                        .size(14.0)
                        .strong(),
                );
                if layout.items.tagline {
                    ui.add_space(ROOT_TOOLBAR_BUTTON_GAP);
                    ui.label(
                        egui::RichText::new(branding::APP_TAGLINE)
                            .color(theme::FG_DIM())
                            .size(10.5),
                    );
                }
            },
        );
    }

    fn render_toolbar_search_rect(&mut self, ui: &mut egui::Ui, layout: &RootToolbarLayout) {
        let mut search_ui = ui.new_child(
            UiBuilder::new()
                .max_rect(layout.search_rect)
                .layout(Layout::left_to_right(Align::Center)),
        );
        self.render_toolbar_search(&mut search_ui);
    }

    fn render_toolbar_actions(&mut self, ui: &mut egui::Ui, layout: &RootToolbarLayout) {
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(layout.actions_rect)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = ROOT_TOOLBAR_BUTTON_GAP;

                if layout.items.fps_meter {
                    self.render_toolbar_fps_meter(ui);
                }
                self.render_toolbar_dependencies_button(ui, layout.items.dependencies);
                self.render_toolbar_menu(ui);
            },
        );
    }

    fn render_toolbar_fps_meter(&self, ui: &mut egui::Ui) {
        let stats = self.frame_stats.snapshot();
        // Classify the number on screen, so "60" never shows in the below-60 color.
        let shown_fps = stats.fps.round();
        let value = if stats.sample_count == 0 {
            "0".to_string()
        } else {
            format!("{shown_fps:.0}")
        };
        let accent = if stats.sample_count == 0 {
            theme::BORDER_SUBTLE()
        } else if shown_fps >= 100.0 {
            theme::PALETTE_GREEN()
        } else if shown_fps >= 60.0 {
            theme::ACCENT()
        } else {
            theme::PALETTE_RED()
        };
        let (rect, response) = ui.allocate_exact_size(Vec2::new(fps_meter_width(), 24.0), Sense::hover());
        let painter = ui.painter();
        let stroke_color = theme::alpha(theme::blend(theme::BORDER_SUBTLE(), accent, 0.36), 220);
        let fill_color = theme::alpha(theme::blend(theme::PANEL_BG_ALT(), accent, 0.10), 232);
        let dot_center = Pos2::new(rect.min.x + 10.0, rect.center().y);

        painter.rect_filled(rect, CornerRadius::same(10), fill_color);
        painter.rect_stroke(
            rect,
            CornerRadius::same(10),
            Stroke::new(1.0_f32, stroke_color),
            egui::StrokeKind::Outside,
        );
        painter.circle_filled(dot_center, 3.0, theme::alpha(accent, 230));
        painter.text(
            Pos2::new(rect.min.x + 18.0, rect.center().y),
            Align2::LEFT_CENTER,
            value,
            FontId::monospace(11.5),
            theme::FG(),
        );
        painter.text(
            Pos2::new(rect.max.x - 8.0, rect.center().y),
            Align2::RIGHT_CENTER,
            "fps",
            FontId::proportional(8.5),
            theme::alpha(theme::FG_DIM(), 220),
        );

        let tooltip = if stats.sample_count == 0 {
            "Idle. The meter measures while Horizon renders continuously, such as during panning, animation or streaming output.".to_string()
        } else {
            format!(
                "{shown_fps:.0} FPS over the last {} frames ({:.2} ms average, {:.1} ms slowest)",
                stats.sample_count, stats.frame_time_ms, stats.slowest_frame_time_ms
            )
        };
        let _ = response.on_hover_text(tooltip);
    }

    fn render_toolbar_dependencies_button(&mut self, ui: &mut egui::Ui, presentation: DependenciesButton) {
        let mark_id = Id::new("toolbar-dependencies-mark");
        let mark = Atom::custom(mark_id, DEPENDENCIES_MARK_SIZE);
        let button = match presentation {
            DependenciesButton::Labeled => Button::new((mark, util::primary_label(DEPENDENCIES_LABEL))),
            DependenciesButton::MarkOnly => Button::new(mark),
        };
        let atoms = util::primary_frame(button.gap(MARK_LABEL_GAP))
            .min_size(Vec2::new(presentation.width(), ROOT_TOOLBAR_BUTTON_HEIGHT))
            .atom_ui(ui);
        if let Some(rect) = atoms.rect(mark_id) {
            paint_dependencies_mark(ui.painter(), rect);
        }

        let mut response = atoms.response;
        if presentation == DependenciesButton::MarkOnly {
            response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), DEPENDENCIES_LABEL));
            response = response.on_hover_text(DEPENDENCIES_LABEL);
        }
        if response.clicked() {
            self.open_dependencies_panel(ui.ctx());
        }
    }
}

fn fps_meter_width() -> f32 {
    ROOT_TOOLBAR_FPS_WIDTH
}

/// Three linked nodes: one on the left joined to two on the right.
fn paint_dependencies_mark(painter: &Painter, rect: Rect) {
    const NODE_RADIUS: f32 = 2.2;

    let color = theme::ACCENT();
    let root = Pos2::new(rect.left() + NODE_RADIUS, rect.center().y);
    let upper = Pos2::new(rect.right() - NODE_RADIUS, rect.top() + NODE_RADIUS);
    let lower = Pos2::new(rect.right() - NODE_RADIUS, rect.bottom() - NODE_RADIUS);
    let link = Stroke::new(1.3_f32, color);

    painter.line_segment([root, upper], link);
    painter.line_segment([root, lower], link);
    for node in [root, upper, lower] {
        painter.circle_filled(node, NODE_RADIUS, color);
    }
}

#[cfg(test)]
mod tests {
    use egui::{Pos2, Rect, Vec2, epaint::Shape};
    use horizon_core::{RuntimeState, StartupDecision};

    use crate::app::root_chrome::{
        DependenciesButton, ROOT_TOOLBAR_BUTTON_GAP, ROOT_TOOLBAR_MENU_WIDTH, root_toolbar_layout,
    };
    use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app_with_startup};

    /// Global egui button padding; a label must keep it inside its button.
    const BUTTON_PADDING: Vec2 = Vec2::new(12.0, 0.0);

    fn label_rect(output: &egui::FullOutput, label: &str) -> Option<Rect> {
        output.shapes.iter().find_map(|shape| match &shape.shape {
            Shape::Text(text) if text.galley.job.text == label => {
                Some(Rect::from_min_size(text.pos, text.galley.size()))
            }
            _ => None,
        })
    }

    #[test]
    fn toolbar_button_labels_fit_inside_the_widths_the_layout_reserves() {
        let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        });
        app.root_viewport_stabilizer = None;

        for width in [480.0, 760.0, 1024.0, 1680.0] {
            let mut output = run_app_frame_with_input(&ctx, &mut app, raw_input([width, 900.0], None));
            for _ in 0..2 {
                output = run_app_frame_with_input(&ctx, &mut app, raw_input([width, 900.0], None));
            }
            let layout = root_toolbar_layout(Rect::from_min_max(Pos2::ZERO, Pos2::new(width, 900.0)));
            let actions = layout.actions_rect;
            let menu_slot = Rect::from_x_y_ranges(
                actions.max.x - ROOT_TOOLBAR_MENU_WIDTH..=actions.max.x,
                actions.y_range(),
            );
            let dependencies_right = menu_slot.min.x - ROOT_TOOLBAR_BUTTON_GAP;
            let dependencies_slot = Rect::from_x_y_ranges(
                dependencies_right - layout.items.dependencies.width()..=dependencies_right,
                actions.y_range(),
            );

            let menu = label_rect(&output, "Menu").expect("Menu label");
            assert!(
                menu_slot.shrink2(BUTTON_PADDING).contains_rect(menu),
                "{width}: {menu:?}"
            );
            let dependencies = label_rect(&output, "Dependencies");
            match layout.items.dependencies {
                DependenciesButton::Labeled => {
                    let dependencies = dependencies.expect("Dependencies label");
                    assert!(
                        dependencies_slot.shrink2(BUTTON_PADDING).contains_rect(dependencies),
                        "{width}: {dependencies:?}"
                    );
                }
                DependenciesButton::MarkOnly => assert_eq!(dependencies, None, "{width}"),
            }
        }
    }
}
