use egui::{Color32, Mesh, Pos2, Rect, Shape, Vec2};

use super::{
    BackdropCache, BackdropColours, GLOW_EDGE_SHARE, GLOW_RINGS, GLOW_SEGMENTS, HORIZON_SHARE, SKY_STRIPS,
    glow_colour_at, horizon_line_alpha_share, horizon_y, mix, sky_colour_at, with_alpha_share,
};
use crate::theme::{self, ResolvedTheme};

/// The colours of `theme`, read as the pass of that theme reads them.
fn colours(theme: ResolvedTheme) -> BackdropColours {
    let _held = crate::test_egui::with_theme(theme);
    BackdropColours::active()
}

fn mesh_of(shape: &Shape) -> &Mesh {
    match shape {
        Shape::Mesh(mesh) => mesh,
        other => panic!("the backdrop is three meshes, and this is {other:?}"),
    }
}

/// The canvas of the root window at the default window size: 1200 by 800 at
/// 210, 46, beside the sidebar.
fn canvas() -> Rect {
    Rect::from_min_size(Pos2::new(210.0, 46.0), Vec2::new(1200.0, 800.0))
}

fn shapes_of(rect: Rect, theme: ResolvedTheme) -> Vec<Shape> {
    let _held = crate::test_egui::with_theme(theme);
    super::build(rect, &BackdropColours::active())
}

fn alpha_of(colour: Color32) -> u8 {
    colour.to_srgba_unmultiplied()[3]
}

#[test]
fn the_sky_holds_its_stops_of_the_design() {
    for theme in [ResolvedTheme::Dark, ResolvedTheme::Light] {
        let colours = colours(theme);
        assert_eq!(sky_colour_at(0.0, &colours), colours.sky_top);
        assert_eq!(sky_colour_at(0.46, &colours), colours.sky_mid);
        assert_eq!(sky_colour_at(HORIZON_SHARE, &colours), colours.sky_horizon);
        assert_eq!(sky_colour_at(1.0, &colours), colours.ground);
        // A share beyond the canvas keeps the colour of the stop at its edge.
        assert_eq!(sky_colour_at(-0.5, &colours), colours.sky_top);
        assert_eq!(sky_colour_at(1.5, &colours), colours.ground);

        // A share between two stops mixes those two colours on every channel.
        let between = sky_colour_at(0.23, &colours).to_srgba_unmultiplied();
        for (channel, value) in between.iter().enumerate() {
            let top = f32::from(colours.sky_top.to_srgba_unmultiplied()[channel]);
            let mid = f32::from(colours.sky_mid.to_srgba_unmultiplied()[channel]);
            let low = top.min(mid);
            let high = top.max(mid);
            assert!(
                (low..=high).contains(&f32::from(*value)),
                "the channel {channel} of the sky at 0.23 is between its two stops in {theme:?}"
            );
        }

        // The horizon is brighter than the ground under it in both themes.
        let brightness = |colour: Color32| {
            let [red, green, blue, _] = colour.to_srgba_unmultiplied();
            u16::from(red) + u16::from(green) + u16::from(blue)
        };
        assert!(
            brightness(colours.sky_horizon) > brightness(colours.ground),
            "the sky at the horizon is brighter than the ground under it in {theme:?}"
        );
    }
}

#[test]
fn the_horizon_stands_at_72_percent_of_the_canvas() {
    let rect = canvas();
    assert!((horizon_y(rect) - (46.0 + 0.72 * 800.0)).abs() <= f32::EPSILON);
    assert!((horizon_y(rect) - rect.top() - rect.height() * HORIZON_SHARE).abs() <= f32::EPSILON);

    // A taller window and a wider one move the horizon with them: the horizon is
    // a share of the canvas and not a place on the screen.
    for rect in [
        Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 480.0)),
        Rect::from_min_size(Pos2::new(210.0, 46.0), Vec2::new(2400.0, 1600.0)),
    ] {
        let share = (horizon_y(rect) - rect.top()) / rect.height();
        assert!((share - HORIZON_SHARE).abs() <= f32::EPSILON, "the horizon of {rect:?}");
    }
}

#[test]
fn the_glow_has_a_warm_centre_a_cool_middle_and_no_edge() {
    for theme in [ResolvedTheme::Dark, ResolvedTheme::Light] {
        let colours = colours(theme);
        assert_eq!(glow_colour_at(0.0, &colours), colours.glow_warm);
        assert_eq!(glow_colour_at(0.46, &colours), colours.glow_cool);

        let alpha = |distance: f32| alpha_of(glow_colour_at(distance, &colours));
        assert_eq!(alpha(0.46), alpha_of(colours.glow_cool));
        assert_eq!(alpha(GLOW_EDGE_SHARE), 0);
        assert_eq!(alpha(1.0), 0);
        assert_eq!(alpha(4.0), 0);

        // The alpha falls from the centre outward with no step: no hundredth of
        // the radius steps more than one level of the channel.
        let mut before = alpha(0.0);
        let mut distance = 0.0;
        while distance < 1.0 {
            distance += 0.01;
            let now = alpha(distance);
            assert!(now <= before, "the glow fades outward at {distance:.2}");
            assert!(
                before - now <= 1,
                "the glow steps {} levels at {distance:.2}",
                before - now
            );
            before = now;
        }
    }
}

#[test]
fn the_line_fades_to_both_edges_and_holds_its_peak_between_them() {
    let close = |share: f32, expected: f32| {
        assert!(
            (horizon_line_alpha_share(share) - expected).abs() <= 1e-5,
            "the line at {share} holds {}, and the design gives {expected}",
            horizon_line_alpha_share(share)
        );
    };
    close(0.0, 0.0);
    close(0.10, 0.5);
    close(0.20, 1.0);
    close(0.50, 1.0);
    close(0.80, 1.0);
    close(0.90, 0.5);
    close(1.0, 0.0);

    // A share outside the canvas never runs backwards.
    close(-0.5, 0.0);
    close(1.5, 0.0);
}

#[test]
fn the_sky_mesh_starts_at_the_top_colour_and_ends_at_the_ground() {
    let rect = canvas();
    let theme = ResolvedTheme::Dark;
    let colours = colours(theme);
    let shapes = shapes_of(rect, theme);
    let sky = mesh_of(&shapes[0]);

    assert_eq!(sky.vertices.len(), 2 * (usize::from(SKY_STRIPS) + 1));
    assert_eq!(sky.indices.len(), 2 * usize::from(SKY_STRIPS) * 3);
    assert_eq!(sky.vertices[0].pos, Pos2::new(rect.left(), rect.top()));
    assert_eq!(sky.vertices[0].color, colours.sky_top);
    assert_eq!(sky.vertices[1].pos, Pos2::new(rect.right(), rect.top()));
    assert_eq!(sky.vertices[1].color, colours.sky_top);

    let foot = sky.vertices.len() - 2;
    assert_eq!(sky.vertices[foot].pos, Pos2::new(rect.left(), rect.bottom()));
    assert_eq!(sky.vertices[foot].color, colours.ground);
    assert_eq!(sky.vertices[foot + 1].color, colours.ground);
}

#[test]
fn the_glow_mesh_holds_its_rings_and_its_centre() {
    let rect = canvas();
    let theme = ResolvedTheme::Light;
    let colours = colours(theme);
    let shapes = shapes_of(rect, theme);
    let glow = mesh_of(&shapes[1]);

    assert_eq!(
        glow.vertices.len(),
        (usize::from(GLOW_RINGS) + 1) * (usize::from(GLOW_SEGMENTS) + 1)
    );
    assert_eq!(
        glow.indices.len(),
        2 * usize::from(GLOW_RINGS) * usize::from(GLOW_SEGMENTS) * 3
    );
    // The first ring is the centre of the ellipse: the middle of the width, on
    // the horizon line.
    assert_eq!(glow.vertices[0].pos, Pos2::new(rect.center().x, horizon_y(rect)));
    assert_eq!(glow.vertices[0].color, colours.glow_warm);

    // The last ring is the edge of the ellipse, and it holds no alpha: the glow
    // has no edge to show.
    let stride = usize::from(GLOW_SEGMENTS) + 1;
    let edge = &glow.vertices[usize::from(GLOW_RINGS) * stride..];
    for vertex in edge {
        assert_eq!(alpha_of(vertex.color), 0);
    }
    let widest = edge
        .iter()
        .map(|vertex| (vertex.pos.x - rect.center().x).abs())
        .fold(0.0_f32, f32::max);
    let tallest = edge
        .iter()
        .map(|vertex| (vertex.pos.y - horizon_y(rect)).abs())
        .fold(0.0_f32, f32::max);
    assert!((widest - rect.width() * 1.3).abs() <= 0.5);
    assert!((tallest - rect.height() * 0.26).abs() <= 0.5);
}

#[test]
fn the_line_mesh_is_one_point_thick_and_brightest_between_its_shoulders() {
    let rect = canvas();
    let theme = ResolvedTheme::Dark;
    let colours = colours(theme);
    let shapes = shapes_of(rect, theme);
    let line = mesh_of(&shapes[2]);

    assert_eq!(line.vertices.len(), 8);
    let peak = alpha_of(colours.horizon_line);
    let alphas: Vec<u8> = (0..4).map(|column| alpha_of(line.vertices[2 * column].color)).collect();
    assert_eq!(alphas, vec![0, peak, peak, 0]);
    for pair in line.vertices.chunks(2) {
        assert!((pair[1].pos.y - pair[0].pos.y - 1.0).abs() <= f32::EPSILON);
        assert!((pair[0].pos.y + 0.5 - horizon_y(rect)).abs() <= f32::EPSILON);
    }
    assert_eq!(line.vertices[0].pos.x, rect.left());
    assert_eq!(line.vertices[6].pos.x, rect.right());
}

/// The sky and the line paint nothing outside their rectangle. The glow reaches
/// past it, because its radii are shares of the canvas that no canvas fills, and
/// the painter clips it to the canvas.
#[test]
fn only_the_glow_of_the_backdrop_reaches_past_the_canvas() {
    let rect = canvas();
    let shapes = shapes_of(rect, ResolvedTheme::Dark);
    let outside = |shape: &Shape| {
        mesh_of(shape)
            .vertices
            .iter()
            .filter(|vertex| !rect.contains(vertex.pos))
            .count()
    };

    assert_eq!(outside(&shapes[0]), 0);
    assert_eq!(outside(&shapes[2]), 0);
    assert!(outside(&shapes[1]) > 0, "the glow is wider than the canvas");
}

#[test]
fn an_unchanged_canvas_builds_no_shapes_again() {
    let theme = ResolvedTheme::Dark;
    let _held = crate::test_egui::with_theme(theme);
    let mut cache = BackdropCache::default();
    let rect = canvas();

    cache.shapes(rect, 1.0, theme);
    assert_eq!(cache.builds, 1);
    // The pan and the zoom of the canvas move the dot grid and not the backdrop,
    // so a frame that changes nothing keeps the shapes of the frame before.
    cache.shapes(rect, 1.0, theme);
    assert_eq!(cache.builds, 1);

    let moved = rect.translate(Vec2::new(24.0, -12.0));
    cache.shapes(moved, 1.0, theme);
    assert_eq!(cache.builds, 2);
    cache.shapes(moved, 2.0, theme);
    assert_eq!(cache.builds, 3);
    // A theme builds its own colours, so a cache never hands the shapes of one
    // theme to the other.
    cache.shapes(moved, 2.0, ResolvedTheme::Light);
    assert_eq!(cache.builds, 4);
    cache.shapes(rect, 1.0, theme);
    assert_eq!(cache.builds, 5);

    // A theme change drops the shapes outright, so no stale colour survives it.
    cache.forget();
    cache.shapes(rect, 1.0, theme);
    assert_eq!(cache.builds, 6);
}

#[test]
fn the_backdrop_holds_a_known_number_of_vertices() {
    let shapes = shapes_of(canvas(), ResolvedTheme::Dark);
    let vertices: usize = shapes.iter().map(|shape| mesh_of(shape).vertices.len()).sum();
    let triangles: usize = shapes.iter().map(|shape| mesh_of(shape).indices.len() / 3).sum();

    // 194 for the sky, 2425 for the glow and 8 for the line.
    assert_eq!(vertices, 2627);
    assert_eq!(triangles, 4806);
}

#[test]
fn a_mix_keeps_the_channels_and_an_alpha_share_stays_in_range() {
    let red = Color32::from_rgba_unmultiplied(255, 0, 0, 255);
    let blue = Color32::from_rgba_unmultiplied(0, 0, 255, 255);
    assert_eq!(mix(red, blue, 0.0), red);
    assert_eq!(mix(red, blue, 1.0), blue);
    assert_eq!(mix(red, blue, 2.0), blue);
    // The mix runs on the premultiplied values, as a mesh interpolates them, so
    // the half of red and blue is 188 and not the 128 of a mix of sRGB bytes.
    let [mixed_red, mixed_green, mixed_blue, mixed_alpha] = mix(red, blue, 0.5).to_srgba_unmultiplied();
    assert!((180..=195).contains(&mixed_red), "the red of a half mix is {mixed_red}");
    assert!(
        (180..=195).contains(&mixed_blue),
        "the blue of a half mix is {mixed_blue}"
    );
    assert_eq!((mixed_green, mixed_alpha), (0, 255));

    let half = with_alpha_share(red, 0.5);
    assert_eq!(half.to_srgba_unmultiplied(), [255, 0, 0, 128]);
    assert_eq!(with_alpha_share(red, 0.0).to_srgba_unmultiplied()[3], 0);
    assert_eq!(with_alpha_share(red, 3.0), red);
}

#[test]
fn the_tokens_of_the_two_themes_hold_the_numbers_of_the_design() {
    {
        let _held = crate::test_egui::with_theme(ResolvedTheme::Dark);
        assert_eq!(theme::CANVAS_SKY_TOP(), Color32::from_rgb(4, 6, 11));
        assert_eq!(theme::CANVAS_SKY_MID(), Color32::from_rgb(8, 13, 27));
        assert_eq!(theme::CANVAS_SKY_HORIZON(), Color32::from_rgb(19, 26, 54));
        assert_eq!(theme::CANVAS_GROUND(), Color32::from_rgb(5, 7, 12));
        assert_eq!(
            theme::CANVAS_GLOW_WARM(),
            Color32::from_rgba_unmultiplied(255, 150, 92, 51)
        );
        assert_eq!(
            theme::CANVAS_GLOW_COOL(),
            Color32::from_rgba_unmultiplied(150, 110, 235, 26)
        );
        assert_eq!(
            theme::CANVAS_HORIZON_LINE(),
            Color32::from_rgba_unmultiplied(255, 196, 150, 61)
        );
        assert_eq!(theme::GRID_DOT(), Color32::from_rgba_unmultiplied(190, 200, 230, 41));
    }

    {
        let _held = crate::test_egui::with_theme(ResolvedTheme::Light);
        assert_eq!(theme::CANVAS_SKY_TOP(), Color32::from_rgb(247, 245, 240));
        assert_eq!(theme::CANVAS_SKY_MID(), Color32::from_rgb(243, 239, 232));
        assert_eq!(theme::CANVAS_SKY_HORIZON(), Color32::from_rgb(243, 230, 216));
        assert_eq!(theme::CANVAS_GROUND(), Color32::from_rgb(230, 221, 207));
        assert_eq!(
            theme::CANVAS_GLOW_WARM(),
            Color32::from_rgba_unmultiplied(240, 150, 86, 61)
        );
        assert_eq!(
            theme::CANVAS_GLOW_COOL(),
            Color32::from_rgba_unmultiplied(120, 130, 225, 26)
        );
        assert_eq!(
            theme::CANVAS_HORIZON_LINE(),
            Color32::from_rgba_unmultiplied(214, 120, 60, 71)
        );
        assert_eq!(theme::GRID_DOT(), Color32::from_rgba_unmultiplied(120, 105, 80, 56));
    }
}
