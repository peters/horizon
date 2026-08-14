use super::*;

#[test]
fn tags_layout_job_uses_single_line_ellipsis_and_preserves_tag_colors() {
    let tags = vec!["tag:cuda\r\nprod".to_string(), "tag:node\u{2028}gpu".to_string()];
    let font = FontId::monospace(11.0);
    let ctx = egui::Context::default();
    ctx.set_fonts(crate::app::configure_fonts());
    let mut job = None;
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        job = Some(tags_layout_job(ctx, &tags, &font, 120.0));
    });
    let Some(job) = job else {
        panic!("tags job was not built");
    };

    assert_eq!(job.text, "tag:cuda prod,tag:node gpu");
    assert_eq!(job.sections.len(), 3);
    assert_eq!(job.sections[0].format.color, tag_color(&tags[0]));
    assert_eq!(job.sections[1].format.color, theme::FG_DIM());
    assert_eq!(job.sections[2].format.color, tag_color(&tags[1]));
    assert!(!job.break_on_newline);
    assert!((job.wrap.max_width - 120.0).abs() < f32::EPSILON);
    assert_eq!(job.wrap.max_rows, 1);
    assert!(job.wrap.break_anywhere);
    assert_eq!(job.wrap.overflow_character, Some('\u{2026}'));
}

#[test]
fn host_alias_layout_flattens_newlines_and_honors_its_column_width() {
    let font = FontId::monospace(12.0);
    let ctx = egui::Context::default();
    ctx.set_fonts(crate::app::configure_fonts());
    let mut job = None;
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        job = Some(truncated_text_layout_job(
            ctx,
            "production\r\nwith a deliberately long alias",
            &font,
            Color32::WHITE,
            90.0,
        ));
    });
    let Some(job) = job else {
        panic!("host alias job was not built");
    };

    assert_eq!(job.text, "production with a deliberately long alias");
    assert_eq!(job.wrap.max_rows, 1);
    assert_eq!(job.wrap.overflow_character, Some('…'));
    assert!((job.wrap.max_width - 90.0).abs() < f32::EPSILON);
}

#[test]
fn pathological_tag_lists_share_one_shaping_budget() {
    let tags = vec!["tag".to_string(); 2_000];
    let font = FontId::monospace(11.0);
    let ctx = egui::Context::default();
    ctx.set_fonts(crate::app::configure_fonts());
    let mut job = None;
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        job = Some(tags_layout_job(ctx, &tags, &font, f32::INFINITY));
    });
    let Some(job) = job else {
        panic!("tags job was not built");
    };

    assert!(job.sections.len() <= 512);
    assert!(job.text.chars().count() <= 513);
}
