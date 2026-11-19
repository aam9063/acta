//! Vigilancia de salud de la grabación (WP1). Tipos de contrato + política
//! pura; el `Watchdog` con I/O (disco libre, ficheros) vive en acta-audio.
//!
//! Reglas por defecto (`HealthConfig`):
//! - `TrackStalled`: una pista no entrega frames durante `stall_ms` (3 s).
//! - `TrackAllZeros`: una pista entrega SOLO ceros exactos durante
//!   `zeros_ms` (10 s). Es la firma de TCC denegado (Core Audio devuelve
//!   silencio digital en vez de error).
//! - `SegmentsStalled`: no se finaliza ningún segmento en
//!   `segment_seconds + segment_grace_ms` (el fallo de Meetily: los
//!   checkpoints dejaron de aparecer y nadie avisó).
//! - `DiskLow`: menos de `min_free_bytes` en el volumen de datos.
//! - `Overrun`, `SidecarRestarted`, `WriteFailed`: al ocurrir.
//! - `PermissionMissing`: mientras falte (se limpia con `on_permission_granted`).
//!
//! Cada aviso se emite al empezar y se "limpia" (`cleared = true`) al
//! resolverse. La UI lo muestra en la pastilla (punto rojo + texto).

use crate::permissions::PermissionKind;
use crate::recording::Track;
use crate::time::UnixMs;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum HealthIssue {
    TrackStalled { track: Track },
    TrackAllZeros { track: Track },
    SegmentsStalled { track: Track },
    DiskLow { free_bytes: u64 },
    Overrun { track: Track, dropped_frames: u64 },
    SidecarRestarted { count: u32 },
    WriteFailed { track: Track, message: String },
    PermissionMissing { permission: PermissionKind },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthWarning {
    pub issue: HealthIssue,
    pub since: UnixMs,
    /// `true` = el problema se ha resuelto; la UI quita el aviso.
    pub cleared: bool,
    /// Texto para el usuario, en castellano.
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthConfig {
    pub stall_ms: u64,
    pub zeros_ms: u64,
    pub segment_grace_ms: u64,
    pub min_free_bytes: u64,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            stall_ms: 3_000,
            zeros_ms: 10_000,
            segment_grace_ms: 10_000,
            min_free_bytes: 1_000_000_000,
        }
    }
}

// ---------------------------------------------------------------------------
// Política pura: HealthMonitor
// ---------------------------------------------------------------------------

use crate::recording::TrackState;
use std::collections::HashMap;

#[derive(Debug, Clone)]
struct TrackHealth {
    last_frame: UnixMs,
    zero_since: Option<UnixMs>,
    last_segment: UnixMs,
    /// El adaptador está reconectando (cambio de dispositivo, sidecar...).
    restarting: bool,
    failed: bool,
}

/// Evalúa las reglas de salud. Pura: el tiempo llega en cada llamada, así que
/// se prueba con relojes falsos. El `Watchdog` de acta-audio la envuelve y le
/// añade la medición del disco.
///
/// Un aviso persistente (parada, ceros, segmentos, disco, permiso) se emite
/// una vez al aparecer y otra con `cleared = true` al resolverse. Los avisos
/// puntuales (overrun, escritura fallida, reinicio del sidecar) se emiten una
/// sola vez en el siguiente `tick`.
#[derive(Debug, Clone)]
pub struct HealthMonitor {
    pub config: HealthConfig,
    segment_ms: u64,
    tracks: HashMap<Track, TrackHealth>,
    active: HashMap<HealthIssue, UnixMs>,
    one_shot: Vec<HealthWarning>,
    paused: bool,
    sidecar_restarts: u32,
    /// Permisos que faltan ahora mismo (aviso persistente, no de un disparo:
    /// la pastilla lo quita cuando el reintento funciona).
    missing: std::collections::BTreeMap<PermissionKind, String>,
}

impl HealthMonitor {
    pub fn new(config: HealthConfig, segment_seconds: u32, tracks: &[Track], started_at: UnixMs) -> Self {
        let tracks = tracks
            .iter()
            .map(|t| {
                (
                    *t,
                    TrackHealth {
                        last_frame: started_at,
                        zero_since: None,
                        last_segment: started_at,
                        restarting: false,
                        failed: false,
                    },
                )
            })
            .collect();
        Self {
            config,
            segment_ms: u64::from(segment_seconds) * 1000,
            tracks,
            active: HashMap::new(),
            one_shot: Vec::new(),
            paused: false,
            sidecar_restarts: 0,
            missing: std::collections::BTreeMap::new(),
        }
    }

    /// Un frame de `track` (`all_zero` = ceros exactos, firma de TCC denegado).
    pub fn on_frame(&mut self, track: Track, all_zero: bool, at: UnixMs) {
        if let Some(t) = self.tracks.get_mut(&track) {
            t.last_frame = at;
            t.failed = false;
            if all_zero {
                t.zero_since.get_or_insert(at);
            } else {
                t.zero_since = None;
            }
        }
    }

    pub fn on_segment_finalized(&mut self, track: Track, at: UnixMs) {
        if let Some(t) = self.tracks.get_mut(&track) {
            t.last_segment = at;
        }
    }

    pub fn on_track_state(&mut self, track: Track, state: TrackState, at: UnixMs) {
        if let Some(t) = self.tracks.get_mut(&track) {
            match state {
                TrackState::Restarting => t.restarting = true,
                TrackState::Running => {
                    t.restarting = false;
                    t.failed = false;
                    // Margen de gracia: el primer frame tardará un poco.
                    t.last_frame = at;
                    t.last_segment = t.last_segment.max(at);
                }
                TrackState::Stalled | TrackState::Failed => {
                    t.restarting = false;
                    t.failed = true;
                }
                TrackState::Stopped => {}
            }
        }
    }

    pub fn on_overrun(&mut self, track: Track, dropped_frames: u64, at: UnixMs) {
        self.push_one_shot(
            HealthIssue::Overrun { track, dropped_frames },
            at,
            format!("Se han perdido {dropped_frames} bloques de audio en la pista {}.", track_es(track)),
        );
    }

    pub fn on_write_failed(&mut self, track: Track, message: String, at: UnixMs) {
        let text = format!(
            "No se pudo guardar un trozo de audio de la pista {} ({message}).",
            track_es(track)
        );
        self.push_one_shot(HealthIssue::WriteFailed { track, message }, at, text);
    }

    pub fn on_sidecar_restarted(&mut self, at: UnixMs) {
        self.sidecar_restarts += 1;
        let count = self.sidecar_restarts;
        self.push_one_shot(
            HealthIssue::SidecarRestarted { count },
            at,
            format!("La captura de audio se ha reiniciado ({count} veces). Puede haber un hueco en el audio."),
        );
        for t in self.tracks.values_mut() {
            t.last_frame = at;
            t.restarting = false;
        }
    }

    /// Falta `permission`. El aviso dura hasta `on_permission_granted`.
    pub fn on_permission_missing(&mut self, permission: PermissionKind, _at: UnixMs) {
        let message = match permission {
            PermissionKind::ScreenRecording => {
                "Grabación de pantalla sin permiso: no habrá capturas (el audio se graba bien). Actívalo en Ajustes del Sistema › Privacidad › Grabación de pantalla; si ya está activado, quita acta de la lista (−) y vuelve a añadirla (macOS guarda el permiso de la versión anterior). Se reintenta solo."
            }
            PermissionKind::Microphone => "Falta el permiso del Micrófono: tu voz no se está grabando. Actívalo en Ajustes del Sistema › Privacidad › Micrófono.",
            PermissionKind::SystemAudio => "Falta el permiso de Audio del sistema: no se graba a los demás. Actívalo en Ajustes del Sistema › Privacidad.",
            PermissionKind::Accessibility => "Accesibilidad sin permiso: no se sabrá quién habla en Teams (el audio se graba bien).",
            PermissionKind::Automation => "Automatización sin permiso: no se detectará Google Meet (el audio se graba bien).",
        };
        self.missing.insert(permission, message.into());
    }

    /// El permiso ya funciona (p. ej. el reintento de la captura arrancó).
    pub fn on_permission_granted(&mut self, permission: PermissionKind) {
        self.missing.remove(&permission);
    }

    pub fn on_paused(&mut self, _at: UnixMs) {
        self.paused = true;
    }

    /// Al reanudar se da margen de gracia a todas las pistas.
    pub fn on_resumed(&mut self, at: UnixMs) {
        self.paused = false;
        for t in self.tracks.values_mut() {
            t.last_frame = at;
            t.last_segment = at;
            t.zero_since = None;
        }
    }

    /// Llamar a 1 Hz. Devuelve avisos NUEVOS o LIMPIADOS desde el último tick.
    pub fn tick(&mut self, now: UnixMs, free_disk_bytes: Option<u64>) -> Vec<HealthWarning> {
        let mut out = std::mem::take(&mut self.one_shot);
        let mut wanted: Vec<(HealthIssue, String)> = Vec::new();

        if !self.paused {
            for (&track, t) in &self.tracks {
                if t.failed {
                    wanted.push((HealthIssue::TrackStalled { track }, stalled_msg(track)));
                    continue;
                }
                if t.restarting {
                    continue;
                }
                if now.since(t.last_frame).max(0) as u64 >= self.config.stall_ms {
                    wanted.push((HealthIssue::TrackStalled { track }, stalled_msg(track)));
                }
                if let Some(z) = t.zero_since
                    && now.since(z).max(0) as u64 >= self.config.zeros_ms
                {
                    let message = match track {
                        Track::Mic => "El micrófono solo da silencio digital. Revisa el permiso de Micrófono; \
                            si el portátil tiene la tapa cerrada, su micro integrado está apagado: \
                            elige otro micrófono en Ajustes."
                            .to_owned(),
                        Track::System => format!(
                            "La pista {} solo recibe silencio digital: probablemente falta el permiso de audio.",
                            track_es(track)
                        ),
                    };
                    wanted.push((HealthIssue::TrackAllZeros { track }, message));
                }
                if now.since(t.last_segment).max(0) as u64 >= self.segment_ms + self.config.segment_grace_ms {
                    wanted.push((
                        HealthIssue::SegmentsStalled { track },
                        format!(
                            "No se está guardando el audio de la pista {}. Detén la grabación y vuelve a empezar.",
                            track_es(track)
                        ),
                    ));
                }
            }
        }
        for (&permission, message) in &self.missing {
            wanted.push((HealthIssue::PermissionMissing { permission }, message.clone()));
        }
        if let Some(free) = free_disk_bytes
            && free < self.config.min_free_bytes
        {
            wanted.push((
                HealthIssue::DiskLow { free_bytes: free },
                "Queda muy poco espacio en disco para seguir grabando.".into(),
            ));
        }

        // Nuevos avisos (DiskLow compara por tipo, no por bytes libres).
        for (issue, message) in &wanted {
            let key = self.find_active(issue);
            if key.is_none() {
                self.active.insert(issue.clone(), now);
                out.push(HealthWarning {
                    issue: issue.clone(),
                    since: now,
                    cleared: false,
                    message: message.clone(),
                });
            }
        }
        // Avisos resueltos.
        let resolved: Vec<HealthIssue> = self
            .active
            .keys()
            .filter(|a| !wanted.iter().any(|(w, _)| same_kind(w, a)))
            .cloned()
            .collect();
        for issue in resolved {
            if let Some(since) = self.active.remove(&issue) {
                out.push(HealthWarning {
                    message: "Resuelto.".into(),
                    issue,
                    since,
                    cleared: true,
                });
            }
        }
        out
    }

    fn find_active(&self, issue: &HealthIssue) -> Option<&HealthIssue> {
        self.active.keys().find(|a| same_kind(a, issue))
    }

    fn push_one_shot(&mut self, issue: HealthIssue, at: UnixMs, message: String) {
        self.one_shot.push(HealthWarning {
            issue,
            since: at,
            cleared: false,
            message,
        });
    }
}

/// Dos avisos persistentes son "el mismo" si coinciden en tipo y pista
/// (los bytes libres de `DiskLow` cambian continuamente).
fn same_kind(a: &HealthIssue, b: &HealthIssue) -> bool {
    use HealthIssue as H;
    match (a, b) {
        (H::DiskLow { .. }, H::DiskLow { .. }) => true,
        _ => a == b,
    }
}

fn track_es(track: Track) -> &'static str {
    match track {
        Track::Mic => "del micrófono",
        Track::System => "del sistema",
    }
}

fn stalled_msg(track: Track) -> String {
    format!("La pista {} no está entregando audio.", track_es(track))
}
