//! Preserve cloud metadata even in builds that cannot operate cloud workers.
#[cfg(feature = "cloud-workspaces")]
pub type CloudGroupsState = crate::cloud_panel::CloudGroups;
#[cfg(not(feature = "cloud-workspaces"))]
pub type CloudGroupsState = Vec<serde_json::Value>;

pub(crate) fn managed_member(groups: &CloudGroupsState, panel: &str) -> bool {
    #[cfg(feature = "cloud-workspaces")]
    {
        groups
            .0
            .iter()
            .any(|group| group.remote.is_some() && group.panels.iter().any(|id| id == panel))
    }
    #[cfg(not(feature = "cloud-workspaces"))]
    {
        groups.iter().any(|group| {
            group.get("remote").is_some_and(|remote| !remote.is_null())
                && group
                    .get("panels")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|panels| panels.iter().any(|id| id.as_str() == Some(panel)))
        })
    }
}

pub(crate) fn contains_panel(groups: &CloudGroupsState, panel: &str) -> bool {
    #[cfg(feature = "cloud-workspaces")]
    {
        groups.0.iter().any(|group| group.panels.iter().any(|id| id == panel))
    }
    #[cfg(not(feature = "cloud-workspaces"))]
    {
        groups.iter().any(|group| {
            group
                .get("panels")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|panels| panels.iter().any(|id| id.as_str() == Some(panel)))
        })
    }
}

pub(crate) fn contains_workspace(groups: &CloudGroupsState, workspace: &str) -> bool {
    workspace_ids(groups).any(|id| id == workspace)
}

pub(crate) fn workspace_ids(groups: &CloudGroupsState) -> impl Iterator<Item = &str> {
    #[cfg(feature = "cloud-workspaces")]
    {
        groups.0.iter().map(|group| group.workspace.as_str())
    }
    #[cfg(not(feature = "cloud-workspaces"))]
    {
        groups
            .iter()
            .filter_map(|group| group.get("workspace").and_then(serde_json::Value::as_str))
    }
}

#[cfg(all(test, not(feature = "cloud-workspaces")))]
mod tests;
