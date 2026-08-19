//! Lifecycle for playback metadata and spectrum observers.

use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

use crate::{
    icy::IcyWatcher,
    spectrum::{BANDS, SpectrumAnalyzer},
};

pub struct StreamObservers {
    icy: IcyWatcher,
    spectrum: SpectrumAnalyzer,
    title_tx: mpsc::Sender<String>,
    title_rx: mpsc::Receiver<String>,
    due: Option<(Instant, u64, String)>,
}

impl StreamObservers {
    pub fn new() -> Self {
        let (title_tx, title_rx) = mpsc::channel();
        Self {
            icy: IcyWatcher::new(),
            spectrum: SpectrumAnalyzer::new(),
            title_tx,
            title_rx,
            due: None,
        }
    }

    pub fn stop(&mut self) {
        self.due = None;
        self.icy.stop_async();
        self.spectrum.stop_async();
    }

    pub fn schedule(&mut self, generation: u64, url: String, relay: bool) {
        self.stop();
        let delay = if relay { 2_500 } else { 1_200 };
        self.due = Some((
            Instant::now() + Duration::from_millis(delay),
            generation,
            url,
        ));
    }

    pub fn poll(
        &mut self,
        current_generation: u64,
        playing: bool,
        eq_enabled: bool,
        relay_url: Option<&str>,
    ) -> Vec<String> {
        if let Some((due, generation, url)) = self.due.clone()
            && Instant::now() >= due
        {
            self.due = None;
            if playing && generation == current_generation {
                self.start(url, eq_enabled, relay_url);
            }
        }
        self.title_rx.try_iter().collect()
    }

    pub fn start(&mut self, url: String, eq_enabled: bool, relay_url: Option<&str>) {
        self.stop();
        let relay_tap = url.ends_with("/tap");
        if eq_enabled {
            let title_tx = (!relay_tap && relay_url != Some(url.as_str()))
                .then(|| self.title_tx.clone());
            self.spectrum.start(url, title_tx);
        } else if !relay_tap {
            // Relay /tap has no ICY metadata — title comes from StreamRelay upstream.
            let title_tx = (relay_url != Some(url.as_str())).then(|| self.title_tx.clone());
            if let Some(tx) = title_tx {
                self.icy.start(url, tx);
            }
        }
    }

    pub fn sync(
        &mut self,
        playing: bool,
        local: bool,
        eq_enabled: bool,
        tap_url: Option<String>,
        relay_url: Option<&str>,
    ) {
        if !playing || local {
            self.stop();
            return;
        }
        if let Some(url) = tap_url {
            self.start(url, eq_enabled, relay_url);
        } else {
            self.stop();
        }
    }

    pub fn levels(&self) -> [f32; BANDS] {
        self.spectrum.levels()
    }
}

impl Default for StreamObservers {
    fn default() -> Self {
        Self::new()
    }
}
