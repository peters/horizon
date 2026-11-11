//! The shapes and rectangles of the Settings panel, for its tests.
//!
//! The tests of the panel and of its footer read the painted shapes of a frame:
//! the text of a widget, the rectangle that a panel reserved, and the band of its
//! bar. These helpers read them the same way for both, so a test of the footer and
//! a test of a tab measure one picture.

/// Runs `visit` over every painted primitive, with the rectangle that it paints
/// and the rectangle that it is clipped to.
pub(super) fn for_each_painted(
    shapes: &[egui::epaint::ClippedShape],
    visit: &mut impl FnMut(&egui::Shape, egui::Rect, egui::Rect),
) {
    for clipped in shapes {
        visit_shape(&clipped.shape, clipped.clip_rect, visit);
    }
}

pub(super) fn visit_shape(
    shape: &egui::Shape,
    clip: egui::Rect,
    visit: &mut impl FnMut(&egui::Shape, egui::Rect, egui::Rect),
) {
    if let egui::Shape::Vec(shapes) = shape {
        for shape in shapes {
            visit_shape(shape, clip, visit);
        }
        return;
    }
    // A shape without vertices, such as an empty mesh, has no rectangle.
    let painted = shape.visual_bounding_rect();
    if [painted.min.x, painted.min.y, painted.max.x, painted.max.y]
        .iter()
        .all(|value| value.is_finite())
    {
        visit(shape, painted, clip);
    }
}

/// The rectangle that the panel reserved in the last frame, which is where its
/// background begins.
pub(super) fn reserved_panel_rect(app: &crate::app::HorizonApp) -> egui::Rect {
    app.settings
        .as_ref()
        .and_then(|editor| editor.panel_rect)
        .expect("the open panel reserved its rectangle")
}

/// The rectangle that the Settings panel paints: the tallest shape that covers
/// the whole width that the panel reserved, which is its background. The
/// rectangle of that shape is what the panel asked for, and the clip of a panel
/// that overflows is still the panel, so only the rectangle tells a panel that
/// fits from one that does not.
pub(super) fn panel_background_rect(shapes: &[egui::epaint::ClippedShape], reserved: egui::Rect) -> egui::Rect {
    let mut panel: Option<egui::Rect> = None;
    for_each_painted(shapes, &mut |_, painted, _| {
        if painted.min.x > reserved.min.x + 2.0 || painted.max.x < reserved.max.x - 2.0 {
            return;
        }
        if panel.is_none_or(|current| painted.height() > current.height()) {
            panel = Some(painted);
        }
    });
    panel.unwrap_or(egui::Rect::NOTHING)
}

/// The clip rectangle of the canvas: the widest clip of the frame that starts at
/// the window corner and stops before the window edge. The canvas is the only
/// surface with such a rectangle; the toolbar and the overlays cover the whole
/// window, and the sidebar and the panel start beside a corner.
pub(super) fn canvas_background_clip(shapes: &[egui::epaint::ClippedShape], viewport: [f32; 2]) -> egui::Rect {
    let mut widest: Option<egui::Rect> = None;
    for clipped in shapes {
        let clip = clipped.clip_rect;
        if clip.min.x.abs() > 0.5 || clip.min.y.abs() > 0.5 || clip.width() >= viewport[0] - 0.5 {
            continue;
        }
        if widest.is_none_or(|current| clip.width() > current.width()) {
            widest = Some(clip);
        }
    }
    widest.unwrap_or(egui::Rect::NOTHING)
}

/// The rectangles of one drawn text.
pub(super) fn text_rects(shapes: &[egui::epaint::ClippedShape], text: &str) -> Vec<egui::Rect> {
    let mut rects = Vec::new();
    for_each_painted(shapes, &mut |shape, painted, _| {
        if let egui::Shape::Text(drawn) = shape
            && drawn.galley.text() == text
        {
            rects.push(painted);
        }
    });
    rects
}

/// The texts that the panel paints, with the place where each one starts.
pub(super) fn panel_texts(shapes: &[egui::epaint::ClippedShape], panel: egui::Rect) -> Vec<(&str, egui::Pos2)> {
    let mut texts = Vec::new();
    for clipped in shapes {
        if !panel.expand(1.0).contains_rect(clipped.clip_rect) {
            continue;
        }
        collect_panel_texts(&clipped.shape, &mut texts);
    }
    texts
}

pub(super) fn collect_panel_texts<'a>(shape: &'a egui::Shape, texts: &mut Vec<(&'a str, egui::Pos2)>) {
    match shape {
        egui::Shape::Text(drawn) => texts.push((drawn.galley.text(), drawn.pos)),
        egui::Shape::Vec(shapes) => {
            for shape in shapes {
                collect_panel_texts(shape, texts);
            }
        }
        _ => {}
    }
}

/// Where the first character of one drawn text begins.
pub(super) fn text_start(shapes: &[egui::epaint::ClippedShape], text: &str) -> Option<egui::Pos2> {
    for clipped in shapes {
        if let Some(start) = text_start_in(&clipped.shape, text) {
            return Some(start);
        }
    }
    None
}

pub(super) fn text_start_in(shape: &egui::Shape, text: &str) -> Option<egui::Pos2> {
    match shape {
        egui::Shape::Text(drawn) if drawn.galley.text() == text => Some(drawn.pos),
        egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| text_start_in(shape, text)),
        _ => None,
    }
}

/// The labels of the controls of the Settings panel, so a test knows which text of
/// the panel is a word and which one is a control.
pub(super) const BUTTON_LABELS: &[&str] = &[
    "Add a computer",
    "Copy link",
    "Delete",
    "Join",
    "Keep",
    "Read the list",
    "Remove",
    "Replace value",
    "Store",
    "Cancel",
    "Save",
    "Revert",
    "Close",
    "Add",
    "Reset Defaults",
];
