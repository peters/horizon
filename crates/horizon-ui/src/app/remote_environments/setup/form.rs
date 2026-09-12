//! Non-secret local editing only; core preview remains the authoritative validator.

use horizon_core::{
    cloud_run::{
        CloudProvider, GitCommitSha, GitSource, WorkerLifetime, WorkerTarget, runpod::RunPodNetworkVolumeExpectation,
    },
    remote_provider_config::RemoteProviderConfig,
    remote_workspace::RemotePanelCommand,
    remote_workspace_setup::RemoteWorkspaceSetupDraft,
};

struct ProfileChoice {
    provider: CloudProvider,
    name: String,
    label: String,
}

#[derive(Default)]
pub(super) struct Form {
    choices: Vec<ProfileChoice>,
    selected: Option<usize>,
    image: String,
    disk_gib: String,
    repository: String,
    commit: String,
    branch: String,
    working_directory: String,
    program: String,
    argv_json: String,
    panel_directory: String,
    hourly_cents: String,
    volume_id: String,
    data_center_id: String,
    minimum_size_gb: String,
}

impl Form {
    pub(super) fn new(config: &RemoteProviderConfig) -> Self {
        let choices = config
            .local_docker
            .iter()
            .map(|p| (CloudProvider::LocalDocker, &p.name))
            .chain(config.runpod.iter().map(|p| (CloudProvider::RunPod, &p.name)))
            .map(|(provider, name)| ProfileChoice {
                provider,
                name: name.clone(),
                label: format!(
                    "{} / {name}",
                    if provider == CloudProvider::LocalDocker {
                        "Local Docker"
                    } else {
                        "RunPod"
                    }
                ),
            })
            .collect();
        Self {
            choices,
            working_directory: ".".into(),
            argv_json: "[]".into(),
            ..Self::default()
        }
    }

    pub(super) fn show(&mut self, ui: &mut egui::Ui) {
        let stacked = ui.available_width() < 520.0;
        let selected = self.selected.and_then(|i| self.choices.get(i));
        egui::CollapsingHeader::new(selected.map_or("Choose a configured profile", |p| p.label.as_str()))
            .id_salt("new-remote-profile")
            .show(ui, |ui| {
                for (index, profile) in self.choices.iter().enumerate() {
                    ui.selectable_value(&mut self.selected, Some(index), &profile.label);
                }
            });
        if self.choices.is_empty() {
            ui.label("Configure a Local Docker or RunPod profile before setup.");
        }
        ui.label("Nothing has been created yet. Review these values before authorizing setup.");
        ui.strong("Worker and repository");
        egui::Grid::new("new-remote-inputs")
            .num_columns(if stacked { 1 } else { 2 })
            .show(ui, |ui| {
                for (label, value) in [
                    ("Digest-pinned image", &mut self.image),
                    ("Container disk (GiB)", &mut self.disk_gib),
                    ("GitHub owner/repository", &mut self.repository),
                    ("Exact commit SHA", &mut self.commit),
                    ("Dedicated work branch", &mut self.branch),
                    ("Repository directory", &mut self.working_directory),
                    ("Planned Shell program", &mut self.program),
                    ("Literal arguments (JSON array)", &mut self.argv_json),
                    ("Panel directory (optional)", &mut self.panel_directory),
                ] {
                    field(ui, label, value, stacked);
                }
            });
        ui.label("Directories are repository-relative. Arguments are not shell-split; use [] for no arguments.");
        if self
            .selected
            .and_then(|i| self.choices.get(i))
            .is_some_and(|p| p.provider == CloudProvider::RunPod)
        {
            ui.strong("RunPod placement limits");
            egui::Grid::new("new-remote-hps")
                .num_columns(if stacked { 1 } else { 2 })
                .show(ui, |ui| {
                    for (label, value) in [
                        ("Compute ceiling (US cents/hour)", &mut self.hourly_cents),
                        ("Authorized HPS volume ID", &mut self.volume_id),
                        ("Volume data center", &mut self.data_center_id),
                        ("Minimum volume size (GB)", &mut self.minimum_size_gb),
                    ] {
                        field(ui, label, value, stacked);
                    }
                });
            ui.label("The compute ceiling is not a total budget. HPS storage charges are separate; entering an ID does not verify ownership or contents.");
        }
    }

    pub(super) fn draft(&self, retain_until_millis: i64) -> Result<RemoteWorkspaceSetupDraft, &'static str> {
        let profile = self
            .selected
            .and_then(|i| self.choices.get(i))
            .ok_or("Choose a configured provider profile.")?;
        let disk_gib = positive(&self.disk_gib).ok_or("Enter a positive whole-number container disk size.")?;
        let commit = GitCommitSha::parse(&self.commit).map_err(|_| "Enter an exact 40-character commit SHA.")?;
        if [
            &self.image,
            &self.repository,
            &self.branch,
            &self.working_directory,
            &self.program,
        ]
        .iter()
        .any(|value| value.is_empty())
        {
            return Err("Complete the image, repository, work branch, directory and saved program fields.");
        }
        if self.argv_json.len() > 65_536 {
            return Err("The argument JSON exceeds the input limit.");
        }
        let args = serde_json::from_str::<Vec<String>>(&self.argv_json)
            .map_err(|_| "Enter arguments as a JSON array containing only strings.")?;
        let (max_hourly_cost_micros, network_volume) = if profile.provider == CloudProvider::RunPod {
            let micros = positive::<u64>(&self.hourly_cents)
                .and_then(|cents| cents.checked_mul(10_000))
                .ok_or("Enter a positive, supported whole-number US cents/hour ceiling.")?;
            let minimum_size_gb =
                positive(&self.minimum_size_gb).ok_or("Enter a positive whole-number volume size.")?;
            if self.volume_id.is_empty() || self.data_center_id.is_empty() {
                return Err("Enter the authorized HPS volume ID and data center.");
            }
            (
                Some(micros),
                Some(RunPodNetworkVolumeExpectation {
                    volume_id: self.volume_id.clone(),
                    data_center_id: self.data_center_id.clone(),
                    minimum_size_gb,
                }),
            )
        } else {
            (None, None)
        };
        Ok(RemoteWorkspaceSetupDraft {
            target: WorkerTarget {
                provider: profile.provider,
                profile: profile.name.clone(),
                image: self.image.clone(),
                disk_gib,
                lifetime: WorkerLifetime::Persistent,
                max_hourly_cost_micros,
            },
            repository: GitSource {
                repository: self.repository.clone(),
                commit,
                branch: Some(self.branch.clone()),
            },
            working_directory: self.working_directory.clone(),
            command: RemotePanelCommand {
                program: self.program.clone(),
                args,
            },
            panel_directory: (!self.panel_directory.is_empty()).then(|| self.panel_directory.clone()),
            retain_until_millis,
            network_volume,
        })
    }
}

fn positive<T: std::str::FromStr + PartialOrd + From<u8>>(text: &str) -> Option<T> {
    text.parse().ok().filter(|value| *value > T::from(0))
}

fn field(ui: &mut egui::Ui, label: &str, value: &mut String, stacked: bool) {
    ui.label(label);
    if stacked {
        ui.end_row();
    }
    let width = ui.available_width().clamp(80.0, 300.0);
    ui.add(
        egui::TextEdit::singleline(value)
            .desired_width(width)
            .char_limit(65_536),
    );
    ui.end_row();
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(in super::super) fn config() -> RemoteProviderConfig {
        serde_json::from_value(serde_json::json!({
            "local_docker":[{"name":"local","docker_host":"unix:///synthetic/docker.sock"}],
            "runpod":[{"name":"gpu","gpu_type_ids":["synthetic"],"gpu_count":1,"ports":["22/tcp","8080/http"],"volume_gib":37,
                "allowed_cuda_versions":["12.8"],"data_center_id":"synthetic-dc","min_download_mbps":123,
                "min_upload_mbps":234,"min_disk_bandwidth_mbps":345,"container_registry_auth_id":"synthetic-registration"}]
        }))
        .expect("profiles")
    }
    fn form() -> Form {
        Form {
            image: format!("example/worker@sha256:{}", "a".repeat(64)),
            disk_gib: "20".into(),
            repository: "example/project".into(),
            commit: "b".repeat(40),
            branch: "work/synthetic".into(),
            program: "/bin/sh".into(),
            volume_id: "synthetic-volume".into(),
            data_center_id: "synthetic-dc".into(),
            minimum_size_gb: "10".into(),
            ..Form::new(&config())
        }
    }

    #[cfg(target_os = "linux")]
    pub(in super::super) fn populated(cloud: bool) -> Form {
        let mut form = form();
        form.selected = Some(usize::from(cloud));
        form.hourly_cents = "459".into();
        form
    }

    #[test]
    fn explicit_choice_and_literal_arguments_are_preserved() {
        let mut form = form();
        assert!(form.draft(123).is_err());
        form.selected = Some(0);
        form.argv_json = r#"["two words","","quote\"","line\nbreak"]"#.into();
        let draft = form.draft(123).expect("draft");
        assert_eq!(draft.command.args, ["two words", "", "quote\"", "line\nbreak"]);
        assert_eq!(draft.target.provider, CloudProvider::LocalDocker);
        assert!(draft.network_volume.is_none() && draft.target.max_hourly_cost_micros.is_none());
        assert!(draft.panel_directory.is_none());
        assert_eq!(draft.retain_until_millis, 123);
        let empty = Form::new(&RemoteProviderConfig::default());
        assert!(empty.selected.is_none() && empty.image.is_empty() && empty.program.is_empty());
        assert!(empty.draft(123).is_err());
    }

    #[test]
    fn runpod_price_is_explicit_integer_currency_and_checked() {
        let mut form = form();
        form.selected = Some(1);
        for invalid in ["", "0", "1.5", "-1", "18446744073709551615"] {
            form.hourly_cents = invalid.into();
            assert!(form.draft(123).is_err());
        }
        form.hourly_cents = "459".into();
        form.panel_directory = "nested".into();
        let draft = form.draft(123).expect("draft");
        assert_eq!(draft.target.max_hourly_cost_micros, Some(4_590_000));
        assert_eq!(draft.network_volume.expect("volume").minimum_size_gb, 10);
        assert_eq!(draft.panel_directory.as_deref(), Some("nested"));
        form.argv_json = "[private-malformed-marker]".into();
        assert_eq!(
            form.draft(123).err(),
            Some("Enter arguments as a JSON array containing only strings.")
        );
    }
}
