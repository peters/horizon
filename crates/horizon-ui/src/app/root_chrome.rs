use egui::{Pos2, Rect, Vec2};

use super::{SIDEBAR_WIDTH, TOOLBAR_HEIGHT};

pub(super) const ROOT_TOOLBAR_BUTTON_HEIGHT: f32 = 30.0;
pub(super) const ROOT_TOOLBAR_BUTTON_GAP: f32 = 8.0;
pub(super) const ROOT_TOOLBAR_FPS_WIDTH: f32 = 72.0;
pub(super) const ROOT_TOOLBAR_MENU_WIDTH: f32 = 76.0;
/// Opens a toolbar menu just below the toolbar's bottom edge instead of under
/// the toolbar, which is drawn above menus.
pub(super) const ROOT_TOOLBAR_MENU_GAP: f32 = ROOT_TOOLBAR_VERTICAL_PAD + 4.0;

const ROOT_TOOLBAR_HORIZONTAL_PAD: f32 = 14.0;
const ROOT_TOOLBAR_VERTICAL_PAD: f32 = 8.0;
const ROOT_TOOLBAR_CLUSTER_GAP: f32 = 12.0;
const ROOT_TOOLBAR_SEARCH_MIN_WIDTH: f32 = 180.0;
const ROOT_TOOLBAR_SEARCH_MAX_WIDTH: f32 = 420.0;
const ROOT_TOOLBAR_DEPENDENCIES_WIDTH: f32 = 128.0;
const ROOT_TOOLBAR_DEPENDENCIES_MARK_ONLY_WIDTH: f32 = 40.0;
const ROOT_TOOLBAR_NAME_WIDTH: f32 = 72.0;
const ROOT_TOOLBAR_TAGLINE_WIDTH: f32 = 152.0;
const SIDEBAR_WIDTH_RATIO: f32 = 0.18;

pub(super) const SIDEBAR_MIN_WIDTH: f32 = 168.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DependenciesButton {
    Labeled,
    MarkOnly,
}

impl DependenciesButton {
    pub(super) const fn width(self) -> f32 {
        match self {
            Self::Labeled => ROOT_TOOLBAR_DEPENDENCIES_WIDTH,
            Self::MarkOnly => ROOT_TOOLBAR_DEPENDENCIES_MARK_ONLY_WIDTH,
        }
    }
}

/// The parts of the root toolbar that give way on narrow windows. The
/// Dependencies and Menu buttons always show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RootToolbarItems {
    pub(super) tagline: bool,
    pub(super) fps_meter: bool,
    pub(super) dependencies: DependenciesButton,
}

impl RootToolbarItems {
    const NARROWEST: Self = Self {
        tagline: false,
        fps_meter: false,
        dependencies: DependenciesButton::MarkOnly,
    };

    /// From widest to narrowest: the tagline goes first, then the fps meter,
    /// then the Dependencies label.
    const NARROWING: [Self; 4] = [
        Self {
            tagline: true,
            fps_meter: true,
            dependencies: DependenciesButton::Labeled,
        },
        Self {
            tagline: false,
            fps_meter: true,
            dependencies: DependenciesButton::Labeled,
        },
        Self {
            tagline: false,
            fps_meter: false,
            dependencies: DependenciesButton::Labeled,
        },
        Self::NARROWEST,
    ];

    fn brand_width(self) -> f32 {
        ROOT_TOOLBAR_NAME_WIDTH
            + if self.tagline {
                ROOT_TOOLBAR_BUTTON_GAP + ROOT_TOOLBAR_TAGLINE_WIDTH
            } else {
                0.0
            }
    }

    fn actions_width(self) -> f32 {
        let fps_meter = if self.fps_meter {
            ROOT_TOOLBAR_FPS_WIDTH + ROOT_TOOLBAR_BUTTON_GAP
        } else {
            0.0
        };
        fps_meter + self.dependencies.width() + ROOT_TOOLBAR_BUTTON_GAP + ROOT_TOOLBAR_MENU_WIDTH
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct RootToolbarLayout {
    pub(super) brand_rect: Rect,
    pub(super) search_rect: Rect,
    pub(super) actions_rect: Rect,
    pub(super) items: RootToolbarItems,
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

    let mut fallback = layout_candidate(viewport, content_rect, RootToolbarItems::NARROWEST);

    for items in RootToolbarItems::NARROWING {
        let candidate = layout_candidate(viewport, content_rect, items);
        if candidate.search_available >= ROOT_TOOLBAR_SEARCH_MIN_WIDTH {
            return candidate.layout;
        }
        fallback = candidate;
    }

    fallback.layout
}

fn layout_candidate(viewport: Rect, content_rect: Rect, items: RootToolbarItems) -> RootToolbarCandidate {
    let brand_rect = Rect::from_min_size(content_rect.min, Vec2::new(items.brand_width(), content_rect.height()));

    let actions_width = items.actions_width();
    let actions_rect = Rect::from_min_size(
        Pos2::new(content_rect.max.x - actions_width, content_rect.min.y),
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
            items,
        },
        search_available,
    }
}

#[cfg(test)]
mod tests {
    use egui::{Pos2, Rect};

    use super::{
        DependenciesButton, ROOT_TOOLBAR_SEARCH_MIN_WIDTH, RootToolbarItems, RootToolbarLayout,
        effective_sidebar_width, root_toolbar_layout,
    };
    use crate::app::{SIDEBAR_WIDTH, TOOLBAR_HEIGHT};

    fn layout_at(width: f32) -> RootToolbarLayout {
        root_toolbar_layout(Rect::from_min_max(Pos2::ZERO, Pos2::new(width, 768.0)))
    }

    /// Window widths from 1680 down to 200 in 5 point steps.
    fn narrowing_widths() -> impl Iterator<Item = f32> {
        (0_u16..=296).map(|step| 1680.0 - f32::from(step) * 5.0)
    }

    #[test]
    fn sidebar_width_shrinks_on_narrow_desktop_viewports() {
        assert!((effective_sidebar_width(1600.0) - SIDEBAR_WIDTH).abs() <= f32::EPSILON);
        assert!(effective_sidebar_width(1024.0) < SIDEBAR_WIDTH);
        assert!((effective_sidebar_width(800.0) - super::SIDEBAR_MIN_WIDTH).abs() <= f32::EPSILON);
    }

    #[test]
    fn toolbar_shows_every_item_on_desktop_widths() {
        for width in [1024.0, 1280.0, 1440.0, 1680.0] {
            let layout = layout_at(width);

            assert!(layout.items.tagline, "{width}");
            assert!(layout.items.fps_meter, "{width}");
            assert_eq!(layout.items.dependencies, DependenciesButton::Labeled, "{width}");
            assert!(layout.search_rect.width() >= ROOT_TOOLBAR_SEARCH_MIN_WIDTH, "{width}");
        }
    }

    #[test]
    fn toolbar_keeps_fps_meter_and_dependencies_label_at_the_minimum_window_width() {
        for width in [760.0, 800.0] {
            let layout = layout_at(width);

            assert!(layout.items.fps_meter, "{width}");
            assert_eq!(layout.items.dependencies, DependenciesButton::Labeled, "{width}");
            assert!(layout.search_rect.width() >= ROOT_TOOLBAR_SEARCH_MIN_WIDTH, "{width}");
            assert!((layout.search_rect.center().y - TOOLBAR_HEIGHT * 0.5).abs() <= f32::EPSILON);
        }
    }

    #[test]
    fn toolbar_gives_way_tagline_then_fps_meter_then_dependencies_label() {
        let mut seen: Vec<RootToolbarItems> = Vec::new();
        for width in narrowing_widths() {
            let items = layout_at(width).items;
            if seen.last() != Some(&items) {
                seen.push(items);
            }
        }

        assert_eq!(seen, RootToolbarItems::NARROWING);
    }

    #[test]
    fn toolbar_keeps_search_minimum_until_nothing_else_can_give_way() {
        for width in narrowing_widths() {
            let layout = layout_at(width);
            if layout.items != RootToolbarItems::NARROWEST {
                assert!(layout.search_rect.width() >= ROOT_TOOLBAR_SEARCH_MIN_WIDTH, "{width}");
            }
        }
    }

    #[test]
    fn toolbar_clusters_never_overlap_and_fit_the_window() {
        for width in narrowing_widths().filter(|width| *width >= 760.0) {
            let layout = layout_at(width);

            assert!(layout.brand_rect.min.x >= 0.0, "{width}");
            assert!(layout.brand_rect.max.x <= layout.search_rect.min.x, "{width}");
            assert!(layout.search_rect.max.x <= layout.actions_rect.min.x, "{width}");
            assert!(layout.actions_rect.max.x <= width, "{width}");
            assert!(
                (layout.actions_rect.width() - layout.items.actions_width()).abs() <= f32::EPSILON,
                "{width}"
            );
        }
    }
}
