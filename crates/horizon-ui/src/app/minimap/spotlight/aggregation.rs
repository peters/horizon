use std::{cmp::Ordering, collections::HashMap, time::SystemTime};

use horizon_core::{AttentionId, AttentionItem, AttentionSeverity, PanelId, WorkspaceId};

use crate::app::attention_feed::is_visible_attention_item;

use super::{HorizonApp, MinimapNavigationTarget, MinimapScope, scope_includes_workspace};

#[derive(Default)]
pub(in crate::app::minimap) struct MinimapSpotlight {
    pub(super) workspaces: HashMap<WorkspaceId, WorkspaceAttentionCue>,
}

#[derive(Clone, Debug)]
pub(super) struct WorkspaceAttentionCue {
    pub(super) display_severity: AttentionSeverity,
    pub(super) open_count: usize,
    pub(super) attention_id: AttentionId,
    pub(super) target: MinimapNavigationTarget,
    pub(super) panel_cues: Vec<PanelAttentionCue>,
}

#[derive(Clone, Debug)]
pub(super) struct PanelAttentionCue {
    pub(super) panel_id: PanelId,
    pub(super) display_severity: AttentionSeverity,
    priority_severity: AttentionSeverity,
    pub(super) attention_id: AttentionId,
    created_at: SystemTime,
}

impl MinimapSpotlight {
    pub(in crate::app::minimap) fn collect(app: &HorizonApp, scope: MinimapScope, now: SystemTime) -> Self {
        project_attention(
            &app.board.attention,
            now,
            app.template_config.features.attention_feed,
            |workspace_id| scope_includes_workspace(app, scope, workspace_id),
        )
    }
}

pub(super) fn aggregate_attention(
    attention: &[AttentionItem],
    now: SystemTime,
    includes_workspace: impl Fn(WorkspaceId) -> bool,
) -> MinimapSpotlight {
    let mut grouped = HashMap::<WorkspaceId, Vec<&AttentionItem>>::new();
    for item in attention {
        if includes_workspace(item.workspace_id) && is_visible_attention_item(item, now) {
            grouped.entry(item.workspace_id).or_default().push(item);
        }
    }

    let workspaces = grouped
        .into_iter()
        .filter_map(|(workspace_id, items)| {
            aggregate_workspace_attention(workspace_id, &items).map(|cue| (workspace_id, cue))
        })
        .collect();

    MinimapSpotlight { workspaces }
}

pub(super) fn project_attention(
    attention: &[AttentionItem],
    now: SystemTime,
    attention_enabled: bool,
    includes_workspace: impl Fn(WorkspaceId) -> bool,
) -> MinimapSpotlight {
    if attention_enabled {
        aggregate_attention(attention, now, includes_workspace)
    } else {
        MinimapSpotlight::default()
    }
}

fn aggregate_workspace_attention(workspace_id: WorkspaceId, items: &[&AttentionItem]) -> Option<WorkspaceAttentionCue> {
    let open_items: Vec<_> = items.iter().copied().filter(|item| item.is_open()).collect();
    let completed = open_items.is_empty();
    let selected = if completed { items } else { &open_items };
    let target_item = selected
        .iter()
        .copied()
        .max_by(|left, right| attention_priority(left, right))?;
    let display_severity = if completed {
        AttentionSeverity::Medium
    } else {
        target_item.severity
    };
    let target = target_item
        .panel_id
        .map_or(MinimapNavigationTarget::Workspace(workspace_id), |panel_id| {
            MinimapNavigationTarget::Panel { workspace_id, panel_id }
        });

    let mut panel_items = HashMap::<PanelId, &AttentionItem>::new();
    for item in selected.iter().copied() {
        let Some(panel_id) = item.panel_id else {
            continue;
        };
        let replace = panel_items
            .get(&panel_id)
            .is_none_or(|current| attention_priority(current, item) == Ordering::Less);
        if replace {
            panel_items.insert(panel_id, item);
        }
    }

    let mut panel_cues: Vec<_> = panel_items
        .into_iter()
        .map(|(panel_id, item)| PanelAttentionCue {
            panel_id,
            display_severity: if completed {
                AttentionSeverity::Medium
            } else {
                item.severity
            },
            priority_severity: item.severity,
            attention_id: item.id,
            created_at: item.created_at,
        })
        .collect();
    panel_cues.sort_by(|left, right| {
        right
            .priority_severity
            .cmp(&left.priority_severity)
            .then_with(|| right.created_at.cmp(&left.created_at))
            .then_with(|| right.panel_id.0.cmp(&left.panel_id.0))
    });
    Some(WorkspaceAttentionCue {
        display_severity,
        open_count: open_items.len(),
        attention_id: target_item.id,
        target,
        panel_cues,
    })
}

fn attention_priority(left: &AttentionItem, right: &AttentionItem) -> Ordering {
    left.severity
        .cmp(&right.severity)
        .then_with(|| left.created_at.cmp(&right.created_at))
        .then_with(|| left.id.0.cmp(&right.id.0))
}
