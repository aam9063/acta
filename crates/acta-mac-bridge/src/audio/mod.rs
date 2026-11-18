//! `AudioSource` sobre el sidecar: `capture.*` + eventos `audio.*`.
//!
//! - Los frames llegan en base64 s16le y se entregan como `Vec<i16>`.
//! - Si el sidecar muere y el cliente lo relanza, se repite `capture.start`
//!   (con los PIDs vigentes) y, si estaba en pausa, `capture.pause`; después se
//!   emite `AudioEvent::Restarted` con el nuevo origen de `pts_us`.
//! - Si el cliente se rinde: `TrackState{Failed}` en ambas pistas y fin.

use crate::client::{SidecarClient, SidecarMessage};
use crate::protocol::{
    AudioFrameEvent, CaptureRetargetArgs, CaptureStartArgs, CaptureStartResult, Command, Event, MicArgs, SystemArgs,
    TrackName, TrackStateName,
};
use crate::task::{Slot, spawn_stream};
use acta_core::recording::{AudioEvent, AudioFrame, AudioSource, CaptureRequest, CaptureStarted, SystemTarget, Track, TrackState};
use acta_core::time::UnixMs;
use acta_core::{EventStream, PortError, PortResult};
use async_trait::async_trait;
use base64::Engine;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{mpsc, oneshot};

const START_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Default)]
struct State {
    args: Option<CaptureStartArgs>,
    paused: bool,
    /// Pide a la tarea que drene lo ya publicado y termine.
    drain: Option<oneshot::Sender<()>>,
}

pub struct MacAudioSource {
    client: SidecarClient,
    state: Arc<Mutex<State>>,
    task: Slot,
}

impl MacAudioSource {
    pub fn new(client: SidecarClient) -> Self {
        Self {
            client,
            state: Arc::default(),
            task: Slot::default(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        lock(&self.state)
    }
}

fn lock(s: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn track(t: TrackName) -> Track {
    match t {
        TrackName::Mic => Track::Mic,
        TrackName::System => Track::System,
    }
}

fn track_state(s: TrackStateName) -> TrackState {
    match s {
        TrackStateName::Running => TrackState::Running,
        TrackStateName::Stalled => TrackState::Stalled,
        TrackStateName::Restarting => TrackState::Restarting,
        TrackStateName::Stopped => TrackState::Stopped,
        TrackStateName::Failed => TrackState::Failed,
    }
}

/// Decodifica un frame PCM s16le base64.
pub fn decode_frame(f: &AudioFrameEvent) -> PortResult<AudioFrame> {
    if f.format != "s16le" || f.channels != 1 {
        return Err(PortError::InvalidResponse(format!(
            "unsupported frame format {} x{}",
            f.format, f.channels
        )));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&f.data)
        .map_err(|e| PortError::InvalidResponse(format!("frame base64: {e}")))?;
    if bytes.len() % 2 != 0 {
        return Err(PortError::InvalidResponse("odd PCM byte count".into()));
    }
    let samples = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i16::from_le_bytes(*b))
        .collect();
    Ok(AudioFrame {
        track: track(f.track),
        seq: f.seq,
        pts_us: f.pts_us,
        sample_rate: f.sample_rate,
        samples,
    })
}

fn map_event(event: Event) -> Option<AudioEvent> {
    Some(match event {
        Event::AudioFrame(f) => match decode_frame(&f) {
            Ok(frame) => AudioEvent::Frame(frame),
            Err(e) => {
                tracing::warn!("dropping bad audio frame: {e}");
                return None;
            }
        },
        Event::AudioDeviceChanged(d) => AudioEvent::DeviceChanged {
            track: track(d.track),
            device_name: d.device_name,
            reason: d.reason,
        },
        Event::AudioTrackState(s) => AudioEvent::TrackState {
            track: track(s.track),
            state: track_state(s.state),
            message: s.message,
        },
        Event::AudioOverrun(o) => AudioEvent::Overrun {
            track: track(o.track),
            dropped_frames: o.dropped_frames,
        },
        _ => return None,
    })
}

async fn fail_both(tx: &mpsc::Sender<AudioEvent>, message: &str) {
    for track in Track::ALL {
        let _ = tx
            .send(AudioEvent::TrackState {
                track,
                state: TrackState::Failed,
                message: Some(message.to_owned()),
            })
            .await;
    }
}

/// Tras un reinicio del sidecar: repite el arranque y la pausa si procede.
async fn recapture(client: &SidecarClient, state: &Mutex<State>) -> PortResult<Option<UnixMs>> {
    let (args, paused) = {
        let s = lock(state);
        (s.args.clone(), s.paused)
    };
    let Some(args) = args else { return Ok(None) };
    let res: CaptureStartResult = client
        .call_typed(Command::CaptureStart(args), Some(START_TIMEOUT))
        .await?;
    if paused {
        client.call(Command::CapturePause, None).await?;
    }
    Ok(Some(UnixMs(res.started_at_unix_ms)))
}

#[async_trait]
impl AudioSource for MacAudioSource {
    async fn start(&self, request: CaptureRequest) -> PortResult<(CaptureStarted, EventStream<AudioEvent>)> {
        self.task.cancel();
        let args = CaptureStartArgs {
            session_id: request.meeting_id.to_string(),
            sample_rate: request.format.sample_rate,
            frame_ms: request.frame_ms,
            mic: MicArgs {
                enabled: request.mic.enabled,
                device_uid: request.mic.device_uid,
            },
            system: SystemArgs {
                enabled: request.system.enabled,
                pids: request.system.pids,
                bundle_ids: request.system.bundle_ids,
            },
        };
        // Suscribirse ANTES de arrancar para no perder los primeros frames.
        let mut rx = self.client.subscribe();
        let res: CaptureStartResult = self
            .client
            .call_typed(Command::CaptureStart(args.clone()), Some(START_TIMEOUT))
            .await?;
        let (drain_tx, mut drain_rx) = oneshot::channel();
        *self.lock() = State {
            args: Some(args),
            paused: false,
            drain: Some(drain_tx),
        };
        let client = self.client.clone();
        let state = self.state.clone();
        let (stream, handle) = spawn_stream(move |tx| async move {
            loop {
                let msg = tokio::select! {
                    biased;
                    _ = &mut drain_rx => {
                        // Todo lo publicado antes de la respuesta a capture.stop
                        // ya está en el broadcast: se entrega y se termina.
                        while let Ok(SidecarMessage::Event(env)) = rx.try_recv() {
                            if let Some(ev) = map_event(env.event)
                                && tx.send(ev).await.is_err() {
                                    return;
                                }
                        }
                        return;
                    }
                    msg = rx.recv() => msg,
                };
                let ev = match msg {
                    Ok(SidecarMessage::Event(env)) => match map_event(env.event) {
                        Some(ev) => ev,
                        None => continue,
                    },
                    Ok(SidecarMessage::Restarted { .. }) => match recapture(&client, &state).await {
                        Ok(Some(restarted_at)) => AudioEvent::Restarted { restarted_at },
                        Ok(None) => return,
                        Err(e) => {
                            fail_both(&tx, &format!("capture restart failed: {e}")).await;
                            return;
                        }
                    },
                    Ok(SidecarMessage::GaveUp { message }) => {
                        fail_both(&tx, &message).await;
                        return;
                    }
                    Err(RecvError::Lagged(n)) => {
                        tracing::warn!("audio consumer lagged {n} sidecar messages");
                        for track in Track::ALL {
                            let _ = tx.send(AudioEvent::Overrun { track, dropped_frames: n }).await;
                        }
                        continue;
                    }
                    Err(RecvError::Closed) => return,
                };
                if tx.send(ev).await.is_err() {
                    return;
                }
            }
        });
        self.task.set(handle);
        Ok((
            CaptureStarted {
                started_at: UnixMs(res.started_at_unix_ms),
                mic_device: res.mic_device,
                system_pids: res.system_pids,
            },
            stream,
        ))
    }

    async fn pause(&self) -> PortResult<()> {
        self.client.call(Command::CapturePause, None).await?;
        self.lock().paused = true;
        Ok(())
    }

    async fn resume(&self) -> PortResult<()> {
        self.client.call(Command::CaptureResume, None).await?;
        self.lock().paused = false;
        Ok(())
    }

    async fn stop(&self) -> PortResult<()> {
        self.lock().args = None;
        let res = self.client.call(Command::CaptureStop, None).await;
        let drain = self.lock().drain.take();
        if let Some(drain) = drain {
            let _ = drain.send(());
        }
        if let Some(mut handle) = self.task.take()
            && tokio::time::timeout(Duration::from_secs(2), &mut handle).await.is_err() {
                handle.abort();
            }
        match res {
            // Sidecar caído: no hay nada que parar.
            Ok(_) | Err(PortError::Unavailable(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn retarget(&self, system: SystemTarget) -> PortResult<()> {
        if let Some(args) = self.lock().args.as_mut() {
            args.system = SystemArgs {
                enabled: system.enabled,
                pids: system.pids.clone(),
                bundle_ids: system.bundle_ids.clone(),
            };
        }
        self.client
            .call(
                Command::CaptureRetarget(CaptureRetargetArgs {
                    pids: system.pids,
                    bundle_ids: system.bundle_ids,
                }),
                None,
            )
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(data: &str) -> AudioFrameEvent {
        AudioFrameEvent {
            track: TrackName::Mic,
            seq: 3,
            pts_us: 300_000,
            sample_rate: 16_000,
            channels: 1,
            format: "s16le".into(),
            data: data.into(),
        }
    }

    #[test]
    fn decodes_little_endian_samples() {
        let f = decode_frame(&frame("AQD+/wCAAAA=")).unwrap(); // 1, -2, i16::MIN, 0
        assert_eq!(f.samples, vec![1, -2, i16::MIN, 0]);
        assert_eq!((f.track, f.seq, f.pts_us), (Track::Mic, 3, 300_000));
    }

    #[test]
    fn rejects_bad_payloads() {
        assert!(decode_frame(&frame("@@@")).is_err());
        assert!(decode_frame(&frame("AQ==")).is_err(), "odd byte count");
        let mut f = frame("AQD+/w==");
        f.format = "f32le".into();
        assert!(decode_frame(&f).is_err());
    }
}
