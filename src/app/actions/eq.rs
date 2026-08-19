//! EQ bar animation and repaint hints.

use crate::observers::BANDS;

use super::super::RockCastApp;

impl RockCastApp {
    pub(in crate::app) fn tick_eq(&mut self, dt: f32) -> bool {
        let targets = if self.eq_enabled && self.playing {
            if self.playing_local {
                self.playback.local_levels()
            } else if self.cast_relay {
                self.playback.relay_levels()
            } else {
                self.observers.levels()
            }
        } else {
            [0.08; BANDS]
        };
        let mut animating = false;
        for ((level, peak), target) in self
            .eq_levels
            .iter_mut()
            .zip(self.eq_peaks.iter_mut())
            .zip(targets)
        {
            if (*level - target).abs() > 0.008 {
                animating = true;
            }
            *level += (target - *level) * (dt * 14.0).min(1.0);
            if self.eq_enabled && self.playing {
                let prev_peak = *peak;
                *peak = peak.max(*level);
                *peak = (*peak - dt * 0.32).max(*level);
                if (prev_peak - *peak).abs() > 0.005 {
                    animating = true;
                }
            } else {
                let prev_peak = *peak;
                *peak += (0.08 - *peak) * (dt * 10.0).min(1.0);
                if (prev_peak - *peak).abs() > 0.005 || (*level - 0.08).abs() > 0.008 {
                    animating = true;
                }
            }
        }
        animating
    }

    pub(in crate::app) fn eq_ui_needs_frames(&self) -> bool {
        if self.eq_enabled && self.playing {
            return true;
        }
        self.eq_levels.iter().any(|l| (*l - 0.08).abs() > 0.015)
            || self.eq_peaks.iter().any(|p| (*p - 0.08).abs() > 0.015)
    }
}
