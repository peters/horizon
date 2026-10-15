//! Applies only a current worker decision while its worker retains the record lock.
use super::super::removal::{Outcome, Owner, Purpose, Refusal, Snapshot};
use super::{HorizonApp, Record};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app::cloud_panel::production) enum Origin {
    Direct,
    Close,
    AfterDelete,
    Anyway,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app::cloud_panel::production) enum RemovalStart {
    Pending,
    Refused,
}

impl HorizonApp {
    pub(super) fn pin_removal_owner(&mut self, id: u32) -> bool {
        let Some(group) = self.cloud_prototype.groups.0.iter().find(|group| group.issue == id) else {
            return false;
        };
        let Some(launch) = group.remote.as_ref() else {
            return false;
        };
        let close = &mut self.cloud_prototype.production.close;
        let Some(generation) = close.generation.checked_add(1) else {
            return false;
        };
        close.generation = generation;
        close.jobs.cancel(id);
        close.removing.remove(&id);
        close.owners.insert(
            id,
            Owner {
                root: self.cloud_prototype.root.clone(),
                session: self.active_session.as_ref().map(|session| session.session_id.clone()),
                issue: id,
                workspace: group.workspace.clone(),
                launch: launch.clone(),
                generation,
            },
        );
        true
    }

    pub(super) fn removal_owner_matches(&self, id: u32) -> bool {
        let Some(owner) = self.cloud_prototype.production.close.owners.get(&id) else {
            return false;
        };
        self.current_removal_owner(owner)
    }

    fn current_removal_owner(&self, owner: &Owner) -> bool {
        owner.root == self.cloud_prototype.root
            && owner.session.as_deref() == self.active_session.as_ref().map(|session| session.session_id.as_str())
            && self.cloud_prototype.groups.0.iter().any(|group| {
                group.issue == owner.issue
                    && group.workspace == owner.workspace
                    && group.remote.as_ref() == Some(&owner.launch)
            })
    }

    pub(in crate::app::cloud_panel::production) fn begin_cloud_removal(
        &mut self,
        id: u32,
        origin: Origin,
        ctx: &egui::Context,
    ) -> RemovalStart {
        if self
            .cloud_prototype
            .production
            .runtimes
            .get(&id)
            .is_some_and(|runtime| runtime.receiver.is_some() || runtime.busy())
        {
            return RemovalStart::Refused;
        }
        if self.cloud_prototype.production.close.removing.contains_key(&id) {
            return RemovalStart::Pending;
        }
        if !self.pin_removal_owner(id) {
            return RemovalStart::Refused;
        }
        let close = &mut self.cloud_prototype.production.close;
        let Some(owner) = close.owners.get(&id).cloned() else {
            return RemovalStart::Refused;
        };
        close.removing.insert(id, origin);
        close.records.insert(id, Record::Checking);
        let purpose = if origin == Origin::Anyway {
            Purpose::RemoveAnyway
        } else {
            Purpose::Remove
        };
        close.jobs.request(owner, purpose, ctx);
        RemovalStart::Pending
    }

    pub(in crate::app::cloud_panel::production) fn poll_cloud_removals(&mut self, ctx: &egui::Context) {
        let completed = self.cloud_prototype.production.close.jobs.poll(ctx);
        for completion in completed {
            let id = completion.owner.issue;
            if self.cloud_prototype.production.close.owners.get(&id) != Some(&completion.owner) {
                continue;
            }
            if !self.current_removal_owner(&completion.owner) {
                self.restore_closing_panels(id);
                self.cloud_prototype.production.close.deleting.remove(&id);
                self.end_close_confirmation(id);
                continue;
            }
            match completion.outcome {
                Outcome::Display(snapshot) => {
                    self.cloud_prototype
                        .production
                        .close
                        .records
                        .insert(id, record(snapshot));
                }
                Outcome::Removal(result) => {
                    let Some(origin) = self.cloud_prototype.production.close.removing.remove(&id) else {
                        continue;
                    };
                    if self
                        .cloud_prototype
                        .production
                        .runtimes
                        .get(&id)
                        .is_some_and(|runtime| runtime.receiver.is_some() || runtime.busy())
                    {
                        self.removal_refused(id, origin, Refusal::Busy);
                        continue;
                    }
                    match result {
                        Ok(custody) => {
                            if let Some(index) =
                                self.cloud_prototype.groups.0.iter().position(|group| group.issue == id)
                            {
                                if completion.purpose == Purpose::RemoveAnyway {
                                    tracing::warn!(cloud = %completion.owner.launch.id, "removed from Horizon while its provider resources may remain");
                                }
                                self.discard_cloud(index, ctx);
                            }
                            // The worker releases its Store only after this exact UI application.
                            drop(custody);
                        }
                        Err(error) => self.removal_refused(id, origin, error),
                    }
                }
            }
        }
    }

    fn removal_refused(&mut self, id: u32, origin: Origin, error: Refusal) {
        let message = match error {
            Refusal::Holds => "Delete the worker and workspace storage before removing this cloud".into(),
            Refusal::Busy => "Another controller owns this cloud operation".into(),
            Refusal::Unreadable(message) => message,
        };
        self.cloud_removal_error(id, message.clone());
        if origin != Origin::Direct {
            if origin == Origin::AfterDelete {
                self.restore_closing_panels(id);
                self.cloud_prototype.production.close.deleting.remove(&id);
            }
            self.close_failed(id, "Could not remove the cloud", origin != Origin::Close, Some(message));
        }
    }
}

fn record(snapshot: Snapshot) -> Record {
    match snapshot {
        Snapshot::Empty => Record::Empty,
        Snapshot::Holds => Record::Holds,
        Snapshot::Busy => Record::Busy,
        Snapshot::Unreadable(message) => Record::Unreadable(message),
    }
}
