//! Background jobs keep repository, settings, and SSH work off the render path.
use super::{Action, CloudGroups, Job, Owner, Snapshot};
use horizon_core::cloud_runtime::{self, Cancellation, companions, settings::Settings};
use std::{
    path::PathBuf,
    sync::mpsc::{Receiver, channel},
};

pub(super) struct Outcome {
    pub snapshot: Option<Snapshot>,
    pub error: Option<String>,
}

pub(super) fn save_deselections(
    root: PathBuf,
    owner: Owner,
    aliases: Vec<String>,
    ctx: egui::Context,
) -> Receiver<Result<(), String>> {
    let (sender, receiver) = channel();
    std::thread::spawn(move || {
        let result = companions::persist_deselections(&root, &owner, &aliases).map_err(|error| error.to_string());
        let _ = sender.send(result);
        ctx.request_repaint();
    });
    receiver
}

pub(super) fn start(root: PathBuf, owner: Owner, groups: CloudGroups, action: Action, ctx: egui::Context) -> Job {
    let (sender, receiver) = channel();
    let cancel = Cancellation::default();
    let worker_cancel = cancel.clone();
    std::thread::spawn(move || {
        let outcome = run(root, owner, &groups, action, &worker_cancel);
        let _ = sender.send(outcome);
        ctx.request_repaint();
    });
    Job { receiver, cancel }
}

fn run(root: PathBuf, owner: Owner, groups: &CloudGroups, action: Action, cancel: &Cancellation) -> Outcome {
    let prepared = companions::inventory::prepare(&owner, groups, cancel);
    let error = prepared.as_ref().err().map(ToString::to_string);
    let context = prepared.ok();
    let result = (|| {
        cancel.check()?;
        let journal = cloud_runtime::state::cloud_directory(&root, &owner.cloud_id)?.join("companions.json");
        // Existing images without companion declarations need no companion protocol support.
        if !matches!(action, Action::Clear { .. })
            && context.as_ref().is_none_or(|context| context.declarations.is_empty())
            && !journal.try_exists()?
        {
            return Ok(None);
        }
        let settings = Settings::load(&root.join("settings.json"))?;
        companions::refresh(
            &companions::Request {
                root,
                owner,
                context,
                action,
                settings,
            },
            cancel,
        )
        .map(Some)
    })();
    match result {
        Ok(snapshot) => Outcome { snapshot, error },
        Err(failure) => Outcome {
            snapshot: None,
            error: Some(format!("{failure}")),
        },
    }
}
