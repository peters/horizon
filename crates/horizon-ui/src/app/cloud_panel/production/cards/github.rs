//! A cloud's GitHub sign-in on its card: the device code to approve, the browser
//! step of an automatic sign-in, and a one-line outcome.
use super::super::Runtime;
use crate::{
    app::util::{chrome_button, primary_button},
    theme,
};
use egui::{RichText, Stroke, vec2};
use horizon_core::cloud_runtime::github::{self, Prompt, requests::Decision};

/// The height of the sign-in box, with the gap below it.
pub(super) const PROMPT_HEIGHT: f32 = 168.0;

/// Whether the cloud needs its GitHub overlay: a sign-in that its panels hide, or
/// access requests and a refused decision.
pub(super) fn overlay(runtime: &Runtime, body: bool) -> bool {
    (!body && waiting(runtime)) || !runtime.github_requests.list.is_empty() || runtime.github_requests.refused.is_some()
}

/// Whether the card shows the sign-in box, which takes [`PROMPT_HEIGHT`] from the output.
pub(super) fn waiting(runtime: &Runtime) -> bool {
    matches!(runtime.github, Some(Prompt::Device { .. } | Prompt::Web { .. })) && runtime.receiver.is_some()
}

/// The sign-in box. `cloud_id` names the cloud a Skip applies to.
pub(super) fn prompt(ui: &mut egui::Ui, cloud_id: &str, runtime: &Runtime) {
    let Some(prompt) = &runtime.github else {
        return;
    };
    egui::Frame::new()
        .fill(theme::alpha(theme::ACCENT(), 18))
        .stroke(Stroke::new(1.0, theme::alpha(theme::ACCENT(), 120)))
        .corner_radius(12)
        .inner_margin(egui::Margin::symmetric(16, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(PROMPT_HEIGHT - 40.0);
            match prompt {
                Prompt::Device {
                    user_code,
                    verification_uri,
                    ..
                } => device(ui, cloud_id, user_code, verification_uri),
                Prompt::Web { url } => web(ui, cloud_id, url),
                Prompt::Connected { .. } | Prompt::Ended(_) => {}
            }
        });
    ui.add_space(12.0);
}

fn device(ui: &mut egui::Ui, cloud_id: &str, user_code: &str, verification_uri: &str) {
    ui.label(
        RichText::new("Approve GitHub access for this cloud")
            .size(15.0)
            .strong()
            .color(theme::FG()),
    );
    ui.label(
        RichText::new("Open GitHub, paste the code and click Authorize. Horizon copies the code for you.")
            .size(12.5)
            .color(theme::FG_DIM()),
    );
    ui.add_space(6.0);
    ui.label(
        RichText::new(user_code)
            .monospace()
            .size(26.0)
            .strong()
            .color(theme::FG()),
    );
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui
            .add(primary_button("Open GitHub").min_size(vec2(120.0, 30.0)))
            .clicked()
        {
            ui.ctx().copy_text(user_code.to_owned());
            if let Err(error) = horizon_core::open_url(verification_uri) {
                tracing::warn!(%error, "could not open the GitHub device page");
            }
        }
        if ui
            .add(chrome_button("Skip: no GitHub for this cloud").min_size(vec2(0.0, 30.0)))
            .clicked()
        {
            github::skip(cloud_id);
        }
    });
}

fn web(ui: &mut egui::Ui, cloud_id: &str, url: &str) {
    ui.label(
        RichText::new("Connecting GitHub for this cloud")
            .size(15.0)
            .strong()
            .color(theme::FG()),
    );
    ui.label(
        RichText::new(
            "Horizon opened GitHub in your browser; if it did not, click Open GitHub. If GitHub asks you \
             to sign in, or to authorize the app the first time, do it there; the page then returns to \
             Horizon by itself.",
        )
        .size(12.5)
        .color(theme::FG_DIM()),
    );
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        // Also the way back when the browser did not open.
        if ui
            .add(primary_button("Open GitHub").min_size(vec2(120.0, 30.0)))
            .clicked()
            && let Err(error) = horizon_core::open_url(url)
        {
            tracing::warn!(%error, "could not open the GitHub sign-in page");
        }
        if ui
            .add(chrome_button("Skip: no GitHub for this cloud").min_size(vec2(0.0, 30.0)))
            .clicked()
        {
            github::skip(cloud_id);
        }
    });
}

/// The width of the request box over the cloud's panels.
pub(super) const REQUESTS_WIDTH: f32 = 380.0;
/// At most this many requests show at once; the rest wait their turn.
const SHOWN: usize = 3;

/// The access requests of the cloud's agents, newest last. Returns the request and
/// the decision when the person clicked one; the buttons wait while the worker is
/// still answering an earlier exchange.
pub(super) fn requests(ui: &mut egui::Ui, runtime: &Runtime) -> Option<(String, Decision)> {
    let state = &runtime.github_requests;
    let mut chosen = None;
    ui.set_width(REQUESTS_WIDTH);
    for request in state.list.iter().take(SHOWN) {
        egui::Frame::new()
            .fill(theme::PANEL_BG())
            .stroke(Stroke::new(1.0, theme::alpha(theme::ACCENT(), 150)))
            .corner_radius(12)
            .inner_margin(egui::Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let who = match (request.agent.as_str(), request.session.as_str()) {
                    ("", "") => "An agent".to_owned(),
                    (agent, "") | ("", agent) => agent.to_owned(),
                    (agent, session) => format!("{agent} in {session}"),
                };
                ui.label(
                    RichText::new(format!("{who} asks for GitHub access"))
                        .size(12.0)
                        .color(theme::FG_DIM()),
                );
                let verb = if request.access == "push" { "Push to" } else { "Read" };
                ui.label(
                    RichText::new(format!("{verb} {}", request.repository))
                        .size(15.0)
                        .strong()
                        .color(theme::FG()),
                );
                if !request.reason.is_empty() {
                    ui.label(
                        RichText::new(format!("\u{201c}{}\u{201d}", request.reason))
                            .size(12.5)
                            .color(theme::FG_SOFT()),
                    );
                }
                ui.add_space(4.0);
                ui.add_enabled_ui(!state.busy(), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        if ui.add(primary_button("Allow for this task")).clicked() {
                            chosen = Some((request.id.clone(), Decision::AllowTask));
                        }
                        if ui.add(chrome_button("Always for this cloud")).clicked() {
                            chosen = Some((request.id.clone(), Decision::AllowCloud));
                        }
                        if ui.add(chrome_button("Deny")).clicked() {
                            chosen = Some((request.id.clone(), Decision::Deny));
                        }
                    })
                });
            });
        ui.add_space(8.0);
    }
    if state.list.len() > SHOWN {
        ui.label(
            RichText::new(format!("{} more requests wait.", state.list.len() - SHOWN))
                .size(12.0)
                .color(theme::FG_DIM()),
        );
    }
    if let Some(refused) = &state.refused {
        ui.label(RichText::new(refused).size(12.0).color(theme::PALETTE_RED()));
    }
    chosen
}

/// **Connect GitHub again**, once this cloud's GitHub step reported its outcome. A click
/// marks the cloud for a new sign-in; the caller then reconnects it.
pub(super) fn renew_button(ui: &mut egui::Ui, cloud_id: &str, runtime: &Runtime) -> bool {
    // Not for access a worker kept after GitHub was disconnected here: nothing could sign
    // it in again.
    if !matches!(
        runtime.github,
        Some(Prompt::Connected { renewable: true, .. } | Prompt::Ended(_))
    ) {
        return false;
    }
    let clicked = ui
        .add(chrome_button("Connect GitHub again"))
        .on_hover_text("Reconnect this cloud and sign in to GitHub again, for example to add a repository.")
        .clicked();
    if clicked {
        github::renew(cloud_id);
    }
    clicked
}

/// One line for the steps card: who the cloud acts as on GitHub, or why it has no access.
pub(super) fn summary(runtime: &Runtime) -> Option<(String, bool)> {
    match runtime.github.as_ref()? {
        Prompt::Connected {
            login, repositories, ..
        } => Some((
            format!(
                "GitHub: signed in as {login} · {} {}",
                repositories.len(),
                if repositories.len() == 1 {
                    "repository"
                } else {
                    "repositories"
                }
            ),
            true,
        )),
        Prompt::Ended(reason) => Some((format!("GitHub: {reason}"), false)),
        Prompt::Device { .. } | Prompt::Web { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_egui::DiscardTextures;

    #[test]
    fn the_steps_card_names_the_account_or_why_there_is_no_access() {
        let mut runtime = Runtime::default();
        assert_eq!(summary(&runtime), None);
        runtime.github = Some(Prompt::Connected {
            login: "octo-cat".into(),
            repositories: vec!["acme/web".into(), "acme/api".into()],
            requests: true,
            renewable: true,
        });
        assert_eq!(
            summary(&runtime),
            Some(("GitHub: signed in as octo-cat · 2 repositories".into(), true))
        );
        runtime.github = Some(Prompt::Ended("Skipped: this cloud has no GitHub access.".into()));
        assert_eq!(
            summary(&runtime),
            Some(("GitHub: Skipped: this cloud has no GitHub access.".into(), false))
        );
    }

    #[test]
    fn requests_show_who_asks_what_and_why() {
        use horizon_core::cloud_runtime::github::requests::Request;
        let mut runtime = Runtime::default();
        runtime.github_requests.list = (0..5)
            .map(|n| Request {
                id: format!("r{n}"),
                repository: "acme/design-system".into(),
                access: "push".into(),
                reason: "The shared Button needs the same fix".into(),
                session: "panel-2".into(),
                agent: "claude".into(),
            })
            .collect();
        let texts: Vec<String> = egui::Context::default()
            .run_ui(egui::RawInput::default(), |ui| {
                assert!(requests(ui, &runtime).is_none(), "nothing is chosen without a click");
            })
            .discard_textures()
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        assert!(
            texts
                .iter()
                .any(|text| text == "claude in panel-2 asks for GitHub access")
        );
        assert_eq!(
            texts
                .iter()
                .filter(|text| *text == "Push to acme/design-system")
                .count(),
            SHOWN
        );
        assert!(texts.iter().any(|text| text == "2 more requests wait."));
        assert!(texts.iter().any(|text| text == "Allow for this task"));
    }

    #[test]
    fn a_cloud_with_panels_shows_its_sign_in_and_requests_over_them() {
        let mut runtime = Runtime {
            github: Some(Prompt::Web {
                url: "https://github.com/login/oauth/authorize".into(),
            }),
            ..Runtime::default()
        };
        let (_sender, receiver) = std::sync::mpsc::channel();
        runtime.receiver = Some(receiver);
        assert!(
            overlay(&runtime, false),
            "the panels hide the body, so the overlay shows the sign-in"
        );
        assert!(!overlay(&runtime, true), "the body shows the sign-in itself");
        runtime.receiver = None;
        assert!(!overlay(&runtime, false));
        runtime.github_requests.refused = Some("This request expired.".into());
        assert!(
            overlay(&runtime, true),
            "requests and refusals show with or without panels"
        );
    }

    #[test]
    fn the_sign_in_box_shows_only_while_the_deployment_waits() {
        let mut runtime = Runtime {
            github: Some(Prompt::Device {
                user_code: "WDJB-MJHT".into(),
                verification_uri: "https://github.com/login/device".into(),
                expires_at: std::time::SystemTime::now(),
            }),
            ..Runtime::default()
        };
        assert!(!waiting(&runtime), "a finished deployment no longer waits for the code");
        let (_sender, receiver) = std::sync::mpsc::channel();
        runtime.receiver = Some(receiver);
        assert!(waiting(&runtime));
        assert_eq!(summary(&runtime), None);
    }
}
