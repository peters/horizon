use egui::{Pos2, Rect, Vec2};

use super::{SIDEBAR_WIDTH, TOOLBAR_HEIGHT};

pub(super) const ROOT_TOOLBAR_BUTTON_HEIGHT: f32 = 30.0;
pub(super) const ROOT_TOOLBAR_BUTTON_GAP: f32 = 8.0;

const ROOT_TOOLBAR_HORIZONTAL_PAD: f32 = 14.0;
const ROOT_TOOLBAR_VERTICAL_PAD: f32 = 8.0;
const ROOT_TOOLBAR_CLUSTER_GAP: f32 = 12.0;
const ROOT_TOOLBAR_SEARCH_MIN_WIDTH: f32 = 180.0;
const ROOT_TOOLBAR_SEARCH_MAX_WIDTH: f32 = 420.0;
pub(super) const ROOT_TOOLBAR_FPS_WIDTH: f32 = 72.0;
const ROOT_TOOLBAR_MORE_WIDTH: f32 = 72.0;
const ROOT_TOOLBAR_NAME_WIDTH: f32 = 72.0;
const ROOT_TOOLBAR_TAGLINE_WIDTH: f32 = 324.0;
const SIDEBAR_WIDTH_RATIO: f32 = 0.18;

pub(super) const SIDEBAR_MIN_WIDTH: f32 = 168.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ToolbarAction {
    NewWorkspace,
    QuickNav,
    FitWorkspace,
    RemoteHosts,
    Sessions,
    Update,
    Settings,
}

impl ToolbarAction {
    const SECONDARY: [Self; 2] = [Self::FitWorkspace, Self::RemoteHosts];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::NewWorkspace => "New Workspace",
            Self::QuickNav => "Quick Nav",
            Self::FitWorkspace => "Fit Workspace",
            Self::RemoteHosts => "Remote Hosts",
            Self::Sessions => "Sessions",
            Self::Update => "Update",
            Self::Settings => "Settings",
        }
    }

    fn estimated_width(self) -> f32 {
        estimated_toolbar_button_width(self.label())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ToolbarItem {
    FpsMeter,
    Action(ToolbarAction),
    OverflowMenu,
}

impl ToolbarItem {
    fn estimated_width(self) -> f32 {
        match self {
            Self::FpsMeter => ROOT_TOOLBAR_FPS_WIDTH,
            Self::Action(action) => action.estimated_width(),
            Self::OverflowMenu => ROOT_TOOLBAR_MORE_WIDTH,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct RootToolbarLayout {
    pub(super) brand_rect: Rect,
    pub(super) search_rect: Rect,
    pub(super) actions_rect: Rect,
    pub(super) visible_items: Vec<ToolbarItem>,
    pub(super) overflow_actions: Vec<ToolbarAction>,
    pub(super) show_tagline: bool,
}

struct RootToolbarCandidate {
    layout: RootToolbarLayout,
    search_available: f32,
}

pub(super) fn effective_sidebar_width(viewport_width: f32) -> f32 {
    (viewport_width * SIDEBAR_WIDTH_RATIO).clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_WIDTH)
}

pub(super) fn root_toolbar_layout(viewport: Rect, show_update: bool) -> RootToolbarLayout {
    let content_rect = Rect::from_min_max(
        Pos2::new(
            viewport.min.x + ROOT_TOOLBAR_HORIZONTAL_PAD,
            viewport.min.y + ROOT_TOOLBAR_VERTICAL_PAD,
        ),
        Pos2::new(
            viewport.max.x - ROOT_TOOLBAR_HORIZONTAL_PAD,
            viewport.min.y + TOOLBAR_HEIGHT - ROOT_TOOLBAR_VERTICAL_PAD,
        ),
    );

    let states = [
        (true, 2_usize, true),
        (false, 2_usize, true),
        (false, 1_usize, true),
        (false, 0_usize, true),
        (false, 2_usize, false),
        (false, 1_usize, false),
        (false, 0_usize, false),
    ];
    let mut fallback = layout_candidate(viewport, content_rect, false, 0, false, show_update);

    for (show_tagline, secondary_visible, show_fps) in states {
        let candidate = layout_candidate(
            viewport,
            content_rect,
            show_tagline,
            secondary_visible,
            show_fps,
            show_update,
        );
        if candidate.search_available >= ROOT_TOOLBAR_SEARCH_MIN_WIDTH {
            return candidate.layout;
        }
        fallback = candidate;
    }

    fallback.layout
}

fn layout_candidate(
    viewport: Rect,
    content_rect: Rect,
    show_tagline: bool,
    secondary_visible: usize,
    show_fps: bool,
    show_update: bool,
) -> RootToolbarCandidate {
    let brand_width = ROOT_TOOLBAR_NAME_WIDTH
        + if show_tagline {
            ROOT_TOOLBAR_BUTTON_GAP + ROOT_TOOLBAR_TAGLINE_WIDTH
        } else {
            0.0
        };
    let brand_rect = Rect::from_min_size(content_rect.min, Vec2::new(brand_width, content_rect.height()));

    let mut visible_items = Vec::with_capacity(6);
    if show_fps {
        visible_items.push(ToolbarItem::FpsMeter);
    }
    visible_items.push(ToolbarItem::Action(ToolbarAction::NewWorkspace));
    visible_items.push(ToolbarItem::Action(ToolbarAction::QuickNav));
    for action in ToolbarAction::SECONDARY.iter().take(secondary_visible).copied() {
        visible_items.push(ToolbarItem::Action(action));
    }

    let overflow_actions = ToolbarAction::SECONDARY
        .iter()
        .skip(secondary_visible)
        .copied()
        .collect::<Vec<_>>();
    if !overflow_actions.is_empty() {
        visible_items.push(ToolbarItem::OverflowMenu);
    }
    if show_update {
        visible_items.push(ToolbarItem::Action(ToolbarAction::Update));
    }
    visible_items.push(ToolbarItem::Action(ToolbarAction::Sessions));
    visible_items.push(ToolbarItem::Action(ToolbarAction::Settings));

    let actions_width = visible_items_width(&visible_items);
    let actions_left = content_rect.max.x - actions_width;
    let actions_rect = Rect::from_min_size(
        Pos2::new(actions_left, content_rect.min.y),
        Vec2::new(actions_width, content_rect.height()),
    );

    let search_left_bound = brand_rect.max.x + ROOT_TOOLBAR_CLUSTER_GAP;
    let search_right_bound = actions_rect.min.x - ROOT_TOOLBAR_CLUSTER_GAP;
    let search_available = (search_right_bound - search_left_bound).max(0.0);
    let search_width = search_available.min(ROOT_TOOLBAR_SEARCH_MAX_WIDTH);
    let search_left = search_left_bound + ((search_available - search_width) * 0.5);
    let search_rect = Rect::from_center_size(
        Pos2::new(search_left + search_width * 0.5, viewport.min.y + TOOLBAR_HEIGHT * 0.5),
        Vec2::new(search_width, TOOLBAR_HEIGHT - 14.0),
    );

    RootToolbarCandidate {
        layout: RootToolbarLayout {
            brand_rect,
            search_rect,
            actions_rect,
            visible_items,
            overflow_actions,
            show_tagline,
        },
        search_available,
    }
}

fn visible_items_width(items: &[ToolbarItem]) -> f32 {
    let gaps = items
        .len()
        .saturating_sub(1)
        .try_into()
        .map_or(0.0, |count: u16| f32::from(count) * ROOT_TOOLBAR_BUTTON_GAP);
    items.iter().map(|item| item.estimated_width()).sum::<f32>() + gaps
}

fn estimated_toolbar_button_width(label: &str) -> f32 {
    let chars = u16::try_from(label.chars().count()).map_or(f32::from(u16::MAX), f32::from);
    34.0 + chars * 7.2
}

#[cfg(test)]
mod tests {
    use egui::{Pos2, Rect};

    use super::{ToolbarAction, ToolbarItem, effective_sidebar_width, root_toolbar_layout};
    use crate::app::{SIDEBAR_WIDTH, TOOLBAR_HEIGHT};

    #[test]
    fn sidebar_width_shrinks_on_narrow_desktop_viewports() {
        assert!((effective_sidebar_width(1600.0) - SIDEBAR_WIDTH).abs() <= f32::EPSILON);
        assert!(effective_sidebar_width(1024.0) < SIDEBAR_WIDTH);
        assert!((effective_sidebar_width(800.0) - super::SIDEBAR_MIN_WIDTH).abs() <= f32::EPSILON);
    }

    #[test]
    fn toolbar_hides_tagline_before_collapsing_actions() {
        let viewport = Rect::from_min_max(Pos2::ZERO, Pos2::new(1024.0, 768.0));
        let layout = root_toolbar_layout(viewport, false);

        assert!(!layout.show_tagline);
        assert!(layout.visible_items.contains(&ToolbarItem::FpsMeter));
        assert!(
            layout
                .visible_items
                .contains(&ToolbarItem::Action(ToolbarAction::Sessions))
        );
        assert!(layout.search_rect.width() >= 180.0);
    }

    #[test]
    fn toolbar_moves_secondary_actions_into_overflow_on_tighter_widths() {
        let viewport = Rect::from_min_max(Pos2::ZERO, Pos2::new(900.0, 768.0));
        let layout = root_toolbar_layout(viewport, false);

        assert!(!layout.show_tagline);
        assert!(layout.overflow_actions.contains(&ToolbarAction::RemoteHosts));
        assert!(layout.visible_items.contains(&ToolbarItem::FpsMeter));
        assert!(layout.visible_items.contains(&ToolbarItem::OverflowMenu));
        assert!((layout.search_rect.center().y - TOOLBAR_HEIGHT * 0.5).abs() <= f32::EPSILON);
    }

    #[test]
    fn toolbar_keeps_primary_actions_visible_at_min_window_width() {
        let viewport = Rect::from_min_max(Pos2::ZERO, Pos2::new(800.0, 600.0));
        let layout = root_toolbar_layout(viewport, false);

        assert_eq!(
            layout.overflow_actions,
            vec![ToolbarAction::FitWorkspace, ToolbarAction::RemoteHosts]
        );
        assert!(
            layout
                .visible_items
                .contains(&ToolbarItem::Action(ToolbarAction::NewWorkspace))
        );
        assert!(
            layout
                .visible_items
                .contains(&ToolbarItem::Action(ToolbarAction::QuickNav))
        );
        assert!(
            layout
                .visible_items
                .contains(&ToolbarItem::Action(ToolbarAction::Sessions))
        );
        assert!(
            layout
                .visible_items
                .contains(&ToolbarItem::Action(ToolbarAction::Settings))
        );
    }

    #[test]
    fn toolbar_keeps_update_visible_when_present() {
        let viewport = Rect::from_min_max(Pos2::ZERO, Pos2::new(800.0, 600.0));
        let layout = root_toolbar_layout(viewport, true);

        assert!(
            layout
                .visible_items
                .contains(&ToolbarItem::Action(ToolbarAction::Update))
        );
        assert_eq!(
            layout.overflow_actions,
            vec![ToolbarAction::FitWorkspace, ToolbarAction::RemoteHosts]
        );
    }
}
