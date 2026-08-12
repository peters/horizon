use std::collections::{HashMap, HashSet};

use crate::panel::{PanelKind, PanelResume};

use super::{
    AgentSessionBinding, AgentSessionBootstrapCatalog, PanelState, RuntimeState, agent_sessions, normalize_cwd,
};

impl RuntimeState {
    /// Repairs exact bindings that reference parent-controlled threads, then
    /// assigns catalog sessions to legacy `resume: last` panels that were
    /// persisted without a session binding.
    ///
    /// `busy_session_ids` lists sessions currently open in a running agent
    /// process (see [`super::live_claude_session_ids`]); those are never
    /// assigned so a restored panel cannot attach to a conversation that is
    /// already open elsewhere.
    pub fn bootstrap_missing_agent_bindings(
        &mut self,
        catalog: &AgentSessionBootstrapCatalog,
        busy_session_ids: &HashSet<String>,
    ) -> bool {
        self.ensure_local_ids();

        let mut used_session_ids = busy_session_ids.clone();
        let mut changed = self.validate_verified_bindings(catalog, &mut used_session_ids);
        changed |= self.repair_parent_controlled_exact_bindings(catalog, &mut used_session_ids);
        changed |= self.materialize_explicit_bindings(catalog, &mut used_session_ids);
        changed |= self.assign_last_bindings(catalog, &mut used_session_ids);
        changed
    }

    fn validate_verified_bindings(
        &mut self,
        catalog: &AgentSessionBootstrapCatalog,
        used_session_ids: &mut HashSet<String>,
    ) -> bool {
        let mut changed = false;
        for panel in self.workspaces.iter_mut().flat_map(|workspace| &mut workspace.panels) {
            if !panel.kind.requires_exact_session_validation() {
                continue;
            }
            let Some(session_id) = panel.stored_session_id().map(str::to_owned) else {
                continue;
            };
            if !matches!(
                catalog.exact_resolution(panel.kind, &session_id),
                agent_sessions::ExactSessionResolution::Verified
            ) {
                continue;
            }
            if used_session_ids.insert(session_id.clone()) {
                changed |= panel.ensure_session_binding(&session_id);
            } else {
                tracing::warn!(session_id, "discarding a duplicate exact session binding");
                changed |= neutralize_exact_session(panel);
            }
        }
        changed
    }

    fn repair_parent_controlled_exact_bindings(
        &mut self,
        catalog: &AgentSessionBootstrapCatalog,
        used_session_ids: &mut HashSet<String>,
    ) -> bool {
        let mut changed = false;
        for panel in self.workspaces.iter_mut().flat_map(|workspace| &mut workspace.panels) {
            if !panel.kind.requires_exact_session_validation() {
                continue;
            }
            let Some(session_id) = panel.stored_session_id().map(str::to_owned) else {
                continue;
            };
            match catalog.exact_resolution(panel.kind, &session_id) {
                agent_sessions::ExactSessionResolution::Rebind(canonical_binding)
                    if used_session_ids.insert(canonical_binding.session_id.clone()) =>
                {
                    changed |= panel.replace_session_binding(canonical_binding);
                }
                agent_sessions::ExactSessionResolution::Rebind(canonical_binding) => {
                    tracing::warn!(
                        child_session_id = session_id,
                        canonical_session_id = canonical_binding.session_id,
                        "discarding a parent-controlled binding because its root is already in use"
                    );
                    changed |= neutralize_exact_session(panel);
                }
                agent_sessions::ExactSessionResolution::Stale => {
                    tracing::info!(session_id, "discarding a stale exact session binding");
                    changed |= neutralize_exact_session(panel);
                }
                // Leave unverified ids intact. The caller must block startup
                // until the operator retries validation or explicitly chooses
                // the scoped safe-open recovery path.
                agent_sessions::ExactSessionResolution::Verified
                | agent_sessions::ExactSessionResolution::Unavailable => {}
            }
        }
        changed
    }

    fn materialize_explicit_bindings(
        &mut self,
        catalog: &AgentSessionBootstrapCatalog,
        used_session_ids: &mut HashSet<String>,
    ) -> bool {
        let mut changed = false;
        for panel in self.workspaces.iter_mut().flat_map(|workspace| &mut workspace.panels) {
            if !panel.kind.supports_session_binding() {
                continue;
            }
            if panel.kind.requires_exact_session_validation()
                && panel.stored_session_id().is_some_and(|session_id| {
                    matches!(
                        catalog.exact_resolution(panel.kind, session_id),
                        agent_sessions::ExactSessionResolution::Unavailable
                            | agent_sessions::ExactSessionResolution::Stale
                    )
                })
            {
                continue;
            }
            if panel.session_binding.is_none()
                && let PanelResume::Session { session_id } = &panel.resume
            {
                panel.session_binding = Some(AgentSessionBinding::new(
                    panel.kind,
                    session_id.clone(),
                    panel.cwd.clone(),
                    Some(panel.name.clone()),
                    None,
                ));
                changed = true;
            }
            if let Some(binding) = &panel.session_binding {
                used_session_ids.insert(binding.session_id.clone());
            }
        }
        changed
    }

    fn assign_last_bindings(
        &mut self,
        catalog: &AgentSessionBootstrapCatalog,
        used_session_ids: &mut HashSet<String>,
    ) -> bool {
        let mut pending_by_group: HashMap<(PanelKind, String), Vec<&mut PanelState>> = HashMap::new();
        for panel in self.workspaces.iter_mut().flat_map(|workspace| &mut workspace.panels) {
            if panel.kind.supports_session_binding()
                && panel.session_binding.is_none()
                && matches!(panel.resume, PanelResume::Last)
            {
                let cwd = normalize_cwd(panel.cwd.as_deref()).unwrap_or_default();
                pending_by_group.entry((panel.kind, cwd)).or_default().push(panel);
            }
        }

        let mut pending_groups: Vec<_> = pending_by_group.into_iter().collect();
        pending_groups.sort_by(|((left_kind, left_cwd), _), ((right_kind, right_cwd), _)| {
            left_cwd
                .is_empty()
                .cmp(&right_cwd.is_empty())
                .then_with(|| left_kind.display_name().cmp(right_kind.display_name()))
                .then_with(|| left_cwd.cmp(right_cwd))
        });

        let mut changed = false;
        for ((kind, cwd), panels) in pending_groups {
            let mut candidates = catalog.recent_for(kind, empty_to_none(&cwd));
            candidates.retain(|candidate| !used_session_ids.contains(&candidate.session_id));
            for (panel, candidate) in panels.into_iter().zip(candidates) {
                used_session_ids.insert(candidate.session_id.clone());
                panel.session_binding = Some(candidate.into_binding());
                changed = true;
            }
        }
        changed
    }

    /// Remove only exact resumes that could not be validated.
    ///
    /// This is the explicit recovery path offered after startup validation
    /// fails. Explicit session requests become fresh launches; `resume: last`
    /// remains configured but loses the unsafe captured binding.
    pub fn neutralize_unverified_session_bindings(&mut self, unavailable_session_ids: &HashSet<String>) -> bool {
        let mut changed = false;
        for panel in self.workspaces.iter_mut().flat_map(|workspace| &mut workspace.panels) {
            if !panel.kind.requires_exact_session_validation()
                || !panel
                    .stored_session_id()
                    .is_some_and(|session_id| unavailable_session_ids.contains(session_id))
            {
                continue;
            }
            changed |= neutralize_exact_session(panel);
        }
        changed
    }

    /// All persisted exact ids whose provider requires validation before
    /// launch. This is the conservative recovery scope when validation stops
    /// before it can report per-id results.
    #[must_use]
    pub fn exact_session_ids_requiring_validation(&self) -> HashSet<String> {
        self.workspaces
            .iter()
            .flat_map(|workspace| &workspace.panels)
            .filter(|panel| panel.kind.requires_exact_session_validation())
            .filter_map(PanelState::stored_session_id)
            .map(str::to_owned)
            .collect()
    }
}

fn neutralize_exact_session(panel: &mut PanelState) -> bool {
    let mut changed = panel.session_binding.take().is_some();
    if matches!(panel.resume, PanelResume::Session { .. }) {
        panel.resume = PanelResume::Fresh;
        changed = true;
    }
    changed
}

fn empty_to_none(value: &str) -> Option<&str> {
    if value.is_empty() { None } else { Some(value) }
}
