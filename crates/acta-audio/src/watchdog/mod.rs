//! Watchdog de salud (WP1). Las reglas viven (puras, con relojes falsos) en
//! `acta_core::recording::HealthMonitor`; esto es el envoltorio que recibe
//! frames y eventos del audio. La medición del disco libre (I/O) la hace el
//! supervisor y entra por `tick(.., free_disk_bytes)`.

use acta_core::permissions::PermissionKind;
use acta_core::recording::{
    AudioEvent, AudioFrame, HealthConfig, HealthMonitor, HealthWarning, Track,
};
use acta_core::time::UnixMs;

pub struct Watchdog {
    pub config: HealthConfig,
    monitor: HealthMonitor,
}

impl Watchdog {
    pub fn new(
        config: HealthConfig,
        segment_seconds: u32,
        tracks: &[Track],
        started_at: UnixMs,
    ) -> Self {
        Self {
            config,
            monitor: HealthMonitor::new(config, segment_seconds, tracks, started_at),
        }
    }

    pub fn on_frame(&mut self, frame: &AudioFrame, at: UnixMs) {
        let all_zero = frame.samples.iter().all(|&s| s == 0);
        self.monitor.on_frame(frame.track, all_zero, at);
    }

    /// Eventos no-frame del `AudioSource` (reconexión, overrun, reinicio).
    pub fn on_audio_event(&mut self, event: &AudioEvent, at: UnixMs) {
        match event {
            AudioEvent::Frame(f) => self.on_frame(f, at),
            AudioEvent::TrackState { track, state, .. } => {
                self.monitor.on_track_state(*track, *state, at)
            }
            AudioEvent::Overrun {
                track,
                dropped_frames,
            } => self.monitor.on_overrun(*track, *dropped_frames, at),
            AudioEvent::Restarted { .. } => self.monitor.on_sidecar_restarted(at),
            AudioEvent::DeviceChanged { track, .. } => {
                self.monitor.on_track_state(
                    *track,
                    acta_core::recording::TrackState::Restarting,
                    at,
                );
            }
        }
    }

    pub fn on_segment_finalized(&mut self, track: Track, at: UnixMs) {
        self.monitor.on_segment_finalized(track, at);
    }

    pub fn on_write_failed(&mut self, track: Track, message: String, at: UnixMs) {
        self.monitor.on_write_failed(track, message, at);
    }

    pub fn on_permission_missing(&mut self, permission: PermissionKind, at: UnixMs) {
        self.monitor.on_permission_missing(permission, at);
    }

    pub fn on_permission_granted(&mut self, permission: PermissionKind) {
        self.monitor.on_permission_granted(permission);
    }

    pub fn on_paused(&mut self, at: UnixMs) {
        self.monitor.on_paused(at);
    }

    pub fn on_resumed(&mut self, at: UnixMs) {
        self.monitor.on_resumed(at);
    }

    /// Llamar a 1 Hz. Devuelve avisos NUEVOS o LIMPIADOS desde el último tick.
    pub fn tick(&mut self, now: UnixMs, free_disk_bytes: Option<u64>) -> Vec<HealthWarning> {
        self.monitor.tick(now, free_disk_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acta_core::recording::{HealthIssue, TrackState};

    fn frame(track: Track, v: i16) -> AudioFrame {
        AudioFrame {
            track,
            seq: 0,
            pts_us: 0,
            sample_rate: 16_000,
            samples: vec![v; 1600],
        }
    }

    #[test]
    fn denied_system_audio_permission_shows_up_as_all_zeros() {
        let t0 = UnixMs(0);
        let mut w = Watchdog::new(HealthConfig::default(), 30, &Track::ALL, t0);
        for i in 0..120 {
            let at = t0.plus_ms(i * 100);
            w.on_frame(&frame(Track::Mic, 50), at);
            w.on_frame(&frame(Track::System, 0), at);
        }
        let warnings = w.tick(t0.plus_ms(11_900), Some(10_000_000_000));
        assert!(warnings.iter().any(|x| x.issue
            == HealthIssue::TrackAllZeros {
                track: Track::System
            }
            && !x.cleared));
    }

    #[test]
    fn device_change_then_running_is_a_reconnect_not_an_error() {
        let t0 = UnixMs(0);
        let mut w = Watchdog::new(HealthConfig::default(), 30, &Track::ALL, t0);
        w.on_frame(&frame(Track::System, 5), t0);
        w.on_frame(&frame(Track::Mic, 5), t0);
        w.on_audio_event(
            &AudioEvent::DeviceChanged {
                track: Track::Mic,
                device_name: None,
                reason: "default_input_changed".into(),
            },
            t0.plus_ms(100),
        );
        w.on_frame(&frame(Track::System, 5), t0.plus_ms(4_000));
        assert!(w.tick(t0.plus_ms(4_000), None).is_empty());
        w.on_audio_event(
            &AudioEvent::TrackState {
                track: Track::Mic,
                state: TrackState::Running,
                message: None,
            },
            t0.plus_ms(4_500),
        );
        w.on_frame(&frame(Track::Mic, 5), t0.plus_ms(4_600));
        w.on_frame(&frame(Track::System, 5), t0.plus_ms(4_600));
        assert!(w.tick(t0.plus_ms(5_000), None).is_empty());
    }
}
