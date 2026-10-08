use super::*;
use crate::app::test_support::{editor_workspace_state, raw_input, run_app_frame_with_input, test_app_with_startup};
use crate::test_egui::DiscardTextures;
use horizon_core::{RuntimeState, StartupDecision};

#[test]
fn application_capture_uses_the_root_window_and_includes_its_dialogs() {
    let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState {
            workspaces: vec![editor_workspace_state("root source", [0.0, 0.0])],
            ..RuntimeState::default()
        }),
    });
    let workspace = app.board.workspaces[0].id;
    for size in [[1600.0, 1000.0], [800.0, 1200.0]] {
        let _ = ctx
            .run_ui(raw_input(size, None), |ui| {
                let rect = app
                    .cast_source_rect(workspace, &CastSource::Application {}, ui.ctx())
                    .expect("root source");
                assert_eq!(
                    rect,
                    Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size[0], size[1]))
                );
                egui::Window::new("Synthetic dialog").show(ui.ctx(), |ui| {
                    ui.label("Part of this window");
                });
                assert!(!app.cast_obscured(workspace, &CastSource::Application {}, rect, ui.ctx()));
                app.casting.root_geometry = Some(RootGeometry::observe(
                    None,
                    rect,
                    ui.ctx().pixels_per_point(),
                    Instant::now()
                        .checked_sub(CAPTURE_INTERVAL)
                        .expect("capture interval fits"),
                ));
                assert!(app.cast_geometry_settled(workspace, &CastSource::Application {}, ui.ctx()));
                assert!(app.cast_obscured(workspace, &CastSource::Panel { id: "synthetic".into() }, rect, ui.ctx()));
            })
            .discard_textures();
    }
    let mut minimized = raw_input([800.0, 1200.0], None);
    minimized.viewports.entry(ViewportId::ROOT).or_default().minimized = Some(true);
    let _ = ctx
        .run_ui(minimized, |ui| {
            assert!(
                app.cast_source_rect(workspace, &CastSource::Application {}, ui.ctx())
                    .is_err()
            );
        })
        .discard_textures();
}

#[test]
fn sustained_root_resize_and_density_changes_keep_capture_frozen_until_settled() {
    let start = Instant::now();
    let mut geometry = None;
    // Five seconds of changing bounds must not expose the encoder's three-second idle timeout.
    for step in 0_u16..=100 {
        let now = start + Duration::from_millis(u64::from(step) * 50);
        let rect = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0 + f32::from(step), 1000.0));
        let current = RootGeometry::observe(geometry, rect, 1.0, now);
        assert!(!current.settled(rect, 1.0, now));
        geometry = Some(current);
    }
    let current = geometry.expect("observed root");
    assert!(current.settled(current.rect, 1.0, start + Duration::from_secs(6)));
    let density_changed = RootGeometry::observe(Some(current), current.rect, 2.0, start + Duration::from_secs(6));
    assert!(!density_changed.settled(current.rect, 2.0, start + Duration::from_secs(6)));
    assert!(!current.settled(current.rect, 2.0, start + Duration::from_secs(7)));
    assert!(density_changed.settled(current.rect, 2.0, start + Duration::from_secs(7)));
}

#[test]
fn slightly_late_ui_frames_preserve_the_capture_cadence() {
    let start = Instant::now();
    let mut previous = None;
    let mut captures = 0;
    for frame in 0..=100 {
        let now = start + Duration::from_millis(frame * 60);
        if previous.is_none_or(|previous| now.duration_since(previous) >= CAPTURE_INTERVAL) {
            previous = Some(capture_slot(previous, now));
            captures += 1;
        }
    }
    assert_eq!(captures, 90);
}

#[test]
fn a_long_capture_stall_reanchors_without_a_catch_up_burst() {
    let previous = Instant::now();
    let now = previous + Duration::from_millis(300);
    assert_eq!(capture_slot(Some(previous), now), now);
    assert_eq!(capture_slot(None, now), now);
}

#[test]
fn letterbox_preserves_aspect_ratio_and_opaque_borders() {
    let image = egui::ColorImage::filled([4, 2], egui::Color32::RED);
    let out = letterbox(&image, (4, 4));
    assert_eq!(&out[..16], &[0, 0, 0, 255].repeat(4));
    assert_eq!(&out[16..48], &[255, 0, 0, 255].repeat(8));
    assert_eq!(&out[48..], &[0, 0, 0, 255].repeat(4));
}
#[test]
fn letterbox_matches_nearest_pixels_for_upscaling_downscaling_and_portrait() {
    let image = egui::ColorImage::new(
        [3, 2],
        vec![
            egui::Color32::RED,
            egui::Color32::GREEN,
            egui::Color32::BLUE,
            egui::Color32::WHITE,
            egui::Color32::BLACK,
            egui::Color32::YELLOW,
        ],
    );
    for (width, height) in [(12, 8), (8, 12), (2, 2), (3, 2)] {
        let actual = letterbox(&image, (width, height));
        let (fit_width, fit_height) = if image.width() * height > image.height() * width {
            (width, image.height() * width / image.width())
        } else {
            (image.width() * height / image.height(), height)
        };
        let left = (width - fit_width) / 2;
        let top = (height - fit_height) / 2;
        for y in 0..height {
            for x in 0..width {
                let expected = if x >= left && x < left + fit_width && y >= top && y < top + fit_height {
                    image[(
                        (x - left) * image.width() / fit_width,
                        (y - top) * image.height() / fit_height,
                    )]
                        .to_array()
                } else {
                    [0, 0, 0, 255]
                };
                let at = (y * width + x) * 4;
                assert_eq!(&actual[at..at + 4], &expected);
            }
        }
    }
}

#[test]
fn layout_changes_wait_for_matching_rendered_geometry() {
    let state = RuntimeState {
        workspaces: vec![editor_workspace_state("source", [0.0, 0.0])],
        ..RuntimeState::default()
    };
    let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(state),
    });
    run_app_frame_with_input(&ctx, &mut app, raw_input([1600.0, 1000.0], None));
    let workspace = app.board.workspaces[0].id;
    let id = app.board.panels[0].id;
    let source = CastSource::Panel {
        id: "source-panel".into(),
    };
    assert!(app.cast_geometry_settled(workspace, &source, &ctx));
    for resize in [false, true] {
        if resize {
            app.board.panels[0].layout.size[0] += 20.0;
        } else {
            app.board.panels[0].layout.position[0] += 20.0;
        }
        assert!(app.cast_source_rect(workspace, &source, &ctx).is_ok());
        assert!(!app.cast_geometry_settled(workspace, &source, &ctx));
        let rendered = app.cast_visible_panel_rect(id, &ctx).expect("visible source");
        app.panel_screen_rects.insert(id, rendered);
        assert!(app.cast_geometry_settled(workspace, &source, &ctx));
    }
}

#[test]
fn cast_controls_shadow_alone_pauses_a_source() {
    let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    crate::theme::apply(&ctx, horizon_core::AppearanceTheme::Dark);
    for menu in [false, true] {
        let layer = if menu {
            LayerId::new(Order::Foreground, Id::new("cast_shadow_menu"))
        } else {
            cast_picker_layer()
        };
        if menu {
            app.casting.control_menus = Some([layer; 2]);
        }
        for _ in 0..3 {
            let _ = ctx
                .run_ui(raw_input([1600.0, 1000.0], None), |ui| {
                    egui::Area::new(layer.id)
                        .order(layer.order)
                        .fixed_pos(egui::pos2(400.0, 300.0))
                        .show(ui.ctx(), |ui| {
                            let frame = if menu {
                                egui::Frame::popup(ui.style())
                            } else {
                                egui::Frame::window(ui.style())
                            };
                            frame.show(ui, |ui| {
                                ui.allocate_space(egui::vec2(200.0, 100.0));
                            });
                        });
                })
                .discard_textures();
        }
        let area = ctx.memory(|memory| memory.area_rect(layer.id)).expect("control area");
        let strip = Rect::from_min_size(
            egui::pos2(area.center().x, area.bottom() + if menu { 15.0 } else { 20.0 }),
            egui::vec2(2.0, 2.0),
        );
        assert!(!area.intersects(strip));
        assert!(app.cast_controls_cover(strip, &ctx), "shadow pixels must stay private");
        let distant = strip.translate(egui::vec2(0.0, 40.0));
        assert!(!app.cast_controls_cover(distant, &ctx));
    }
}

#[test]
fn cast_controls_pause_only_covered_sources_and_other_overlays_remain_private() {
    let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    let source = CastSource::Panel { id: "synthetic".into() };
    let region = Rect::from_min_size(egui::pos2(400.0, 300.0), egui::vec2(200.0, 150.0));
    for other_overlay in [false, true] {
        let _ = ctx
            .run_ui(raw_input([1600.0, 1000.0], None), |ui| {
                egui::Area::new(cast_picker_layer().id)
                    .order(Order::Foreground)
                    .fixed_pos(region.min)
                    .show(ui.ctx(), |ui| {
                        ui.allocate_space(region.size());
                    });
                let child = egui::Area::new(Id::new("cast_menu"))
                    .order(Order::Foreground)
                    .fixed_pos(region.min)
                    .show(ui.ctx(), |ui| {
                        ui.allocate_space(region.size());
                    });
                ui.ctx().set_sublayer(cast_picker_layer(), child.response.layer_id);
                app.casting.control_menus = Some([
                    child.response.layer_id,
                    LayerId::new(Order::Foreground, Id::new("second_cast_menu")),
                ]);
                if other_overlay {
                    egui::Area::new(Id::new("unrelated_overlay"))
                        .order(Order::Foreground)
                        .fixed_pos(region.min)
                        .show(ui.ctx(), |ui| {
                            ui.allocate_space(region.size());
                        });
                }
                assert!(app.cast_controls_cover(region, ui.ctx()));
                assert!(!app.cast_controls_cover(region.translate(egui::vec2(700.0, 0.0)), ui.ctx()));
                assert_eq!(
                    app.cast_obscured(WorkspaceId(1), &source, region, ui.ctx()),
                    other_overlay
                );
            })
            .discard_textures();
        // egui has discarded sublayers but retained prior-frame areas.
        assert!(app.cast_controls_cover(region, &ctx));
        assert_eq!(app.cast_obscured(WorkspaceId(1), &source, region, &ctx), other_overlay);
    }
}

#[test]
fn source_authority_and_overlay_privacy_are_checked() {
    let state = RuntimeState {
        workspaces: vec![
            editor_workspace_state("first", [0.0, 0.0]),
            editor_workspace_state("second", [600.0, 0.0]),
        ],
        ..RuntimeState::default()
    };
    let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(state),
    });
    run_app_frame_with_input(&ctx, &mut app, raw_input([1600.0, 1000.0], None));
    let first = app.board.workspaces[0].id;
    let source = CastSource::Panel {
        id: "first-panel".into(),
    };
    assert!(
        app.cast_source_rect(
            first,
            &CastSource::Panel {
                id: "second-panel".into()
            },
            &ctx
        )
        .is_err()
    );
    let region = Rect::from_min_size(egui::pos2(400.0, 300.0), egui::vec2(200.0, 150.0));
    let _ = ctx.run_ui(raw_input([1600.0, 1000.0], None), |ui| {
        egui::Area::new(Id::new("synthetic_overlay"))
            .order(Order::Foreground)
            .fixed_pos(region.min)
            .show(ui.ctx(), |ui| {
                ui.allocate_space(region.size());
            });
    });
    assert!(app.cast_obscured(first, &source, region, &ctx));
    app.board.panels[0].layout.size[0] = 10_000.0;
    let id = app.board.panels[0].id;
    app.panel_screen_rects.insert(id, region);
    assert!(
        app.cast_source_rect(first, &source, &ctx)
            .expect_err("clipped source")
            .contains("Fit the entire source")
    );
    assert!(!app.cast_obscured(
        first,
        &source,
        Rect::from_min_size(egui::pos2(1200.0, 700.0), egui::vec2(20.0, 20.0)),
        &ctx
    ));
}

/// Clouds in a workspace cast.
#[cfg(feature = "cloud-workspaces")]
mod clouds {
    use crate::app::test_support::{
        editor_workspace_state, raw_input, run_app_frame_with_input, test_app_with_startup,
    };
    use crate::app::view::canvas_scene_transform;
    use horizon_core::browser::manifest::cast::CastSource;
    use horizon_core::cloud_panel::{CloudGroup, CloudLaunch};
    use horizon_core::{CanvasViewState, RuntimeState, StartupDecision};

    fn launch() -> CloudLaunch {
        let config = horizon_core::cloud_panel::CloudConfig::parse(
            "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n",
        )
        .unwrap();
        CloudLaunch {
            deployment_started: true,
            id: "cast-fixture".into(),
            revision: "a".repeat(40),
            profile_name: "dev".into(),
            profile: config.profiles["dev"].clone(),
            placement: horizon_core::cloud_panel::Placement::default(),
        }
    }

    /// A workspace with a cloud at `cloud_at`, with or without its editor panel, rendered
    /// at half zoom so everything is in view.
    fn desk(with_panel: bool) -> (tempfile::TempDir, egui::Context, crate::app::HorizonApp) {
        desk_with(with_panel, |_, _| {})
    }

    /// Like [`desk`], with `setup` run on the board and the cloud before it is drawn.
    fn desk_with(
        with_panel: bool,
        setup: impl FnOnce(&mut horizon_core::Board, &mut CloudGroup),
    ) -> (tempfile::TempDir, egui::Context, crate::app::HorizonApp) {
        let mut workspace = editor_workspace_state("cast desk", [0.0, 0.0]);
        if !with_panel {
            workspace.panels.clear();
        }
        let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState {
                workspaces: vec![workspace],
                ..RuntimeState::default()
            }),
        });
        let workspace = app.board.workspaces[0].id;
        app.board.retain_workspace_when_empty(workspace);
        let local = app.board.workspace(workspace).unwrap().local_id.clone();
        let mut group = CloudGroup::new(101, "Cast cloud".into(), local, "/synthetic".into(), [900.0, 80.0]);
        group.remote = Some(launch());
        setup(&mut app.board, &mut group);
        app.board.cloud_groups.0.push(group);
        app.prepare_cloud_prototype(&ctx);
        app.canvas_view = CanvasViewState::new([0.0, 0.0], 0.5);
        for _ in 0..4 {
            run_app_frame_with_input(&ctx, &mut app, raw_input([1600.0, 1000.0], Some([1.0, 1.0])));
        }
        (temp, ctx, app)
    }

    fn cloud_screen_rect(app: &crate::app::HorizonApp, ctx: &egui::Context) -> egui::Rect {
        let group = &app.cloud_prototype.groups.0[0];
        let (min, max) = group.overview_bounds();
        canvas_scene_transform(app.canvas_rect(ctx), app.canvas_view)
            * egui::Rect::from_min_max(egui::Pos2::from(min), egui::Pos2::from(max))
    }

    #[test]
    fn a_workspace_of_only_a_cloud_casts_the_cloud() {
        let (_temp, ctx, app) = desk(false);
        let workspace = app.board.workspaces[0].id;
        let source = CastSource::Workspace {
            id: app.board.workspaces[0].local_id.clone(),
        };
        let rect = app
            .cast_source_rect(workspace, &source, &ctx)
            .expect("the cloud is the content");
        let cloud = cloud_screen_rect(&app, &ctx);
        assert!(rect.expand(1.5).contains_rect(cloud), "{rect:?} holds {cloud:?}");
        assert!(
            !app.cast_obscured(workspace, &source, rect, &ctx),
            "the cloud's own header does not cover it"
        );
        assert!(app.cast_geometry_settled(workspace, &source, &ctx));
    }

    #[test]
    fn a_workspace_cast_holds_its_panels_and_its_clouds() {
        let (_temp, ctx, app) = desk(true);
        let workspace = app.board.workspaces[0].id;
        let source = CastSource::Workspace {
            id: app.board.workspaces[0].local_id.clone(),
        };
        let rect = app
            .cast_source_rect(workspace, &source, &ctx)
            .expect("panels and cloud");
        let cloud = cloud_screen_rect(&app, &ctx);
        let panel = app.panel_screen_rects[&app.board.panels[0].id];
        assert!(
            rect.expand(1.5).contains_rect(cloud),
            "{rect:?} holds the cloud {cloud:?}"
        );
        assert!(
            rect.expand(1.5).contains_rect(panel),
            "{rect:?} holds the panel {panel:?}"
        );
        assert!(!app.cast_obscured(workspace, &source, rect, &ctx));
        // A single panel's cast still treats the cloud's header as something covering it
        // when they overlap; here they do not, so the panel cast is clear too.
        let panel_source = CastSource::Panel {
            id: app.board.panels[0].local_id.clone(),
        };
        let panel_rect = app
            .cast_source_rect(workspace, &panel_source, &ctx)
            .expect("panel source");
        assert!(
            !app.cast_obscured(workspace, &panel_source, panel_rect, &ctx),
            "the panel's own resize grip is part of it"
        );
    }

    #[test]
    fn an_open_drawer_is_part_of_the_cast() {
        let (_temp, ctx, mut app) = desk(false);
        app.open_cloud_drawer(101);
        for _ in 0..3 {
            run_app_frame_with_input(&ctx, &mut app, raw_input([1600.0, 1000.0], Some([1.0, 1.0])));
        }
        let workspace = app.board.workspaces[0].id;
        let source = CastSource::Workspace {
            id: app.board.workspaces[0].local_id.clone(),
        };
        let group = app.cloud_prototype.groups.0[0].clone();
        let drawer = app.cloud_drawer_rect(&group).expect("the drawer is open");
        let drawer = canvas_scene_transform(app.canvas_rect(&ctx), app.canvas_view) * drawer;
        let rect = app
            .cast_source_rect(workspace, &source, &ctx)
            .expect("cloud with drawer");
        assert!(
            rect.expand(1.5).contains_rect(drawer),
            "{rect:?} holds the drawer {drawer:?}"
        );
        assert!(
            !app.cast_obscured(workspace, &source, rect, &ctx),
            "the drawer does not cover its own cloud"
        );
    }

    fn cloud_source() -> CastSource {
        CastSource::Cloud {
            id: "cast-fixture".into(),
        }
    }

    #[test]
    fn a_cloud_cast_holds_the_cloud_and_not_the_panels_beside_it() {
        let (_temp, ctx, app) = desk(true);
        let workspace = app.board.workspaces[0].id;
        let rect = app
            .cast_source_rect(workspace, &cloud_source(), &ctx)
            .expect("the cloud alone");
        let cloud = cloud_screen_rect(&app, &ctx);
        let panel = app.panel_screen_rects[&app.board.panels[0].id];
        assert!(rect.expand(1.5).contains_rect(cloud), "{rect:?} holds {cloud:?}");
        assert!(!rect.intersects(panel), "{rect:?} leaves out the panel {panel:?}");
        assert!(!app.cast_obscured(workspace, &cloud_source(), rect, &ctx));
        assert!(app.cast_geometry_settled(workspace, &cloud_source(), &ctx));
    }

    #[test]
    fn a_cloud_cast_holds_its_panels() {
        let (_temp, ctx, app) = desk_with(true, |board, group| {
            let panel = board.panels[0].id;
            group.attach(board, panel);
        });
        let group = &app.cloud_prototype.groups.0[0];
        assert_eq!(group.panels, vec![app.board.panels[0].local_id.clone()]);
        let workspace = app.board.workspaces[0].id;
        let rect = app
            .cast_source_rect(workspace, &cloud_source(), &ctx)
            .expect("cloud and member");
        let panel = app.panel_screen_rects[&app.board.panels[0].id];
        assert!(
            rect.expand(1.5).contains_rect(panel),
            "{rect:?} holds the member {panel:?}"
        );
        assert!(
            !app.cast_obscured(workspace, &cloud_source(), rect, &ctx),
            "the member and its owner badge are part of the cloud"
        );
        assert!(app.cast_geometry_settled(workspace, &cloud_source(), &ctx));
    }

    #[test]
    fn a_cloud_cast_refuses_an_unknown_cloud_and_an_overlapping_one() {
        let (_temp, ctx, app) = desk(false);
        let workspace = app.board.workspaces[0].id;
        let unknown = CastSource::Cloud { id: "elsewhere".into() };
        assert_eq!(
            app.cast_source_rect(workspace, &unknown, &ctx),
            Err("Source is outside the current workspace".into())
        );
        // Clouds make room for each other, so only a moment such as a drag puts one over
        // another; place the UI copy there without drawing a frame in between.
        let (_temp, ctx, mut app) = desk_with(false, |board, group| {
            let mut other = CloudGroup::new(
                102,
                "Other".into(),
                group.workspace.clone(),
                "/other".into(),
                [0.0, 80.0],
            );
            let mut launch = launch();
            launch.id = "other-cloud".into();
            other.remote = Some(launch);
            board.cloud_groups.0.push(other);
        });
        let workspace = app.board.workspaces[0].id;
        assert!(app.cast_source_rect(workspace, &cloud_source(), &ctx).is_ok());
        let cast = app
            .cloud_prototype
            .groups
            .0
            .iter()
            .find(|group| group.issue == 101)
            .unwrap()
            .position;
        let other = app
            .cloud_prototype
            .groups
            .0
            .iter_mut()
            .find(|group| group.issue == 102)
            .unwrap();
        other.position = [cast[0] + 40.0, cast[1] + 40.0];
        assert_eq!(
            app.cast_source_rect(workspace, &cloud_source(), &ctx),
            Err("Another cloud overlaps this source; move it before casting".into())
        );
    }

    #[test]
    fn the_cast_picker_lists_and_opens_a_cloud() {
        let (_temp, ctx, mut app) = desk(true);
        let workspace = app.board.workspaces[0].id;
        let snapshot = app.cast_snapshot(workspace, &ctx);
        let cloud = snapshot
            .sources
            .iter()
            .find(|source| source.source == cloud_source())
            .expect("the cloud is a source");
        assert_eq!(cloud.name, "Cast cloud");
        assert!(cloud.available);
        assert!(!cloud.requires_user_approval);
        app.open_cloud_cast_picker(101, &ctx);
        let picker = app.casting.picker.as_ref().expect("Cast in Manage opens the picker");
        assert_eq!(picker.source, cloud_source());
        assert_eq!(picker.workspace, workspace);
        assert_eq!(picker.anchor, None);
    }

    #[test]
    fn a_hidden_panel_does_not_block_a_workspace_cast() {
        let (_temp, ctx, mut app) = desk(true);
        app.board.panels[0].visible = false;
        for _ in 0..3 {
            run_app_frame_with_input(&ctx, &mut app, raw_input([1600.0, 1000.0], Some([1.0, 1.0])));
        }
        let workspace = app.board.workspaces[0].id;
        let source = CastSource::Workspace {
            id: app.board.workspaces[0].local_id.clone(),
        };
        let rect = app
            .cast_source_rect(workspace, &source, &ctx)
            .expect("the hidden panel is not part of the picture");
        assert!(rect.expand(1.5).contains_rect(cloud_screen_rect(&app, &ctx)));
        assert!(app.cast_geometry_settled(workspace, &source, &ctx));
    }

    #[test]
    fn a_duplicated_cloud_id_is_not_a_source() {
        // Persisted records can carry one cloud ID twice, here in another workspace.
        let (_temp, ctx, app) = desk_with(false, |board, group| {
            let mut copy = group.clone();
            copy.issue = 102;
            copy.workspace = "elsewhere".into();
            board.cloud_groups.0.push(copy);
        });
        let workspace = app.board.workspaces[0].id;
        assert_eq!(
            app.cast_source_rect(workspace, &cloud_source(), &ctx),
            Err("This cloud's identity is duplicated; it cannot be cast".into())
        );
        let listed = app
            .cast_snapshot(workspace, &ctx)
            .sources
            .into_iter()
            .find(|source| source.source == cloud_source())
            .expect("the cloud in this workspace is listed");
        assert!(!listed.available);
    }
}
