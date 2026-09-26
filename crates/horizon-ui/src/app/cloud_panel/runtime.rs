//! Presentational deployment replay. No provider, Docker or SSH calls are made.
use std::time::{Duration, Instant};

use egui::{ColorImage, Id, Order, Pos2, RichText, Stroke, TextureOptions, Vec2};
use horizon_core::cloud_panel::{CloudConfig, CloudGroup, RUNTIME_HEIGHT, RUNTIME_WIDTH};
use horizon_core::{PanelKind, WorkspaceLayout};

use super::HorizonApp;
use crate::app::view::canvas_scene_transform;
use crate::theme;

#[derive(Default)]
pub(super) struct DemoDeployment {
    started: Option<Instant>,
    finished: bool,
    launched: bool,
}

impl DemoDeployment {
    pub(super) fn scenario(id: u32) -> Self {
        Self {
            started: (id != 101).then(|| {
                Instant::now()
                    .checked_sub(Duration::from_secs(if id == 102 { 4 } else { 0 }))
                    .unwrap_or_else(Instant::now)
            }),
            finished: id == 101,
            launched: id == 101,
        }
    }

    fn stage(&self) -> usize {
        if self.finished {
            return 3;
        }
        self.started.map_or(0, |t| match t.elapsed().as_secs() {
            0..=3 => 0,
            4..=8 => 1,
            9..=11 => 2,
            _ => 3,
        })
    }
}

enum Action {
    Deploy(u32),
    Profile(u32, String),
    Layout(u32, Option<WorkspaceLayout>),
    Fullscreen(u32),
}

impl HorizonApp {
    pub(in crate::app::cloud_panel) fn ensure_cloud_provider_logo(&mut self, ctx: &egui::Context) {
        if self.cloud_prototype.provider_logo.is_none()
            && let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/runpod-logo.png"))
        {
            let image = ColorImage::from_rgba_unmultiplied([icon.width as usize, icon.height as usize], &icon.rgba);
            self.cloud_prototype.provider_logo = Some(ctx.load_texture("cloud-runpod", image, TextureOptions::LINEAR));
        }
    }

    pub(super) fn advance_cloud_deployments(&mut self, ctx: &egui::Context) {
        let mut launch = Vec::new();
        for (&id, demo) in &mut self.cloud_prototype.deployments {
            if demo.started.is_some() && !demo.finished {
                ctx.request_repaint_after(Duration::from_millis(100));
                if demo.stage() == 3 {
                    demo.finished = true;
                    if !demo.launched {
                        demo.launched = true;
                        launch.push(id);
                    }
                }
            }
        }
        for id in launch {
            let agents = if id == 102 {
                [PanelKind::Claude, PanelKind::Grok]
            } else {
                [PanelKind::Claude, PanelKind::Codex]
            };
            for kind in agents {
                self.cloud_add_panel(ctx, id, kind, None);
            }
            if id == 103 {
                let endpoint = std::env::var("HORIZON_CLOUD_MOCK_VNC").ok();
                self.cloud_add_panel(ctx, id, PanelKind::Device, endpoint);
                self.cloud_add_panel(ctx, id, PanelKind::Browser, Some("https://www.daytona.io".into()));
            }
            self.cloud_overview(ctx);
        }
    }

    pub(in crate::app) fn render_cloud_runtimes(&mut self, ctx: &egui::Context) {
        if !self.cloud_prototype.ready {
            return;
        }
        self.ensure_cloud_provider_logo(ctx);
        let Some(profiles) = self.cloud_prototype.profiles.as_ref() else {
            return;
        };
        let canvas = self.canvas_rect(ctx);
        let transform = canvas_scene_transform(canvas, self.canvas_view);
        let clip = transform.inverse() * canvas;
        let mut action = None;
        for group in &self.cloud_prototype.groups.0 {
            if self
                .cloud_prototype
                .fullscreen
                .as_ref()
                .is_some_and(|view| view.id != group.issue)
            {
                continue;
            }
            let position = Pos2::from(group.runtime_bounds().0);
            let demo = self
                .cloud_prototype
                .deployments
                .entry(group.issue)
                .or_insert_with(|| DemoDeployment {
                    finished: !group.panels.is_empty(),
                    launched: !group.panels.is_empty(),
                    ..DemoDeployment::default()
                });
            egui::Area::new(Id::new(("cloud-runtime", group.issue)))
                .order(Order::Middle)
                .fixed_pos(position)
                .constrain(false)
                .show(ctx, |ui| {
                    ctx.set_transform_layer(ui.layer_id(), transform);
                    ui.set_clip_rect(clip);
                    egui::Frame::new()
                        .fill(theme::BG_ELEVATED())
                        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
                        .corner_radius(14)
                        .inner_margin(18)
                        .show(ui, |ui| {
                            ui.set_width(RUNTIME_WIDTH - 36.0);
                            ui.spacing_mut().item_spacing.y = 4.0;
                            ui.set_min_height(RUNTIME_HEIGHT - 36.0);
                            egui::ScrollArea::vertical()
                                .id_salt(("runtime-scroll", group.issue))
                                .max_height(RUNTIME_HEIGHT - 106.0)
                                .show(ui, |ui| {
                                    runtime_heading(ui, group, self.cloud_prototype.provider_logo.as_ref());
                                    runtime_options(
                                        ui,
                                        group,
                                        profiles,
                                        demo,
                                        self.cloud_prototype.fullscreen.is_some(),
                                        &mut action,
                                    );
                                    ui.add_space(6.0);
                                    ui.separator();
                                    ui.add_space(6.0);
                                    deployment_steps(ui, group, demo);
                                    ui.add_space(6.0);
                                });
                            deployment_button(ui, group, demo, &mut action);
                        });
                });
        }
        if let Some(action) = action {
            self.cloud_runtime_action(ctx, action);
        }
    }

    fn cloud_runtime_action(&mut self, ctx: &egui::Context, action: Action) {
        let id = match &action {
            Action::Deploy(id) | Action::Profile(id, _) | Action::Layout(id, _) | Action::Fullscreen(id) => *id,
        };
        let Some(index) = self.cloud_prototype.groups.0.iter().position(|g| g.issue == id) else {
            return;
        };
        match action {
            Action::Fullscreen(_) => self.toggle_cloud_fullscreen(ctx, id),
            Action::Deploy(_) => {
                let demo = self.cloud_prototype.deployments.entry(id).or_default();
                demo.started = Some(Instant::now());
                demo.finished = false;
            }
            Action::Profile(_, name) => {
                if let Some(profile) = self
                    .cloud_prototype
                    .profiles
                    .as_ref()
                    .and_then(|p| p.profiles.get(&name))
                {
                    let environment = &mut self.cloud_prototype.groups.0[index].environment;
                    environment.profile = Some(name);
                    environment.provider = Some(profile.provider.clone());
                    environment.image.clone_from(&profile.image);
                    let demo = self.cloud_prototype.deployments.entry(id).or_default();
                    demo.started = None;
                    demo.finished = false;
                }
            }
            Action::Layout(_, layout) => {
                self.cloud_prototype.groups.0[index].set_layout(&mut self.board, layout);
                self.cloud_prototype.groups.make_room(&mut self.board, index);
            }
        }
        self.save_cloud_prototype();
    }
}

fn deployment_button(ui: &mut egui::Ui, group: &CloudGroup, demo: &DemoDeployment, action: &mut Option<Action>) {
    let running = demo.started.is_some() && !demo.finished;
    let label = if running {
        "Deployment in progress…"
    } else if demo.finished {
        "Replay deployment"
    } else {
        "Deploy cloud"
    };
    if ui
        .add_enabled(
            !running,
            egui::Button::new(label)
                .min_size(Vec2::new(264.0, 34.0))
                .fill(theme::blend(theme::PANEL_BG(), theme::ACCENT(), 0.20)),
        )
        .clicked()
    {
        *action = Some(Action::Deploy(group.issue));
    }
    ui.add_space(7.0);
    ui.label(
        RichText::new("Simulated cloud · real local panels")
            .size(14.0)
            .color(theme::FG_DIM()),
    );
}

pub(in crate::app::cloud_panel) fn runtime_heading(
    ui: &mut egui::Ui,
    group: &CloudGroup,
    logo: Option<&egui::TextureHandle>,
) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("CLOUD RUNTIME")
                .size(14.0)
                .strong()
                .color(theme::FG_SOFT()),
        );
        if group.remote.is_none() {
            ui.label(RichText::new("DESIGN FIXTURE").size(12.0).color(theme::PALETTE_CYAN()));
        }
    });
    ui.add_space(10.0);
    if group.environment.provider.as_deref() == Some("runpod") {
        if let Some(logo) = logo {
            ui.add(egui::Image::new((logo.id(), Vec2::new(141.0, 32.0))).tint(theme::FG()));
        }
        ui.label(RichText::new("runpod.io").size(15.0).color(theme::FG_DIM()));
    } else {
        let (name, domain) = match group.environment.provider.as_deref() {
            Some("daytona") => ("Daytona", "daytona.io"),
            Some("fly") => ("Fly.io", "fly.io"),
            other => (other.unwrap_or("Runtime"), ""),
        };
        ui.label(RichText::new(name).size(26.0).strong());
        if !domain.is_empty() {
            ui.label(RichText::new(domain).size(15.0).color(theme::FG_DIM()));
        }
    }
    ui.add_space(8.0);
}

fn runtime_options(
    ui: &mut egui::Ui,
    group: &CloudGroup,
    config: &CloudConfig,
    demo: &DemoDeployment,
    fullscreen: bool,
    action: &mut Option<Action>,
) {
    let current = group.environment.profile.as_deref().unwrap_or(&config.default);
    ui.label(RichText::new("Runtime profile").size(14.0).color(theme::FG_SOFT()));
    ui.add_enabled_ui(demo.started.is_none() || demo.finished, |ui| {
        egui::ComboBox::from_id_salt(("profile", group.issue))
            .width(244.0)
            .selected_text(format!(
                "{} / {current}",
                group.environment.provider.as_deref().unwrap_or("local")
            ))
            .show_ui(ui, |ui| {
                for (name, profile) in &config.profiles {
                    if ui
                        .selectable_label(name == current, format!("{} / {name}", profile.provider))
                        .clicked()
                    {
                        *action = Some(Action::Profile(group.issue, name.clone()));
                    }
                }
            });
    });
    if let Some(profile) = config.profiles.get(current) {
        ui.label(
            RichText::new(format!(
                "{} vCPU  ·  {} GB  ·  {}",
                profile.cpu,
                profile.memory_gb,
                if profile.gpu { "GPU" } else { "CPU only" }
            ))
            .size(14.0)
            .color(theme::FG_SOFT()),
        );
        ui.label(
            RichText::new(&profile.image)
                .monospace()
                .size(14.0)
                .color(theme::FG_DIM()),
        );
    }
    ui.label(
        RichText::new("From .horizon/cloud.yml")
            .size(14.0)
            .color(theme::FG_DIM()),
    );
    ui.add_space(10.0);
    ui.label(RichText::new("Panel layout").size(14.0).color(theme::FG_SOFT()));
    let accent = theme::workspace_accent(group.issue.saturating_sub(101) as usize);
    ui.horizontal(|ui| {
        let mut layout = group.layout;
        if crate::app::workspace::workspace_layout_buttons(ui, &mut layout, accent) {
            *action = Some(Action::Layout(group.issue, layout));
        }
    });
    if ui
        .add(
            egui::Button::new(if fullscreen { "Exit full screen" } else { "Full screen" })
                .min_size(Vec2::new(264.0, 30.0)),
        )
        .on_hover_text("Show only this cloud; Escape returns to the overview")
        .clicked()
    {
        *action = Some(Action::Fullscreen(group.issue));
    }
}

fn deployment_steps(ui: &mut egui::Ui, group: &CloudGroup, demo: &DemoDeployment) {
    let stage = demo.stage();
    let active = demo.started.is_some() && !demo.finished;
    let agents = match group.issue {
        101 => "Browser preview",
        102 => "Claude + Grok",
        103 => "Claude + Codex · VNC · Browser",
        _ => "Claude + Codex",
    };
    for (index, (title, detail)) in [
        ("Provision a worker", "Reserve compute and attach storage"),
        ("Deploy Docker image", group.environment.image.as_str()),
        ("Start workspace", agents),
    ]
    .into_iter()
    .enumerate()
    {
        let status = if stage > index {
            "Complete"
        } else if active && stage == index {
            "Running…"
        } else {
            "Queued"
        };
        let color = if stage > index {
            theme::PALETTE_CYAN()
        } else {
            theme::FG_SOFT()
        };
        ui.label(
            RichText::new(format!("{}  {title}", index + 1))
                .size(14.0)
                .strong()
                .color(color),
        );
        ui.label(
            RichText::new(format!("{status} · {detail}"))
                .size(14.0)
                .color(theme::FG_DIM()),
        );
        egui::CollapsingHeader::new("Verbose output").id_salt((group.issue, index)).show(ui, |ui| {
            let output = if stage < index || (!active && !demo.finished) { "Waiting for deployment." }
            else { match index {
                0 => "[mock] Resolving repository profile\n[mock] Request accepted by provider\n[mock] Allocating worker and volume\n[mock] Worker heartbeat received",
                1 => "[mock] Pulling image manifest\n[mock] Downloading layers: 3 / 3\n[mock] Starting container\n[mock] SSH and desktop health checks OK",
                _ => "[mock] Preparing task checkout\n[mock] Opening agent sessions\n[mock] New-device sign-in if required\n[local] Real panels handle their own login",
            }};
            egui::ScrollArea::vertical().id_salt(("log", group.issue, index)).max_height(108.0).show(ui, |ui| {
                ui.label(RichText::new(output).monospace().size(14.0).color(theme::FG_SOFT()));
            });
        });
        ui.add_space(10.0);
    }
}
