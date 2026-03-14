#![forbid(unsafe_code)]

mod app;
mod branding;
mod input;
mod terminal_widget;
mod theme;

use std::path::PathBuf;

use app::OrbitermApp;
use orbiterm_core::Config;

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("orbiterm=info,orbiterm_core=info")),
        )
        .init();

    // Parse --config <path> from CLI args
    let config_path = parse_config_arg();
    let config = Config::load(config_path.as_deref()).unwrap_or_else(|e| {
        tracing::error!("failed to load config: {e}");
        Config::default()
    });

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(branding::APP_NAME)
            .with_app_id(branding::APP_ID)
            .with_icon(branding::app_icon())
            .with_decorations(false)
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([800.0, 600.0])
            .with_resizable(true),
        renderer: eframe::Renderer::Wgpu,
        centered: true,
        ..Default::default()
    };

    eframe::run_native(
        branding::APP_NAME,
        options,
        Box::new(move |cc| Ok(Box::new(OrbitermApp::new(cc, &config, config_path)))),
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
