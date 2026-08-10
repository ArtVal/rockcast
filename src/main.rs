//! RockCast — internet radio on Chromecast (native CASTV2 client).

use rockcast::{app::RockCastApp, settings::AppSettings};

fn main() -> eframe::Result<()> {
    // rustls 0.23: crypto provider required
    let _ = rustls::crypto::ring::default_provider().install_default();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let settings = AppSettings::load();
    let title = settings.language.t().window_title;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([900.0, 700.0])
            .with_min_inner_size([780.0, 620.0])
            .with_title(title),
        ..Default::default()
    };

    eframe::run_native(
        "RockCast",
        options,
        Box::new(|cc| Ok(Box::new(RockCastApp::new(cc)))),
    )
}
