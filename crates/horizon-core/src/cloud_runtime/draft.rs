//! The New cloud drafts that a dismissal parks in their workspaces: the choices each one
//! kept, saved in `drafts.json` beside the cloud settings, so that a quit or a session switch
//! brings the card back. Only choices are kept: a clone, a token, a check, a publishing
//! selection, a sibling and a pending creation never outlive the dialog.
//!
//! The file is one folder for every session, so it can hold the records of several sessions at
//! once while each record names the session that owns it. The New cloud form holds one draft at
//! a time: a restore shows the card of the first kept draft that belongs to the session it
//! restored and whose workspace still exists, and the records of other sessions stay for the
//! restore that owns them.
use super::{Error, Result};
use crate::cloud_panel::Placement;
use crate::cloud_runtime::repository::launch;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io::Write as _, path::Path};

const FILE: &str = "drafts.json";
/// More drafts than a board has workspaces with a card; a larger file is not this one.
const MAX_ENTRIES: usize = 20;

/// The choices one parked draft kept, by workspace local id.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kept {
    /// The session whose board held the workspace, so that a restore drops the record of a
    /// removed workspace only for the session it belongs to; `None` for an ephemeral session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default)]
    pub title: String,
    /// The repository as the field held it: a link or a folder.
    #[serde(default)]
    pub repository: String,
    /// The committed base revision the person typed; empty for the checked-out commit.
    #[serde(default)]
    pub revision: String,
    /// The profile name the person chose; empty keeps the settings' default.
    #[serde(default)]
    pub profile: String,
    /// The worker size the person chose; none keeps the profile's size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<Worker>,
    #[serde(default, skip_serializing_if = "Placement::is_default")]
    pub placement: Placement,
    /// The provider's `id`, such as `runpod`, as `provider::by_id` resolves it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// The tailnet the person chose, by id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tailnet: Option<String>,
    /// The setup agent's `id`, such as `claude`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_agent: Option<String>,
    /// The setup agent preset the person chose, by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_preset: Option<String>,
    #[serde(default)]
    pub configuration: Configuration,
    /// Whether New workspace made the workspace that holds the draft, whose empty workspace
    /// Cancel then takes away.
    #[serde(default)]
    pub new_workspace: bool,
}

/// A worker size a draft kept: its vCPU count and its memory in GB.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worker {
    pub cpu: u16,
    pub memory_gb: u16,
}

/// The launch configuration a draft keeps. New cloud sets only these two: a committed
/// configuration follows a build and is not a choice of the person.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Configuration {
    #[default]
    WorkingCopy,
    QuickStart,
}

impl Configuration {
    /// What a saved draft keeps of the launch configuration.
    #[must_use]
    pub fn of(configuration: launch::Configuration) -> Self {
        match configuration {
            launch::Configuration::QuickStart => Self::QuickStart,
            launch::Configuration::Committed | launch::Configuration::WorkingCopy => Self::WorkingCopy,
        }
    }

    /// The launch configuration the dialog reads its repository with.
    #[must_use]
    pub fn launch(self) -> launch::Configuration {
        match self {
            Self::WorkingCopy => launch::Configuration::WorkingCopy,
            Self::QuickStart => launch::Configuration::QuickStart,
        }
    }
}

/// The kept choices of the parked drafts under a cloud settings folder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Drafts(BTreeMap<String, Kept>);

impl Drafts {
    /// The drafts saved under `root`. A missing or unreadable file keeps nothing, and so does
    /// a file that holds more drafts than a board has workspaces with a card.
    #[must_use]
    pub fn load(root: &Path) -> Self {
        std::fs::read(root.join(FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<BTreeMap<String, Kept>>(&bytes).ok())
            .filter(|drafts| drafts.len() <= MAX_ENTRIES)
            .map(Self)
            .unwrap_or_default()
    }

    /// The choices the draft of `workspace` kept.
    #[must_use]
    pub fn get(&self, workspace: &str) -> Option<&Kept> {
        self.0.get(workspace)
    }

    /// The workspace of each saved draft.
    pub fn workspaces(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    /// Keeps `kept` for `workspace`, or forgets that draft with `None`, and saves all drafts
    /// under `root`. A save that changes nothing writes nothing, so ending a draft that was
    /// never parked touches no file. The file is written and renamed into place here, so a
    /// save that cannot be written fails now; on Unix, making it durable runs on a worker
    /// thread, and a crash before then can leave the file unreadable on a file system that
    /// does not write data before a rename. An unreadable file keeps nothing ([`Self::load`]),
    /// so a draft then only stays until Horizon quits: a card is a convenience, not worth a
    /// stalled frame.
    ///
    /// A caller that must not wait for the write applies [`Self::keep`] and has a worker thread
    /// call [`Self::save`].
    ///
    /// # Errors
    /// The workspace has no id, more drafts are kept than fit beside the cloud settings, or
    /// the file cannot be written.
    pub fn set(&mut self, root: &Path, workspace: &str, kept: Option<Kept>) -> Result<()> {
        if self.keep(workspace, kept)? {
            self.save(root)?;
        }
        Ok(())
    }

    /// Keeps `kept` for `workspace`, or forgets that draft with `None`, in memory only, and
    /// reports whether that changed what this holds. A caller that keeps a draft on a frame
    /// changes it here and lets a worker thread write it with [`Self::save`], so that no frame
    /// opens, writes or renames the file.
    ///
    /// # Errors
    /// The workspace has no id, or more drafts are kept than fit beside the cloud settings.
    pub fn keep(&mut self, workspace: &str, kept: Option<Kept>) -> Result<bool> {
        let workspace = workspace.trim();
        if workspace.is_empty() {
            return Err(Error::Invalid("No workspace to keep a draft for"));
        }
        let mut next = self.0.clone();
        match kept {
            Some(kept) => next.insert(workspace.to_owned(), kept),
            None => next.remove(workspace),
        };
        if next.len() > MAX_ENTRIES {
            return Err(Error::Invalid("Too many drafts keep their choices"));
        }
        if next == self.0 {
            return Ok(false);
        }
        self.0 = next;
        Ok(true)
    }

    /// Writes every draft that this holds under `root`, for a caller that is not a frame: the
    /// file is replaced in one rename, and the earlier file stays until then.
    ///
    /// # Errors
    /// The file cannot be written.
    pub fn save(&self, root: &Path) -> Result<()> {
        save(root, &self.0)
    }
}

/// Writes `drafts` under `root` and renames them into place.
fn save(root: &Path, drafts: &BTreeMap<String, Kept>) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(drafts).map_err(|_| Error::Json)?;
    std::fs::create_dir_all(root)?;
    let mut file = tempfile::NamedTempFile::new_in(root)?;
    file.write_all(&bytes)?;
    file.persist(root.join(FILE)).map_err(|error| error.error)?;
    #[cfg(unix)]
    {
        let root = root.to_path_buf();
        std::thread::spawn(move || sync_in_background(&root));
    }
    Ok(())
}

/// Makes the saved drafts under `root`, and their name in the folder, durable.
#[cfg(unix)]
fn sync_in_background(root: &Path) {
    let synced = std::fs::File::open(root.join(FILE))
        .and_then(|file| file.sync_all())
        .and_then(|()| std::fs::File::open(root)?.sync_all());
    if let Err(error) = synced {
        tracing::warn!(root = %root.display(), %error, "the cloud drafts are saved but not yet durable");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kept(title: &str, repository: &str) -> Kept {
        Kept {
            session: Some("session-a".to_owned()),
            title: title.to_owned(),
            repository: repository.to_owned(),
            revision: "main".to_owned(),
            profile: "atlas-large".to_owned(),
            worker: Some(Worker { cpu: 4, memory_gb: 8 }),
            placement: Placement {
                data_centers: vec!["eu-central".to_owned()],
                ..Placement::default()
            },
            provider: Some("hetzner".to_owned()),
            tailnet: Some("tailnet-1".to_owned()),
            setup_agent: Some("claude".to_owned()),
            setup_preset: Some("Pi DeepSeek".to_owned()),
            configuration: Configuration::QuickStart,
            new_workspace: true,
        }
    }

    #[test]
    fn every_kept_choice_survives_a_reload_and_a_draft_can_be_forgotten() {
        let root = tempfile::tempdir().unwrap();
        let mut drafts = Drafts::load(root.path());
        assert_eq!(drafts.get("atlas"), None);
        assert_eq!(drafts.workspaces().count(), 0);

        drafts
            .set(root.path(), "atlas", Some(kept("Atlas cloud", "/code/atlas")))
            .unwrap();
        drafts
            .set(
                root.path(),
                "beta",
                Some(kept("Beta cloud", "https://github.com/example/beta")),
            )
            .unwrap();
        let reloaded = Drafts::load(root.path());
        assert_eq!(
            reloaded.get("atlas"),
            Some(&kept("Atlas cloud", "/code/atlas")),
            "a reload keeps every choice"
        );
        assert_eq!(
            reloaded.get("beta").map(|kept| kept.repository.as_str()),
            Some("https://github.com/example/beta")
        );
        assert_eq!(reloaded.workspaces().collect::<Vec<_>>(), ["atlas", "beta"]);

        let mut drafts = reloaded;
        drafts.set(root.path(), "atlas", None).unwrap();
        assert_eq!(Drafts::load(root.path()).get("atlas"), None);
        assert_eq!(
            Drafts::load(root.path()).get("beta").map(|kept| kept.title.as_str()),
            Some("Beta cloud"),
            "another workspace keeps its draft"
        );
    }

    #[test]
    fn an_unchanged_draft_writes_no_file() {
        let root = tempfile::tempdir().unwrap();
        let mut drafts = Drafts::load(root.path());
        drafts
            .set(root.path(), "atlas", Some(kept("Atlas cloud", "/code/atlas")))
            .unwrap();
        // A damaged file stays as it is, so a save that would change nothing never ran.
        std::fs::write(root.path().join(FILE), b"not the drafts").unwrap();
        drafts
            .set(root.path(), "atlas", Some(kept("Atlas cloud", "/code/atlas")))
            .unwrap();
        drafts.set(root.path(), "nowhere", None).unwrap();
        assert_eq!(std::fs::read(root.path().join(FILE)).unwrap(), b"not the drafts");
    }

    #[test]
    fn a_kept_draft_opens_no_file_until_a_save_writes_it() {
        let root = tempfile::tempdir().unwrap();
        let mut drafts = Drafts::load(root.path());
        assert!(
            drafts.keep("atlas", Some(kept("Atlas cloud", "/code/atlas"))).unwrap(),
            "keeping a draft changes what this holds"
        );
        assert_eq!(drafts.get("atlas"), Some(&kept("Atlas cloud", "/code/atlas")));
        assert_eq!(
            std::fs::read(root.path().join(FILE)).ok(),
            None,
            "keeping a draft on a frame writes no file"
        );
        assert!(
            !drafts.keep("atlas", Some(kept("Atlas cloud", "/code/atlas"))).unwrap(),
            "the same draft again keeps nothing new"
        );
        drafts.save(root.path()).unwrap();
        assert_eq!(
            Drafts::load(root.path()).get("atlas"),
            Some(&kept("Atlas cloud", "/code/atlas")),
            "the save writes the draft that was kept in memory"
        );
    }

    #[test]
    fn a_damaged_file_and_a_file_of_too_many_drafts_keep_nothing() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(FILE), b"{not json").unwrap();
        assert_eq!(Drafts::load(root.path()), Drafts::default());

        let many: BTreeMap<String, Kept> = (0..=MAX_ENTRIES)
            .map(|index| (format!("workspace-{index}"), Kept::default()))
            .collect();
        std::fs::write(root.path().join(FILE), serde_json::to_vec(&many).unwrap()).unwrap();
        assert_eq!(Drafts::load(root.path()), Drafts::default());

        let mut drafts = Drafts::load(root.path());
        assert!(drafts.set(root.path(), "  ", Some(Kept::default())).is_err());
        for index in 0..MAX_ENTRIES {
            drafts
                .set(root.path(), &format!("workspace-{index}"), Some(Kept::default()))
                .unwrap();
        }
        assert!(
            drafts.set(root.path(), "one-too-many", Some(Kept::default())).is_err(),
            "a board keeps a bounded number of drafts"
        );
    }

    #[test]
    fn the_file_holds_only_the_choices_of_the_person() {
        let root = tempfile::tempdir().unwrap();
        let mut drafts = Drafts::load(root.path());
        drafts
            .set(root.path(), "atlas", Some(kept("Atlas cloud", "/code/atlas")))
            .unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(root.path().join(FILE)).unwrap()).unwrap();
        let mut fields: Vec<&str> = saved["atlas"].as_object().unwrap().keys().map(String::as_str).collect();
        fields.sort_unstable();
        assert_eq!(
            fields,
            [
                "configuration",
                "new_workspace",
                "placement",
                "profile",
                "provider",
                "repository",
                "revision",
                "session",
                "setup_agent",
                "setup_preset",
                "tailnet",
                "title",
                "worker",
            ],
            "a token, a check, a clone, a sibling or a pending creation is never saved"
        );
        assert_eq!(saved["atlas"]["configuration"], "quick_start");
        assert_eq!(saved["atlas"]["worker"]["memory_gb"], 8);
    }

    #[test]
    fn a_launch_configuration_keeps_what_the_person_can_choose() {
        assert_eq!(
            Configuration::of(launch::Configuration::QuickStart).launch(),
            launch::Configuration::QuickStart
        );
        assert_eq!(
            Configuration::of(launch::Configuration::Committed).launch(),
            launch::Configuration::WorkingCopy,
            "a committed configuration follows a build, not a choice"
        );
    }
}
