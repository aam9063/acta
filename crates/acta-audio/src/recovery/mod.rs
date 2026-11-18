//! Recuperación tras crash (WP1).
//!
//! Tras un crash, en `audio/<pista>/` puede haber: segmentos `.wav` ya
//! cerrados (a veces sin entrar en el manifiesto), un `.wav.tmp` a medias y
//! `.raw` de emergencia. `recover` los reconcilia:
//! - `.wav.tmp`: se reescribe la cabecera según el tamaño REAL (descartando un
//!   sample incompleto), se renombra a `.wav`. Si es menor que una cabecera o
//!   no tiene muestras, se ignora sin borrarlo.
//! - Por cada índice manda: manifiesto > `.wav` > `.raw` > `.wav.tmp`.
//! - `media_start` de los segmentos que el manifiesto no conoce se encadena
//!   tras el anterior de su pista (las pausas no ocupan línea media).

use crate::fileio::{WAV_HEADER_LEN, fsync_dir, wav_header};
use crate::writer::{read_manifest, write_manifest_atomic};
use acta_core::ids::MeetingId;
use acta_core::layout::DataLayout;
use acta_core::recording::{ManifestStatus, SegmentInfo, SessionManifest, StopReason, Track};
use acta_core::time::MediaMs;
use acta_core::{PortError, PortResult};
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

/// Reuniones cuyo manifiesto quedó en `Recording`/`Paused`.
pub fn scan_incomplete(layout: &DataLayout) -> PortResult<Vec<MeetingId>> {
    let dir = layout.meetings_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(id) = name.parse::<MeetingId>() else {
            continue;
        };
        match read_manifest(layout, id) {
            Ok(m) if matches!(m.status, ManifestStatus::Recording | ManifestStatus::Paused) => {
                found.push(id)
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(%id, error = %e, "unreadable manifest, skipping"),
        }
    }
    found.sort();
    Ok(found)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Tmp,
    Raw,
    Wav,
}

fn parse_name(name: &str) -> Option<(u32, Kind)> {
    let rest = name.strip_prefix("seg-")?;
    let (num, kind) = if let Some(n) = rest.strip_suffix(".wav.tmp") {
        (n, Kind::Tmp)
    } else if let Some(n) = rest.strip_suffix(".wav") {
        (n, Kind::Wav)
    } else {
        let n = rest.strip_suffix(".raw")?;
        (n, Kind::Raw)
    };
    Some((num.parse().ok()?, kind))
}

/// Muestras válidas de un `.wav.tmp`, tras reescribir su cabecera. `None` si
/// está truncado (sin cabecera completa o sin una sola muestra).
fn repair_tmp(path: &Path, rate: u32) -> std::io::Result<Option<u64>> {
    let len = std::fs::metadata(path)?.len();
    if len < WAV_HEADER_LEN + 2 {
        return Ok(None);
    }
    let samples = (len - WAV_HEADER_LEN) / 2; // descarta un byte suelto
    let mut f = OpenOptions::new().read(true).write(true).open(path)?;
    f.seek(SeekFrom::Start(0))?;
    f.write_all(&wav_header(samples, rate))?;
    f.set_len(WAV_HEADER_LEN + samples * 2)?;
    f.sync_all()?;
    Ok(Some(samples))
}

fn wav_samples(path: &Path) -> Option<u64> {
    hound::WavReader::open(path)
        .ok()
        .map(|r| u64::from(r.len()))
}

/// Repara `.wav.tmp` (cabecera según tamaño real, descarta medio sample),
/// los renombra, recalcula el manifiesto y lo deja en `Stopped`.
pub fn recover(layout: &DataLayout, meeting_id: MeetingId) -> PortResult<SessionManifest> {
    let mut manifest = read_manifest(layout, meeting_id)?;
    let rate = manifest.format.sample_rate;
    let rate64 = u64::from(rate);

    for track in Track::ALL {
        let dir = layout.track_dir(meeting_id, track);
        let mut by_index: BTreeMap<u32, Vec<(Kind, String)>> = BTreeMap::new();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if let Some((idx, kind)) = parse_name(&name) {
                    by_index.entry(idx).or_default().push((kind, name));
                }
            }
        }
        let tm = manifest
            .tracks
            .iter_mut()
            .find(|t| t.track == track)
            .ok_or_else(|| PortError::Internal("manifest without track".into()))?;
        let known: BTreeMap<u32, SegmentInfo> =
            tm.segments.drain(..).map(|s| (s.index, s)).collect();
        let mut result: BTreeMap<u32, SegmentInfo> = known.clone();
        let mut end_of_prev: u64 = 0; // en muestras
        let all_indices: Vec<u32> = by_index
            .keys()
            .chain(known.keys())
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();

        for idx in all_indices {
            if let Some(info) = known.get(&idx) {
                end_of_prev = info.media_start.0 * rate64 / 1000 + info.samples;
                continue;
            }
            let mut candidates = by_index.remove(&idx).unwrap_or_default();
            candidates.sort_by_key(|a| std::cmp::Reverse(a.0)); // Wav > Raw > Tmp
            for (kind, name) in candidates {
                let path = dir.join(&name);
                let rel = format!("audio/{}/seg-{idx:06}", track.as_str());
                let (samples, file) = match kind {
                    Kind::Wav => match wav_samples(&path) {
                        Some(n) if n > 0 => (n, format!("{rel}.wav")),
                        _ => continue,
                    },
                    Kind::Raw => {
                        let n = std::fs::metadata(&path).map(|m| m.len() / 2).unwrap_or(0);
                        if n == 0 {
                            continue;
                        }
                        (n, format!("{rel}.raw"))
                    }
                    Kind::Tmp => {
                        let final_path = layout.segment_path(meeting_id, track, idx);
                        if final_path.exists() {
                            continue;
                        }
                        match repair_tmp(&path, rate) {
                            Ok(Some(n)) => {
                                std::fs::rename(&path, &final_path)?;
                                fsync_dir(&dir)?;
                                (n, format!("{rel}.wav"))
                            }
                            Ok(None) => continue,
                            Err(e) => {
                                tracing::warn!(?path, error = %e, "cannot repair tmp segment");
                                continue;
                            }
                        }
                    }
                };
                result.insert(
                    idx,
                    SegmentInfo {
                        index: idx,
                        file,
                        media_start: MediaMs(end_of_prev * 1000 / rate64),
                        samples,
                        padded_samples: 0,
                    },
                );
                end_of_prev += samples;
                break;
            }
        }
        tm.segments = result.into_values().collect();
    }

    let closed_at = manifest.updated_at;
    for span in &mut manifest.spans {
        if span.wall_end.is_none() {
            span.wall_end = Some(closed_at.max(span.wall_start));
        }
    }
    manifest.status = ManifestStatus::Stopped;
    manifest.stop_reason.get_or_insert(StopReason::Error);
    write_manifest_atomic(layout, &manifest)?;
    Ok(manifest)
}
