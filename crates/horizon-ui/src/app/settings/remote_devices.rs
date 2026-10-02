use crate::theme;
use egui::{Align, Layout, RichText, Sense, Stroke, Ui, Vec2};

pub(super) struct Device {
    pub id: String,
    pub name: String,
    pub available: bool,
    pub busy: bool,
}
pub(super) enum Action {
    Refresh,
    Forget(String),
}
#[derive(Default)]
pub(super) struct RemoteDevicesPanel {
    pub devices: Vec<Device>,
    pub error: Option<String>,
    pub store_error: Option<String>,
    pub loading: bool,
    pub action: Option<Action>,
    confirming: Option<String>,
}
impl RemoteDevicesPanel {
    pub(super) fn render(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Paired devices").size(16.0).strong().color(theme::FG()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("Refresh").clicked() {
                    self.action = Some(Action::Refresh);
                }
            });
        });
        ui.add_space(6.0);
        super::dim_label(ui, "Your Apple TVs are remembered here, ready for the next cast.");
        ui.add_space(18.0);
        if let Some(error) = self.error.as_ref().or(self.store_error.as_ref()) {
            ui.colored_label(theme::PALETTE_RED(), error);
            ui.add_space(10.0);
        }
        if self.loading && self.devices.is_empty() {
            ui.horizontal(|ui| {
                ui.spinner();
                super::dim_label(ui, "Loading paired devices…");
            });
        } else if self.devices.is_empty() {
            super::section_card(ui, |ui| {
                ui.add_space(12.0);
                ui.vertical_centered(|ui| {
                    tv_icon(ui, false);
                    ui.add_space(12.0);
                    ui.label(RichText::new("Your next big screen").size(15.0).strong().color(theme::FG()));
                    ui.add_space(6.0);
                    super::dim_label(ui, "Use the cast icon on a panel or workspace to pair an Apple TV. It will appear here automatically.");
                });
                ui.add_space(12.0);
            });
        }
        for device in &self.devices {
            ui.push_id(&device.id, |ui| {
                super::section_card(ui, |ui| {
                    ui.horizontal(|ui| {
                        tv_icon(ui, device.busy);
                        ui.add_space(8.0);
                        ui.vertical(|ui| {
                            ui.add(
                                egui::Label::new(RichText::new(&device.name).size(14.0).strong().color(theme::FG()))
                                    .wrap(),
                            );
                            super::dim_label(ui, "Apple TV · Paired");
                        });
                    });
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        let (text, color) = if device.busy {
                            ("In use", theme::ACCENT())
                        } else if device.available {
                            ("Discovered on network", theme::PALETTE_GREEN())
                        } else {
                            ("Saved pairing", theme::FG_DIM())
                        };
                        ui.colored_label(color, RichText::new("●").size(8.0));
                        ui.label(RichText::new(text).size(11.0).color(color));
                    });
                    ui.add_space(10.0);
                    if self.confirming.as_deref() == Some(&device.id) {
                        super::dim_label(ui, "You will need the TV's code to pair again.");
                        ui.add_space(6.0);
                        ui.horizontal_wrapped(|ui| {
                            let button = egui::Button::new(RichText::new("Forget pairing").color(theme::PALETTE_RED()));
                            if ui.add_enabled(!device.busy, button).clicked() {
                                self.action = Some(Action::Forget(device.id.clone()));
                                self.confirming = None;
                            }
                            if ui.button("Cancel").clicked() {
                                self.confirming = None;
                            }
                        });
                    } else if ui
                        .add_enabled(!device.busy, egui::Button::new("Forget pairing"))
                        .clicked()
                    {
                        self.confirming = Some(device.id.clone());
                    }
                    if device.busy {
                        super::dim_label(ui, "Stop this TV's session before forgetting its pairing.");
                    }
                });
            });
        }
        super::dim_label(ui, "One session per Apple TV. Each TV can be managed independently.");
    }
}
fn tv_icon(ui: &mut Ui, active: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(36.0, 36.0), Sense::hover());
    let color = if active { theme::ACCENT() } else { theme::FG_SOFT() };
    let screen = egui::Rect::from_min_size(rect.min + Vec2::new(2.0, 5.0), Vec2::new(32.0, 22.0));
    ui.painter().rect(
        screen,
        4.0,
        theme::PANEL_BG_ALT(),
        Stroke::new(1.5, color),
        egui::StrokeKind::Inside,
    );
    ui.painter().line_segment(
        [
            rect.center() + Vec2::new(0.0, 10.0),
            rect.center() + Vec2::new(0.0, 15.0),
        ],
        Stroke::new(1.5, color),
    );
    ui.painter().line_segment(
        [
            rect.center() + Vec2::new(-6.0, 15.0),
            rect.center() + Vec2::new(6.0, 15.0),
        ],
        Stroke::new(1.5, color),
    );
}
