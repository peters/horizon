//! The header strip at several widths: what it drops first and what it keeps clear.
use super::*;
use crate::test_egui::DiscardTextures;

const LINE: &str = "$1.02 run · $8.86 total · $7.68/day";
const OUTLOOK: &str = "$53.76/wk · $233.60/mo · +$130.12 by Oct 31 if left running";
/// The untruncated width of the title and subtitle beside the strip.
const TITLE_WIDTH: f32 = 260.0;

/// Every text and control the header strip draws for `status`, at `width`.
fn strip_frame(width: f32, status: &status::Status) -> (strip::Strip, Vec<(String, egui::Rect)>, egui::Rect) {
    let ctx = egui::Context::default();
    let mut result = None;
    let header = egui::Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(width, 118.0));
    let output = ctx
        .run_ui(egui::RawInput::default(), |ui| {
            let indicators = strip::Indicators {
                running: 2,
                terminals: 3,
                desktop: Some(true),
                sharing: strip::Sharing::Open(3),
                companions: 2,
            };
            let spend = strip::Spend {
                line: LINE.into(),
                outlook: Some(OUTLOOK.into()),
                explanation: String::new(),
            };
            result = Some(strip::show(
                ui,
                strip::Header {
                    rect: header,
                    title_width: TITLE_WIDTH,
                },
                status,
                &indicators,
                &spend,
                false,
                None,
            ));
        })
        .discard_textures();
    let texts = output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) => Some((
                text.galley.text().to_owned(),
                text.visual_bounding_rect().intersect(clipped.clip_rect),
            )),
            _ => None,
        })
        .collect();
    (result.unwrap(), texts, header)
}

#[test]
fn the_header_strip_fits_the_narrowest_cloud_with_long_errors() {
    let long = "Detailed retryable failure ".repeat(40);
    let runtime = super::super::super::Runtime {
        stage: Some(Stage::Push),
        error: Some(long.clone()),
        ..Default::default()
    };
    let failed = status::of(&runtime, status::Occupancy::default(), std::time::SystemTime::now());
    let ready = status::of(
        &super::super::super::Runtime::default(),
        status::Occupancy {
            panels: 3,
            running: 2,
            terminals: 3,
        },
        std::time::SystemTime::now(),
    );
    for status in [&failed, &ready] {
        for width in [548.0, 1100.0, 1760.0] {
            let (strip, texts, header) = strip_frame(width, status);
            let close_left = header.right() - 42.0;
            assert!(
                strip.reserved <= width - 64.0 - 170.0 + 0.5,
                "the title keeps its room at {width}"
            );
            for (text, rect) in &texts {
                if rect.is_negative() || rect.width() <= 0.0 {
                    continue;
                }
                assert!(
                    header.contains_rect(rect.shrink(0.5)),
                    "{text:?} stays in the header at {width}: {rect:?}"
                );
                if rect.top() < 60.0 {
                    assert!(
                        rect.right() <= close_left + 0.5,
                        "{text:?} clears the close button at {width}"
                    );
                }
            }
            let has = |label: &str| texts.iter().any(|(text, _)| text == label);
            assert!(has(status.primary.unwrap().label()), "the main action survives {width}");
            assert_eq!(
                has(LINE),
                width >= 1100.0,
                "spend is the first thing a narrow header drops ({width})"
            );
            if has(LINE) {
                assert!(
                    has("2/3"),
                    "spend never shows without the connection indicators ({width})"
                );
            }
            let rect = |label: &str| texts.iter().find(|(text, _)| text == label).map(|(_, rect)| *rect);
            if let Some(outlook) = rect(OUTLOOK) {
                let line = rect(LINE).expect("the outlook shows only under the spend line");
                assert!(
                    outlook.top() >= line.bottom(),
                    "the outlook sits under the line at {width}"
                );
                assert!(
                    outlook.left() >= 64.0 + TITLE_WIDTH,
                    "the outlook never shortens the title at {width}"
                );
            }
            assert_eq!(
                has(OUTLOOK),
                width >= 1760.0,
                "the outlook drops before the line ({width})"
            );
        }
    }
}
