//! A cloud's GitHub sign-in on its card: the device code to approve, the browser
//! step of an automatic sign-in, and a one-line outcome.
use super::super::Runtime;
use crate::{
    app::util::{chrome_button, primary_button},
    theme,
};
use egui::{RichText, Stroke, vec2};
use horizon_core::cloud_runtime::github::{self, Prompt};

/// The height of the sign-in box, with the gap below it.
pub(super) const PROMPT_HEIGHT: f32 = 168.0;

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
                Prompt::Web { .. } => web(ui, cloud_id),
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

fn web(ui: &mut egui::Ui, cloud_id: &str) {
    ui.label(
        RichText::new("Connecting GitHub for this cloud")
            .size(15.0)
            .strong()
            .color(theme::FG()),
    );
    ui.label(
        RichText::new(
            "Horizon opened GitHub in your browser. If GitHub asks you to sign in, sign in there; \
             the page then returns to Horizon by itself.",
        )
        .size(12.5)
        .color(theme::FG_DIM()),
    );
    ui.add_space(8.0);
    if ui
        .add(chrome_button("Skip: no GitHub for this cloud").min_size(vec2(0.0, 30.0)))
        .clicked()
    {
        github::skip(cloud_id);
    }
}

/// One line for the steps card: who the cloud acts as on GitHub, or why it has no access.
pub(super) fn summary(runtime: &Runtime) -> Option<(String, bool)> {
    match runtime.github.as_ref()? {
        Prompt::Connected { login, repositories } => Some((
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

    #[test]
    fn the_steps_card_names_the_account_or_why_there_is_no_access() {
        let mut runtime = Runtime::default();
        assert_eq!(summary(&runtime), None);
        runtime.github = Some(Prompt::Connected {
            login: "octo-cat".into(),
            repositories: vec!["acme/web".into(), "acme/api".into()],
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
