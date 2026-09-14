//! Explicit, single-use saved Shell intent; all storage work uses the shared worker.

mod paint;

use super::{ClientContext, Completion, Context, HorizonHome, ReopenState, RequestScope};
use horizon_core::remote_workspace::{
    RemotePanelCommand,
    panels::{PreparedRemoteShellPanel, RemoteShellPanelDraft, add_remote_shell_panel, prepare_remote_shell_panel},
};

#[derive(Clone, Copy)]
pub(in crate::app::remote_environments) enum Action {
    Open,
    Prepare,
    Confirm,
    Cancel,
}

#[derive(Default)]
pub(super) struct AddState {
    form: Option<Form>,
    scope: Option<Scope>,
    confirmation: Option<Box<PreparedRemoteShellPanel>>,
}

struct Form {
    program: String,
    arguments: String,
    directory: String,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            program: "/bin/bash".into(),
            arguments: "-l".into(),
            directory: String::new(),
        }
    }
}

#[derive(Clone)]
struct Scope {
    request: RequestScope,
    home: HorizonHome,
}

impl Scope {
    fn matches(&self, client: &ClientContext<'_>) -> bool {
        self.home.root() == client.home.root() && self.request.matches(client)
    }
}

pub(super) struct PendingAdd {
    scope: Scope,
    saving: bool,
}

impl PendingAdd {
    pub(super) fn matches(&self, client: &ClientContext<'_>) -> bool {
        self.scope.matches(client)
    }

    pub(super) fn saving(&self) -> bool {
        self.saving
    }

    pub(super) fn label(&self) -> &'static str {
        if self.saving {
            "Saving the independent Shell panel…"
        } else {
            "Preparing the independent Shell panel confirmation…"
        }
    }
}

impl AddState {
    pub(super) fn cancel(&mut self) -> bool {
        let changed = self.scope.is_some();
        *self = Self::default();
        changed
    }
}

impl ReopenState {
    pub(super) fn add_action(&mut self, action: Action, client: &ClientContext<'_>, ctx: &Context) {
        if matches!(action, Action::Cancel) {
            self.add.cancel();
            if let Some(pending) = &mut self.pending
                && pending.add.as_ref().is_some_and(|add| !add.saving)
            {
                pending.discard = true;
            }
            ctx.request_repaint();
            return;
        }
        if !cfg!(target_os = "linux") {
            self.set_notice(
                "Adding independent Shell panels is currently supported on Linux clients.",
                ctx,
            );
            return;
        }
        if self.is_pending() {
            return;
        }
        match action {
            Action::Open => match RequestScope::current(client) {
                Ok(request) => {
                    self.start.cancel();
                    self.add = AddState {
                        form: Some(Form::default()),
                        scope: Some(Scope {
                            request,
                            home: client.home.clone(),
                        }),
                        confirmation: None,
                    };
                    self.add_notice = None;
                    self.notice = None;
                    self.repaint_context = Some(ctx.clone());
                    ctx.request_repaint();
                }
                Err(message) => self.set_notice(message, ctx),
            },
            Action::Prepare | Action::Confirm => self.dispatch_add(action, client, ctx),
            Action::Cancel => {}
        }
    }

    fn dispatch_add(&mut self, action: Action, client: &ClientContext<'_>, ctx: &Context) {
        let Some(scope) = self.add.scope.clone() else { return };
        if !scope.matches(client) {
            self.invalidate();
            self.set_notice(
                "The session or saved selection changed. Add the Shell panel again.",
                ctx,
            );
            return;
        }
        let request = scope.request.clone();
        let saving = matches!(action, Action::Confirm);
        if saving {
            let Some(prepared) = self.add.confirmation.take() else {
                return;
            };
            self.add.cancel();
            let home = client.home.clone();
            self.spawn(client.home, request.clone(), ctx, move |_| {
                let store = horizon_core::cloud_run::CloudWorkflowStore::open_existing_without_migration(&home)
                    .map_err(|_| "The saved environment could not be safely opened for update.".to_string())?;
                add_remote_shell_panel(&store, &request.owner, &request.expected, *prepared)
                    .map(Box::new)
                    .map(Completion::PanelAdded)
                    .map_err(|error| error.to_string())
            });
        } else {
            let Some(form) = &self.add.form else { return };
            let draft = RemoteShellPanelDraft {
                command: RemotePanelCommand {
                    program: form.program.clone(),
                    args: form.arguments.lines().map(str::to_owned).collect(),
                },
                working_directory: (!form.directory.is_empty()).then(|| form.directory.clone()),
            };
            self.add.confirmation = None;
            self.spawn(client.home, request.clone(), ctx, move |store| {
                prepare_remote_shell_panel(store, &request.owner, &request.expected, draft)
                    .map(Box::new)
                    .map(Completion::PanelPreview)
                    .map_err(|error| error.to_string())
            });
        }
        if let Some(pending) = &mut self.pending {
            pending.add = Some(PendingAdd { scope, saving });
        }
    }

    pub(super) fn accept_add(&mut self, pending: &PendingAdd, result: Result<Completion, String>) {
        if pending.saving {
            self.refresh_inventory = true;
            self.catalog = None;
            self.add_notice = Some(match result {
                Ok(Completion::PanelAdded(added))
                    if added.environment.owning_session_id == pending.scope.request.owner
                        && added.environment.workspace_local_id == pending.scope.request.expected.workspace_local_id =>
                {
                    "Independent Shell panel saved. Show saved panels to start its task or reopen its disconnected view.".into()
                }
                Err(message) => format!("{message}. Refresh saved panels before another addition. No retry was scheduled."),
                _ => "The Shell save outcome is unknown. Refresh saved panels before another addition. No retry was scheduled.".into(),
            });
        } else {
            match result {
                Ok(Completion::PanelPreview(prepared))
                    if *prepared.environment() == pending.scope.request.expected && self.add.scope.is_some() =>
                {
                    self.add.confirmation = Some(prepared);
                }
                result => {
                    self.notice = Some(
                        result
                            .err()
                            .unwrap_or_else(|| "The Shell preview did not match its request.".into()),
                    );
                }
            }
        }
    }

    pub(in crate::app::remote_environments) fn show_add(
        &mut self,
        ui: &mut egui::Ui,
        enabled: bool,
        action: &mut super::InventoryAction,
    ) {
        if !cfg!(target_os = "linux") {
            ui.label("Adding independent Shell panels is currently supported on Linux clients.");
        }
        let enabled = enabled && !self.is_pending() && cfg!(target_os = "linux");
        paint::show(ui, &mut self.add, enabled, action);
        if let Some(notice) = &self.add_notice {
            ui.label(notice);
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
