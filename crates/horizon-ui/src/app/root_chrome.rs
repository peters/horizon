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
/// "+ New workspace · Cloud ▾"; the compact form drops "workspace".
pub(super) const ROOT_TOOLBAR_NEW_WIDTH: f32 = 214.0;
pub(super) const ROOT_TOOLBAR_NEW_COMPACT_WIDTH: f32 = 140.0;
pub(super) const ROOT_TOOLBAR_MENU_WIDTH: f32 = 82.0;
const ROOT_TOOLBAR_NAME_WIDTH: f32 = 72.0;
const ROOT_TOOLBAR_TAGLINE_WIDTH: f32 = 152.0;
const SIDEBAR_WIDTH_RATIO: f32 = 0.18;

pub(super) const SIDEBAR_MIN_WIDTH: f32 = 168.0;

/// The actions the toolbar's Menu carries out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ToolbarAction {
    QuickNav,
    RemoteHosts,
    #[cfg(feature = "cloud-workspaces")]
    Cloud,
    Sessions,
    Settings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ToolbarItem {
    FpsMeter,
    /// The primary New workspace button; `compact` drops the word "workspace".
    NewWorkspace {
        compact: bool,
    },
    Menu,
}

impl ToolbarItem {
    fn estimated_width(self) -> f32 {
        match self {
            Self::FpsMeter => ROOT_TOOLBAR_FPS_WIDTH,
            Self::NewWorkspace { compact: false } => ROOT_TOOLBAR_NEW_WIDTH,
            Self::NewWorkspace { compact: true } => ROOT_TOOLBAR_NEW_COMPACT_WIDTH,
            Self::Menu => ROOT_TOOLBAR_MENU_WIDTH,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct RootToolbarLayout {
    pub(super) brand_rect: Rect,
    pub(super) search_rect: Rect,
    pub(super) actions_rect: Rect,
    pub(super) visible_items: Vec<ToolbarItem>,
    pub(super) show_tagline: bool,
}

struct RootToolbarCandidate {
    layout: RootToolbarLayout,
    search_available: f32,
}

pub(super) fn effective_sidebar_width(viewport_width: f32) -> f32 {
    (viewport_width * SIDEBAR_WIDTH_RATIO).clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_WIDTH)
}

pub(super) fn root_toolbar_layout(viewport: Rect) -> RootToolbarLayout {
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

    // (tagline, fps meter, compact New workspace), from roomiest to tightest.
    let states = [
        (true, true, false),
        (false, true, false),
        (false, false, false),
        (false, false, true),
    ];
    let mut fallback = layout_candidate(viewport, content_rect, false, false, true);

    for (show_tagline, show_fps, compact) in states {
        let candidate = layout_candidate(viewport, content_rect, show_tagline, show_fps, compact);
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
    show_fps: bool,
    compact: bool,
) -> RootToolbarCandidate {
    let brand_width = ROOT_TOOLBAR_NAME_WIDTH
        + if show_tagline {
            ROOT_TOOLBAR_BUTTON_GAP + ROOT_TOOLBAR_TAGLINE_WIDTH
        } else {
            0.0
        };
    let brand_rect = Rect::from_min_size(content_rect.min, Vec2::new(brand_width, content_rect.height()));

    let mut visible_items = Vec::with_capacity(3);
    if show_fps {
        visible_items.push(ToolbarItem::FpsMeter);
    }
    visible_items.push(ToolbarItem::NewWorkspace { compact });
    visible_items.push(ToolbarItem::Menu);

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

#[cfg(test)]
mod tests {
    use egui::{Pos2, Rect};

    use super::{ToolbarItem, effective_sidebar_width, root_toolbar_layout};
    use crate::app::{SIDEBAR_WIDTH, TOOLBAR_HEIGHT};

    #[test]
    fn sidebar_width_shrinks_on_narrow_desktop_viewports() {
        assert!((effective_sidebar_width(1600.0) - SIDEBAR_WIDTH).abs() <= f32::EPSILON);
        assert!(effective_sidebar_width(1024.0) < SIDEBAR_WIDTH);
        assert!((effective_sidebar_width(800.0) - super::SIDEBAR_MIN_WIDTH).abs() <= f32::EPSILON);
    }

    #[test]
    fn toolbar_keeps_short_tagline_when_search_still_fits() {
        let viewport = Rect::from_min_max(Pos2::ZERO, Pos2::new(1280.0, 768.0));
        let layout = root_toolbar_layout(viewport);

        assert!(layout.show_tagline);
        assert!(layout.visible_items.contains(&ToolbarItem::FpsMeter));
        assert!(
            layout
                .visible_items
                .contains(&ToolbarItem::NewWorkspace { compact: false })
        );
        assert!(layout.search_rect.width() >= 180.0);
    }

    #[test]
    fn toolbar_hides_tagline_before_the_fps_meter() {
        let viewport = Rect::from_min_max(Pos2::ZERO, Pos2::new(800.0, 768.0));
        let layout = root_toolbar_layout(viewport);

        assert!(!layout.show_tagline);
        assert!(layout.visible_items.contains(&ToolbarItem::FpsMeter));
        assert!(layout.search_rect.width() >= 180.0);
    }

    #[test]
    fn toolbar_keeps_new_workspace_and_menu_at_every_width() {
        for width in [760.0, 800.0, 900.0, 1024.0, 1280.0] {
            let layout = root_toolbar_layout(Rect::from_min_max(Pos2::ZERO, Pos2::new(width, 768.0)));
            let news = layout
                .visible_items
                .iter()
                .filter(|item| matches!(item, ToolbarItem::NewWorkspace { .. }))
                .count();
            assert_eq!(news, 1, "{width}");
            assert_eq!(layout.visible_items.last(), Some(&ToolbarItem::Menu), "{width}");
            assert!(layout.brand_rect.max.x <= layout.search_rect.min.x);
            assert!(layout.search_rect.max.x <= layout.actions_rect.min.x);
            assert!((layout.search_rect.center().y - TOOLBAR_HEIGHT * 0.5).abs() <= f32::EPSILON);
        }
    }
}
