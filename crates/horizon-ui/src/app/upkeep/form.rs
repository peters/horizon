//! What the person chooses in New Upkeep, and the search for repositories behind it.
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

use horizon_core::WorkspaceId;
use horizon_core::upkeep::worker::{AGENT, Checkouts, Config};
use horizon_core::upkeep::{Catalog, DEFAULT_INSTRUCTIONS};

/// What the search for repositories found.
pub(in crate::app) struct Found {
    pub(in crate::app) catalog: Catalog,
    /// Whether Codex has a login to take to the worker: a `ChatGPT` sign-in or an `OpenAI`
    /// key in Cloud settings.
    pub(in crate::app) codex_login: bool,
}

/// How often the agent runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum Every {
    #[default]
    Hour,
    FourHours,
    Day,
}

impl Every {
    pub(in crate::app) const ALL: [Self; 3] = [Self::Hour, Self::FourHours, Self::Day];

    pub(in crate::app) fn label(self) -> &'static str {
        match self {
            Self::Hour => "Every hour",
            Self::FourHours => "Every 4 hours",
            Self::Day => "Once a day",
        }
    }

    fn minutes(self) -> u32 {
        match self {
            Self::Hour => 60,
            Self::FourHours => 240,
            Self::Day => 1440,
        }
    }

    /// The choice of `minutes`; a period the dialog does not offer becomes the default.
    fn from_minutes(minutes: u32) -> Self {
        Self::ALL
            .into_iter()
            .find(|every| every.minutes() == minutes)
            .unwrap_or_default()
    }
}

/// Bots whose pull requests Upkeep moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::app) enum Bot {
    Dependabot,
    Renovate,
}

impl Bot {
    pub(in crate::app) const ALL: [Self; 2] = [Self::Dependabot, Self::Renovate];

    pub(in crate::app) fn label(self) -> &'static str {
        match self {
            Self::Dependabot => "Dependabot",
            Self::Renovate => "Renovate",
        }
    }

    /// The login `gh pr list --author` and GitHub search take.
    fn author(self) -> &'static str {
        match self {
            Self::Dependabot => "app/dependabot",
            Self::Renovate => "app/renovate",
        }
    }

    pub(in crate::app) fn from_author(author: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|bot| bot.author() == author)
    }
}

pub(in crate::app) fn checkouts_label(checkouts: Checkouts) -> &'static str {
    match checkouts {
        Checkouts::OnDemand => "Only when needed",
        Checkouts::Partial => "Partial clones",
        Checkouts::Full => "Full clones",
    }
}

pub(in crate::app) struct Form {
    /// The workspace the cloud goes into; `None` makes a new workspace.
    pub(in crate::app) workspace: Option<WorkspaceId>,
    pub(in crate::app) catalog: Catalog,
    /// The search for repositories while it runs.
    pub(in crate::app) finding: Option<Receiver<Result<Found, String>>>,
    /// Why the last search failed.
    pub(in crate::app) find_error: Option<String>,
    /// The bots the catalog counted pull requests for.
    searched: BTreeSet<Bot>,
    /// Try again was pressed: the next frame searches, whatever the bots.
    retrying: bool,
    /// Keeps chosen names that a new list does not hold: the Repositories editor must not
    /// drop a saved repository just because the app no longer lists it.
    pub(in crate::app) keep_unlisted: bool,
    pub(in crate::app) codex_login: Option<bool>,
    pub(in crate::app) selected: BTreeSet<String>,
    /// The owner tab of the repository list; `None` shows the first owner.
    pub(in crate::app) owner: Option<String>,
    pub(in crate::app) search: String,
    pub(in crate::app) only_open: bool,
    pub(in crate::app) bots: BTreeSet<Bot>,
    pub(in crate::app) every: Every,
    pub(in crate::app) checkouts: Checkouts,
    pub(in crate::app) instructions: String,
    /// Start Upkeep was pressed and the first repository is being prepared.
    pub(in crate::app) starting: bool,
    /// Closed while starting: it shows again when the start fails or New Upkeep opens.
    pub(in crate::app) hidden: bool,
    /// Why the last start failed.
    pub(in crate::app) error: Option<String>,
    /// An agent filled the form in through `cloud_upkeep`.
    pub(in crate::app) proposed: bool,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            workspace: None,
            catalog: Catalog::default(),
            finding: None,
            find_error: None,
            searched: BTreeSet::new(),
            retrying: false,
            keep_unlisted: false,
            codex_login: None,
            selected: BTreeSet::new(),
            owner: None,
            search: String::new(),
            only_open: true,
            bots: BTreeSet::from([Bot::Dependabot]),
            every: Every::default(),
            checkouts: Checkouts::default(),
            instructions: DEFAULT_INSTRUCTIONS.to_string(),
            starting: false,
            hidden: false,
            error: None,
            proposed: false,
        }
    }
}

impl Form {
    /// New Upkeep filled in with what an agent proposed, for `workspace`. Bots other than
    /// those the dialog offers are left out.
    pub(in crate::app) fn proposed(workspace: WorkspaceId, config: &Config) -> Self {
        Self {
            workspace: Some(workspace),
            selected: config.repositories.iter().map(|name| name.to_lowercase()).collect(),
            bots: config
                .authors
                .iter()
                .filter_map(|author| Bot::from_author(author))
                .collect(),
            every: Every::from_minutes(config.every_minutes),
            checkouts: config.checkouts,
            instructions: config.instructions.clone(),
            proposed: true,
            ..Self::default()
        }
    }

    /// What the person chose, as the worker gets it.
    pub(in crate::app) fn config(&self) -> Config {
        Config {
            repositories: self.selected.iter().cloned().collect(),
            authors: self.authors(),
            agent: AGENT.into(),
            every_minutes: self.every.minutes(),
            instructions: self.instructions.clone(),
            worker: String::new(),
            checkouts: self.checkouts,
            order: Vec::new(),
        }
    }

    fn authors(&self) -> Vec<String> {
        self.bots.iter().map(|bot| bot.author().to_owned()).collect()
    }

    /// Why Start Upkeep cannot start yet, or `None` when it can.
    pub(in crate::app) fn blocked(&self) -> Option<String> {
        if self.starting {
            return Some("Starting Upkeep...".into());
        }
        if self.selected.is_empty() {
            return Some("Choose at least one repository.".into());
        }
        if self.bots.is_empty() {
            return Some("Choose at least one bot.".into());
        }
        // The choice is checked against a list GitHub just gave, never an older one.
        if self.finding.is_some() {
            return Some("Finding your repositories...".into());
        }
        if self.find_error.is_some() {
            return Some("Search GitHub again before starting.".into());
        }
        if self.codex_login == Some(false) {
            return Some("Sign in with ChatGPT in Cloud settings first.".into());
        }
        self.config().validate().err().map(|error| format!("{error}."))
    }

    /// Whether the catalog counts pull requests of other bots than the chosen ones.
    pub(in crate::app) fn stale(&self) -> bool {
        self.finding.is_none() && (self.retrying || (!self.bots.is_empty() && self.searched != self.bots))
    }

    /// Forgets the counts when no bot is chosen: they belong to bots that are not chosen,
    /// also when a search that started before arrives later.
    pub(in crate::app) fn forget_counts_without_bots(&mut self) {
        if !self.bots.is_empty() {
            return;
        }
        self.searched.clear();
        if self.catalog.repositories.iter().any(|repository| repository.open > 0) {
            for repository in &mut self.catalog.repositories {
                repository.open = 0;
            }
        }
    }

    /// Makes the next frame search again, after a search failed.
    pub(in crate::app) fn retry(&mut self) {
        self.retrying = true;
    }

    /// Starts a search for repositories with the GitHub sign-in of the cloud root `root`.
    pub(in crate::app) fn find(&mut self, root: PathBuf, ctx: &egui::Context) {
        let authors = self.authors();
        self.searched.clone_from(&self.bots);
        self.retrying = false;
        let (tx, rx) = std::sync::mpsc::channel();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let found = horizon_core::upkeep::discover(&root, &authors).map(|catalog| Found {
                catalog,
                codex_login: codex_login(&root),
            });
            let _ = tx.send(found);
            repaint.request_repaint();
        });
        self.finding = Some(rx);
    }

    /// Takes the search's answer when it came. A repository that was chosen stays chosen.
    pub(in crate::app) fn receive(&mut self) {
        let Some(rx) = &self.finding else { return };
        let answer = match rx.try_recv() {
            Ok(answer) => answer,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("The search for repositories stopped.".into()),
        };
        self.finding = None;
        match answer {
            Ok(found) => {
                // A repository the app can no longer reach cannot be chosen anew.
                if !self.keep_unlisted {
                    self.selected.retain(|name| {
                        found
                            .catalog
                            .repositories
                            .iter()
                            .any(|repository| repository.name == *name)
                    });
                }
                self.catalog = found.catalog;
                self.codex_login = Some(found.codex_login);
                self.find_error = None;
            }
            Err(error) => self.find_error = Some(error),
        }
    }
}

/// Whether Cloud settings hold a login that Codex on a worker can use.
fn codex_login(root: &std::path::Path) -> bool {
    use horizon_core::cloud_runtime::{codex_login, settings, settings::Settings};
    let signed_in = || codex_login::account(&codex_login::home(root)).is_ok_and(|account| account.is_some());
    // Deployment refuses a bound key file it cannot use, also when a sign-in exists, and
    // prefers a usable one to the sign-in.
    match Settings::load(&root.join("settings.json")).map(|settings| settings.openai_api_key_file) {
        Ok(Some(path)) => settings::validate_private_key_file(&path).is_ok(),
        _ => signed_in(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_choice_becomes_a_codex_config_the_worker_accepts() {
        let mut form = Form::default();
        assert_eq!(form.blocked().as_deref(), Some("Choose at least one repository."));
        form.selected.insert("acme/web".into());
        form.selected.insert("acme/api".into());
        form.bots.insert(Bot::Renovate);
        form.every = Every::FourHours;
        assert_eq!(form.blocked(), None);
        let config = form.config();
        assert_eq!(config.repositories, ["acme/api", "acme/web"]);
        assert_eq!(config.authors, ["app/dependabot", "app/renovate"]);
        assert_eq!((config.agent.as_str(), config.every_minutes), ("codex", 240));
        assert!(config.validate().is_ok());
    }

    #[test]
    fn start_says_why_it_cannot_start() {
        let mut form = Form::default();
        form.selected.insert("acme/web".into());
        form.bots.clear();
        assert_eq!(form.blocked().as_deref(), Some("Choose at least one bot."));
        form.bots.insert(Bot::Dependabot);
        form.instructions = "x".repeat(9 * 1024);
        assert!(form.blocked().is_some_and(|reason| reason.contains("too long")));
        form.instructions = DEFAULT_INSTRUCTIONS.into();
        form.codex_login = Some(false);
        assert_eq!(
            form.blocked().as_deref(),
            Some("Sign in with ChatGPT in Cloud settings first.")
        );
        form.codex_login = Some(true);
        assert_eq!(form.blocked(), None);
        form.starting = true;
        assert_eq!(form.blocked().as_deref(), Some("Starting Upkeep..."));
    }

    #[test]
    fn a_repository_the_app_no_longer_reaches_is_no_longer_chosen() {
        use horizon_core::upkeep::{Catalog, CatalogRepository};
        let mut form = Form::default();
        form.selected.insert("acme/web".into());
        form.selected.insert("acme/gone".into());
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Ok(Found {
            catalog: Catalog {
                repositories: vec![CatalogRepository {
                    name: "acme/web".into(),
                    open: 0,
                }],
                search_error: None,
            },
            codex_login: true,
        }))
        .unwrap();
        form.finding = Some(rx);
        form.receive();
        assert_eq!(form.selected.iter().collect::<Vec<_>>(), ["acme/web"]);
        assert_eq!(form.codex_login, Some(true));
    }

    #[test]
    fn try_again_searches_again_for_the_same_bots() {
        let mut form = Form::default();
        form.searched.clone_from(&form.bots);
        assert!(!form.stale());
        form.retry();
        assert!(form.stale());
        form.bots.clear();
        assert!(form.stale(), "Try again searches also without a bot");
    }

    #[test]
    fn a_change_of_bots_makes_the_counts_stale() {
        let mut form = Form::default();
        assert!(form.stale(), "nothing was searched yet");
        form.searched.clone_from(&form.bots);
        assert!(!form.stale());
        form.bots.insert(Bot::Renovate);
        assert!(form.stale());
    }

    #[test]
    fn without_bots_no_counts_remain() {
        use horizon_core::upkeep::CatalogRepository;
        let mut form = Form::default();
        form.searched.clone_from(&form.bots);
        form.catalog.repositories.push(CatalogRepository {
            name: "acme/web".into(),
            open: 5,
        });
        form.selected.insert("acme/web".into());
        form.bots.clear();
        form.forget_counts_without_bots();
        assert_eq!(form.catalog.open_in(&form.selected), 0);
        form.bots.insert(Bot::Dependabot);
        assert!(form.stale(), "a bot chosen again searches again");
    }
}
