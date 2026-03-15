#![forbid(unsafe_code)]

mod app;
mod branding;
mod input;
mod terminal_widget;
mod theme;

use std::path::PathBuf;

use app::HorizonApp;
use horizon_core::Config;

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("horizon=info,horizon_core=info")),
        )
        .init();

    // Parse --config <path> from CLI args
    let config_path = parse_config_arg();
    let config = Config::load(config_path.as_deref()).unwrap_or_else(|e| {
        tracing::error!("failed to load config: {e}");
        Config::default()
    });

    let window = &config.window;
    // Clamp to reasonable bounds so we don't open larger than the screen.
    let width = window.width.clamp(800.0, 7680.0);
    let height = window.height.clamp(600.0, 4320.0);
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(branding::APP_NAME)
        .with_icon(branding::app_icon())
        .with_decorations(true)
        .with_transparent(false)
        .with_inner_size([width, height])
        .with_min_inner_size([800.0, 600.0])
        .with_resizable(true);

    if let (Some(x), Some(y)) = (window.x, window.y) {
        viewport = viewport.with_position([x, y]);
    }

    if cfg!(target_os = "linux") {
        viewport = viewport.with_app_id(branding::APP_ID);
    }

    let has_saved_position = window.x.is_some();
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Wgpu,
        centered: !has_saved_position,
        ..Default::default()
    };

    eframe::run_native(
        branding::APP_NAME,
        options,
        Box::new(move |cc| Ok(Box::new(HorizonApp::new(cc, &config, config_path)))),
    )
}

fn parse_config_arg() -> Option<PathBuf> {
    let args: Vec<String> = std::env::args().collect();
    for (i, arg) in args.iter().enumerate() {
        if (arg == "--config" || arg == "-c") && i + 1 < args.len() {
            return Some(PathBuf::from(&args[i + 1]));
        }
    }
    None
}
