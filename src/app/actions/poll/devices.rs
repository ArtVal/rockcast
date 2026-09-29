//! Device discovery and remote Chromecast discovery message handlers.

use time::OffsetDateTime;

use crate::{
    app::{RockCastApp, messages::same_output_device},
    cast::CastDeviceInfo,
    i18n,
    output::OutputDevice,
};

impl RockCastApp {
    pub(super) fn handle_device_found(&mut self, device: OutputDevice) {
        let selected_id = self
            .selected_device
            .and_then(|index| self.devices.get(index))
            .map(|device| device.id().to_owned());
        let kind = if device.is_local() { "local" } else { "cast" };
        log::info!(
            "device found incrementally: kind={kind} id={} name='{}'",
            device.id(),
            device.name()
        );
        if let Some(index) = self
            .devices
            .iter()
            .position(|existing| same_output_device(existing, &device))
        {
            self.devices[index] = device;
        } else {
            self.devices.push(device);
        }
        self.devices.sort_by_key(|device| !device.is_local());
        self.selected_device = selected_id
            .as_deref()
            .and_then(|id| self.devices.iter().position(|device| device.id() == id));
        if self.selected_device.is_none() {
            self.restore_device_selection();
        }
        self.status = format!(
            "{} ({})",
            self.lang.t().searching_devices,
            self.devices.len()
        );
        if self.pending_voice_play && self.can_start_play() {
            self.pending_voice_play = false;
            log::info!("voice playback resumed after first audio device");
            self.play();
        }
    }

    pub(super) fn handle_devices_finished(&mut self, status: String) {
        log::info!(
            "device scan finished: count={} status={status}",
            self.devices.len()
        );
        self.restore_device_selection();
        self.loading_devices = false;
        let local_n = self.devices.iter().filter(|d| d.is_local()).count();
        let cast_n = self.devices.len().saturating_sub(local_n);
        let selected = self
            .selected_device
            .and_then(|i| self.devices.get(i))
            .map(|d| d.label(self.lang))
            .unwrap_or_else(|| self.lang.t().device_none.into());
        self.status = if cast_n == 0 {
            i18n::fmt1(self.lang.t().cast_none, local_n)
        } else {
            i18n::fmt3(self.lang.t().cast_found, local_n, cast_n, selected)
        };
        if status.contains("panic") || status.contains("Ошибка") {
            self.status = status;
        }
        if let Some(i) = self.selected_device {
            log::info!(
                "device selected after scan: idx={i} id={}",
                self.devices.get(i).map(|d| d.id()).unwrap_or("?")
            );
        }
    }

    pub(super) fn handle_remote_chromecast_discovery(
        &mut self,
        command_id: String,
        result: Result<Vec<CastDeviceInfo>, ()>,
    ) {
        if self.pending_chromecast_discovery.as_deref() != Some(&command_id) {
            return;
        }
        self.pending_chromecast_discovery = None;
        match result {
            Ok(devices) => {
                let receivers = self.chromecast_receivers.replace_at(
                    devices.into_iter().map(|device| {
                        (
                            device.discovered.id.clone(),
                            device.discovered.name.clone(),
                            device,
                        )
                    }),
                    OffsetDateTime::now_utc(),
                );
                self.device_control.complete_command(
                    &command_id,
                    crate::device_control::CommandResult::succeeded_with_receivers(receivers),
                );
            }
            Err(()) => self.device_control.complete_command(
                &command_id,
                crate::device_control::CommandResult::failed(
                    "command_timeout",
                    "Chromecast discovery failed",
                ),
            ),
        }
    }
}
