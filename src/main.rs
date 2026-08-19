//! RockCast — internet radio on Chromecast (native CASTV2 client).

// Release: GUI only (no console flash). Debug keeps a console for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    sync::{Arc, Mutex},
};

use env_logger::Target;
use rockcast::{
    app::RockCastApp,
    settings::{self, AppSettings},
};

/// Writes log lines to a file and (in debug) also to stderr.
struct TeeLog {
    file: Arc<Mutex<File>>,
    also_stderr: bool,
}

impl Write for TeeLog {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Ok(mut f) = self.file.lock() {
            let _ = f.write_all(buf);
            let _ = f.flush();
        }
        if self.also_stderr {
            let _ = io::stderr().write_all(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Ok(mut f) = self.file.lock() {
            let _ = f.flush();
        }
        if self.also_stderr {
            let _ = io::stderr().flush();
        }
        Ok(())
    }
}

fn init_logging() -> std::path::PathBuf {
    let path = settings::log_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    // Fresh log each launch so a failure session is easy to share.
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .unwrap_or_else(|_| File::create(&path).expect("create rockcast.log"));

    let tee = TeeLog {
        file: Arc::new(Mutex::new(file)),
        also_stderr: cfg!(debug_assertions),
    };

    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("rockcast=debug,info"),
    )
    .format_timestamp_millis()
    .target(Target::Pipe(Box::new(tee)))
    .init();

    path
}

fn main() -> eframe::Result<()> {
    // rustls 0.23: crypto provider required
    let _ = rustls::crypto::ring::default_provider().install_default();
    let log_path = init_logging();
    log::info!("RockCast starting; log file: {}", log_path.display());
    if rockcast::profile::enabled() {
        log::info!(
            "Playback diagnostics ON — PLAYBACK_DIAG every 2s + DIAG warnings in {}",
            log_path.display()
        );
    }
    #[cfg(debug_assertions)]
    log::debug!("Optional telemetry: set ROCKCAST_METRICS=1 or ROCKCAST_PROFILE=1 before launch");

    let settings = AppSettings::load();
    let title = settings.language.t().window_title;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([940.0, 740.0])
            .with_min_inner_size([820.0, 640.0])
            .with_title(title),
        ..Default::default()
    };

    eframe::run_native(
        "RockCast",
        options,
        Box::new(|cc| Ok(Box::new(RockCastApp::new(cc)))),
    )
}
