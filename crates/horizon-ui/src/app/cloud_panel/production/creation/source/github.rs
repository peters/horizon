//! The connected GitHub account in the source step: pick one of the repositories the app
//! reaches, and clone a private one without a personal access token. This computer signs
//! in once for that, the way the app's setting says, and Horizon renews that sign-in.
use crate::{
    app::util::{chrome_button, primary_button},
    theme,
};
use egui::{Context, RichText, Ui, vec2};
use horizon_core::cloud_runtime::{
    Cancellation,
    github::{self, Prompt, Secret, host},
    settings::Settings as Machine,
};
use std::{
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, TryRecvError, channel},
};
use zeroize::Zeroize as _;

/// At most this many repositories show at once; typing narrows them.
const SHOWN: usize = 8;

/// What a finished exchange with GitHub is for.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Purpose {
    /// The list of repositories to pick from.
    List,
    /// A token to clone the repository in the field.
    Clone,
}

enum Answer {
    Prompt(Prompt),
    Done {
        token: Secret,
        repositories: Option<Vec<String>>,
    },
    Failed(String),
}

/// Ends its own request's sign-in when dropped, as on Cancel or when the dialog closes,
/// and no other.
struct Abort(Cancellation);

impl Drop for Abort {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

struct Job {
    receiver: Receiver<Answer>,
    purpose: Purpose,
    _abort: Abort,
}

/// The connected app and what this computer's account has shown of it.
pub(super) struct Account {
    root: PathBuf,
    settings: github::Settings,
    job: Option<Job>,
    /// The code to approve, or the page to open, while this computer signs in.
    prompt: Option<Prompt>,
    pub(super) repositories: Option<Vec<String>>,
    pub(super) failed: Option<String>,
}

impl Account {
    /// The connected app of the machine settings in `root`, or `None` without one.
    pub(super) fn load(root: &Path) -> Option<Self> {
        let settings = Machine::load(&root.join("settings.json")).ok()?.github?;
        Some(Self {
            root: root.to_owned(),
            settings,
            job: None,
            prompt: None,
            repositories: None,
            failed: None,
        })
    }

    /// The page where the person adds repositories to the app.
    pub(super) fn installation_url(&self) -> String {
        self.settings.installation_url()
    }

    pub(super) fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// Asks GitHub for a token, signing this computer in first when it has none, and for
    /// the repositories too when they are to be listed.
    pub(super) fn request(&mut self, purpose: Purpose, ctx: &Context) {
        if self.job.is_some() {
            return;
        }
        let (sender, receiver) = channel();
        let cancel = Cancellation::default();
        self.job = Some(Job {
            receiver,
            purpose,
            _abort: Abort(cancel.clone()),
        });
        self.failed = None;
        let (root, settings, ctx) = (self.root.clone(), self.settings.clone(), ctx.clone());
        std::thread::spawn(move || {
            let shown = sender.clone();
            let repaint = ctx.clone();
            let show = move |prompt: Prompt| {
                let _ = shown.send(Answer::Prompt(prompt));
                repaint.request_repaint();
            };
            let answer = exchange(&root, &settings, &cancel, &show, purpose);
            let _ = sender.send(answer);
            ctx.request_repaint();
        });
    }

    /// Takes what GitHub answered. Returns the token of a finished request to clone.
    pub(super) fn poll(&mut self) -> Option<Secret> {
        let job = self.job.as_ref()?;
        let purpose = job.purpose;
        loop {
            match job.receiver.try_recv() {
                Ok(Answer::Prompt(prompt)) => {
                    if let Prompt::Web { url } = &prompt
                        && let Err(error) = horizon_core::open_url(url)
                    {
                        tracing::warn!(%error, "could not open the GitHub sign-in page");
                    }
                    self.prompt = Some(prompt);
                }
                Ok(Answer::Done { token, repositories }) => {
                    self.finish();
                    if repositories.is_some() {
                        self.repositories = repositories;
                    }
                    return (purpose == Purpose::Clone).then_some(token);
                }
                Ok(Answer::Failed(reason)) => {
                    self.finish();
                    self.failed = Some(reason);
                    return None;
                }
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => {
                    self.finish();
                    return None;
                }
            }
        }
    }

    fn finish(&mut self) {
        self.job = None;
        self.prompt = None;
    }

    /// The sign-in box while this computer signs in, with a way out.
    pub(super) fn sign_in_box(&mut self, ui: &mut Ui) {
        if prompt(ui, self.prompt.as_ref(), self.job.is_some()) {
            // The request goes, and its sign-in ends with it.
            self.finish();
        }
    }

    /// The repositories to pick from, narrowed by `filter`. Returns the one clicked.
    pub(super) fn picker(&mut self, ui: &mut Ui, filter: &str) -> Option<String> {
        let repositories = self.repositories.as_ref()?;
        let matching = matching(repositories, filter);
        let mut chosen = None;
        if repositories.is_empty() {
            ui.label(
                RichText::new("The GitHub App is on no repository of yours yet.")
                    .size(13.0)
                    .color(theme::FG_DIM()),
            );
        }
        for name in matching {
            if ui
                .add(
                    egui::Button::new(RichText::new(name).size(13.5).color(theme::FG()))
                        .frame(false)
                        .min_size(vec2(ui.available_width(), 24.0)),
                )
                .clicked()
            {
                chosen = Some(name.clone());
            }
        }
        chosen
    }
}

/// Shows `prompt`, or a spinner while `busy` without one. Returns whether the person
/// cancelled the sign-in.
fn prompt(ui: &mut Ui, prompt: Option<&Prompt>, busy: bool) -> bool {
    let Some(prompt) = prompt else {
        if busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Asking GitHub…").size(13.5).color(theme::FG_SOFT()));
            });
        }
        return false;
    };
    let mut cancelled = false;
    egui::Frame::new()
        .fill(theme::alpha(theme::ACCENT(), 18))
        .stroke(egui::Stroke::new(1.0, theme::alpha(theme::ACCENT(), 120)))
        .corner_radius(10)
        .inner_margin(14)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new("Sign in to GitHub on this computer")
                    .size(14.5)
                    .strong()
                    .color(theme::FG()),
            );
            let (words, code, page) = match prompt {
                Prompt::Device {
                    user_code,
                    verification_uri,
                    ..
                } => (
                    "Once, for listing and cloning your repositories. Open GitHub, paste the code and click \
                     Authorize.",
                    Some(user_code),
                    verification_uri,
                ),
                Prompt::Web { url } => (
                    "Horizon opened GitHub in your browser; if it did not, click Open GitHub. The page then \
                     returns here by itself.",
                    None,
                    url,
                ),
                _ => return,
            };
            ui.label(RichText::new(words).size(12.5).color(theme::FG_DIM()));
            if let Some(code) = code {
                ui.label(RichText::new(code).monospace().size(22.0).strong().color(theme::FG()));
            }
            ui.horizontal(|ui| {
                // Also the way back when the browser did not open.
                if ui
                    .add(primary_button("Open GitHub").min_size(vec2(120.0, 30.0)))
                    .clicked()
                {
                    if let Some(code) = code {
                        ui.ctx().copy_text(code.clone());
                    }
                    if let Err(error) = horizon_core::open_url(page) {
                        tracing::warn!(%error, "could not open the GitHub sign-in page");
                    }
                }
                cancelled = ui.add(chrome_button("Cancel")).clicked();
            });
        });
    cancelled
}

/// The repositories whose name holds the typed `filter`, at most [`SHOWN`]. A whole link
/// in the field is looked up as it is, so the list steps aside for it.
fn matching<'a>(repositories: &'a [String], filter: &str) -> Vec<&'a String> {
    let filter = filter.trim().to_ascii_lowercase();
    let filter = filter.as_str();
    repositories
        .iter()
        .filter(|name| filter.is_empty() || name.contains(filter))
        .take(SHOWN)
        .collect()
}

/// Whether the field holds a link rather than a name to narrow the list by.
fn names_a_link(input: &str) -> bool {
    let input = input.trim();
    input.contains(':') || input.to_ascii_lowercase().contains("github.com")
}

fn exchange(
    root: &Path,
    settings: &github::Settings,
    cancel: &Cancellation,
    show: &dyn Fn(Prompt),
    purpose: Purpose,
) -> Answer {
    let token = match host::current(root, settings) {
        Ok(Some((_, token))) => token,
        Ok(None) => match host::sign_in(root, settings, cancel, show) {
            Ok(Ok((_, token))) => token,
            Ok(Err(reason)) => return Answer::Failed(reason),
            Err(error) => return Answer::Failed(error.to_string()),
        },
        Err(error) => return Answer::Failed(error.to_string()),
    };
    let repositories = match purpose {
        Purpose::Clone => None,
        Purpose::List => match host::repositories(&token) {
            Ok(found) => Some(found),
            Err(reason) => return Answer::Failed(reason),
        },
    };
    Answer::Done { token, repositories }
}

impl super::State {
    /// Reads the connected GitHub App of the machine settings in `root`, once per dialog.
    pub fn connect_github(&mut self, root: Option<&Path>) {
        if !std::mem::replace(&mut self.account_read, true) {
            self.account = root.and_then(Account::load);
        }
    }

    /// Starts the clone with the connected account's token once it arrived; it is used as a
    /// pasted token would be, and never saved. A token asked for a repository the field no
    /// longer names starts nothing.
    pub(super) fn take_account_token(&mut self, ctx: &Context) {
        if let Some(token) = self.account.as_mut().and_then(Account::poll) {
            let asked = self.account_for.take();
            if asked.is_none() || self.remote() != asked.as_ref() {
                return;
            }
            self.token.zeroize();
            self.token.push_str(token.expose());
            self.remember = false;
            self.connected = true;
            self.start(ctx);
        }
    }

    /// Whether a private GitHub link is cloned with the connected account rather than a
    /// pasted token.
    pub(super) fn offers_account(&self, remote: &horizon_core::cloud_runtime::repository::source::Remote) -> bool {
        remote.host == "github.com" && self.account.is_some() && !self.token_instead
    }

    /// In place of the token card for a private GitHub repository: clone it with the
    /// connected account, or paste a token after all.
    pub(super) fn connected_card(&mut self, ui: &mut Ui) {
        let tried = self.connected;
        let Some(account) = self.account.as_mut() else {
            return;
        };
        let (mut clone, mut instead) = (false, false);
        egui::Frame::new()
            .fill(theme::PANEL_BG_ALT())
            .stroke(egui::Stroke::new(1.0, theme::BORDER_SUBTLE()))
            .corner_radius(10)
            .inner_margin(14)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 6.0;
                ui.label(
                    RichText::new("Private repository on github.com")
                        .size(14.5)
                        .strong()
                        .color(theme::FG()),
                );
                let (body, color) = if tried {
                    (
                        "Your GitHub App cannot reach this repository yet. Add it to the app on GitHub, then try \
                         again.",
                        theme::PALETTE_YELLOW(),
                    )
                } else {
                    (
                        "Clone it with your connected GitHub account. No token needed.",
                        theme::FG_SOFT(),
                    )
                };
                ui.label(RichText::new(body).size(13.0).color(color));
                ui.add_enabled_ui(!account.busy(), |ui| {
                    ui.horizontal(|ui| {
                        let label = if tried { "Try again" } else { "Clone with GitHub" };
                        clone = ui.add(primary_button(label).min_size(vec2(150.0, 32.0))).clicked();
                        if tried
                            && ui.add(chrome_button("Add it on GitHub")).clicked()
                            && let Err(error) = horizon_core::open_url(&account.installation_url())
                        {
                            tracing::warn!(%error, "could not open the GitHub App installation page");
                        }
                        instead = ui.add(chrome_button("Use a token instead")).clicked();
                    });
                });
                account.sign_in_box(ui);
                if let Some(failed) = &account.failed {
                    ui.label(RichText::new(failed).size(13.0).color(theme::PALETTE_RED()));
                }
            });
        if clone {
            self.account_for = self.remote().cloned();
            if let Some(account) = self.account.as_mut() {
                account.request(Purpose::Clone, ui.ctx());
            }
        }
        if instead {
            self.token_instead = true;
        }
    }

    /// The repositories of the connected app to pick from, while no folder or other host is
    /// in the field. Picking one puts its link in the field, which the dialog then looks up.
    pub(super) fn github_section(&mut self, ui: &mut Ui) {
        if self.job.is_some() || self.folder.is_some() {
            return;
        }
        let elsewhere = self.remote.as_ref().is_some_and(|remote| remote.host != "github.com");
        // A typed `owner/name` narrows the list; a link in the field, pasted or picked, is
        // the choice.
        let picked = self.remote.is_some() && names_a_link(&self.input);
        let Some(account) = self.account.as_mut() else {
            return;
        };
        // Once a link is in the field the list steps aside; a sign-in for a clone shows in
        // the private repository's card.
        if elsewhere || (picked && account.repositories.is_some()) {
            return;
        }
        let mut chosen = None;
        ui.add_space(4.0);
        if account.repositories.is_none() {
            if account.busy() {
                if !picked {
                    account.sign_in_box(ui);
                }
            } else if !picked && ui.add(chrome_button("Pick from your GitHub repositories")).clicked() {
                account.request(Purpose::List, ui.ctx());
            }
        } else {
            super::super::selector::widgets::caption(ui, "YOUR GITHUB REPOSITORIES");
            let filter = self.input.trim().to_owned();
            chosen = account.picker(ui, &filter);
            if ui
                .add(chrome_button("Add repositories on GitHub"))
                .on_hover_text("Choose which repositories your GitHub App can reach.")
                .clicked()
                && let Err(error) = horizon_core::open_url(&account.installation_url())
            {
                tracing::warn!(%error, "could not open the GitHub App installation page");
            }
        }
        if let Some(failed) = &account.failed
            && !picked
        {
            ui.label(RichText::new(failed).size(13.0).color(theme::PALETTE_RED()));
        }
        if let Some(name) = chosen {
            self.input = format!("https://github.com/{name}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_egui::DiscardTextures as _;

    fn account() -> Account {
        Account {
            root: PathBuf::from("/synthetic"),
            settings: github::Settings {
                app_id: 42,
                slug: "horizon-example".into(),
                client_id: "Iv23synthetic".into(),
                client_secret_file: PathBuf::from("/synthetic/secret"),
                mode: github::Mode::Ask,
            },
            job: None,
            prompt: None,
            repositories: None,
            failed: None,
        }
    }

    #[test]
    fn typing_narrows_the_repositories_by_name() {
        let names: Vec<String> = ["acme/api", "acme/web", "acme/web-docs", "octo/web"]
            .iter()
            .map(|name| (*name).to_owned())
            .collect();
        assert_eq!(matching(&names, "").len(), 4);
        assert_eq!(matching(&names, "WEB"), [&names[1], &names[2], &names[3]]);
        assert_eq!(matching(&names, "acme/web"), [&names[1], &names[2]]);
        let many: Vec<String> = (0..20).map(|n| format!("acme/repo-{n}")).collect();
        assert_eq!(matching(&many, "repo").len(), SHOWN);
    }

    #[test]
    fn a_private_github_link_offers_the_connected_account_unless_a_token_was_chosen() {
        let github = horizon_core::cloud_runtime::repository::source::parse("github.com/acme/private").unwrap();
        let gitlab = horizon_core::cloud_runtime::repository::source::parse("gitlab.com/acme/private").unwrap();
        let mut state = super::super::State::default();
        assert!(!state.offers_account(&github), "without a connected app");
        state.account = Some(account());
        assert!(state.offers_account(&github));
        assert!(!state.offers_account(&gitlab));
        state.token_instead = true;
        assert!(!state.offers_account(&github));
    }

    #[test]
    fn the_list_shows_the_connected_repositories() {
        let mut account = account();
        account.repositories = Some(vec!["acme/web".into(), "acme/api".into()]);
        let texts: Vec<String> = egui::Context::default()
            .run_ui(egui::RawInput::default(), |ui| {
                assert!(account.picker(ui, "web").is_none(), "nothing is chosen without a click");
            })
            .discard_textures()
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|text| text == "acme/web"));
        assert!(!texts.iter().any(|text| text == "acme/api"));
    }

    #[test]
    fn ending_a_request_ends_only_its_own_sign_in() {
        let (mine, other) = (Cancellation::default(), Cancellation::default());
        drop(Abort(mine.clone()));
        assert!(mine.check().is_err());
        assert!(other.check().is_ok(), "another request's sign-in goes on");
    }

    #[test]
    fn an_automatic_sign_in_offers_the_page_again_and_a_way_out() {
        let web = Prompt::Web {
            url: "https://github.com/login/oauth/authorize?client_id=Iv23synthetic".into(),
        };
        let texts: Vec<String> = egui::Context::default()
            .run_ui(egui::RawInput::default(), |ui| {
                assert!(!prompt(ui, Some(&web), true), "nothing is cancelled without a click");
            })
            .discard_textures()
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|text| text == "Open GitHub"));
        assert!(texts.iter().any(|text| text == "Cancel"));
    }

    #[test]
    fn a_token_asked_for_another_repository_starts_nothing() {
        let mut state = super::super::State::default();
        state.input = "github.com/acme/other".into();
        state.account_for = horizon_core::cloud_runtime::repository::source::parse("github.com/acme/private");
        let (sender, receiver) = channel();
        sender
            .send(Answer::Done {
                token: Secret::new("ghu_synthetic".into()),
                repositories: None,
            })
            .unwrap();
        let mut account = account();
        account.job = Some(Job {
            receiver,
            purpose: Purpose::Clone,
            _abort: Abort(Cancellation::default()),
        });
        state.account = Some(account);
        state.take_account_token(&egui::Context::default());
        assert!(state.token.is_empty(), "the token is not used");
        assert!(state.job.is_none() && !state.connected, "no clone started");
        assert!(state.account_for.is_none());
    }

    #[test]
    fn a_typed_name_narrows_the_list_and_a_link_sets_it_aside() {
        assert!(!names_a_link("acme/web"));
        assert!(!names_a_link(" web "));
        for link in [
            "https://github.com/acme/web",
            "github.com/acme/web",
            "git@github.com:acme/web.git",
        ] {
            assert!(names_a_link(link), "{link}");
        }
        let mut state = super::super::State::default();
        let mut account = account();
        account.repositories = Some(vec!["acme/api".into(), "acme/web".into()]);
        state.account = Some(account);
        let texts = |state: &mut super::super::State| -> Vec<String> {
            egui::Context::default()
                .run_ui(egui::RawInput::default(), |ui| state.github_section(ui))
                .discard_textures()
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                    _ => None,
                })
                .collect()
        };
        state.input = "acme/web".into();
        let _ = state.remote();
        let shown = texts(&mut state);
        assert!(shown.iter().any(|text| text == "acme/web"), "{shown:?}");
        assert!(!shown.iter().any(|text| text == "acme/api"));
        state.input = "https://github.com/acme/web".into();
        let _ = state.remote();
        assert!(!texts(&mut state).iter().any(|text| text == "acme/web"));
    }
}
