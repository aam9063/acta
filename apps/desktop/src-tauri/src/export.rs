//! Descargas de una reunión: transcripción (`.txt` / `.md`) y audio (la mezcla
//! final), siempre a `~/Descargas` (`$HOME/Downloads`) sin pisar nada, y rutas
//! de las carpetas de datos para abrirlas en el Finder.
//!
//! No hay diálogo de «Guardar como»: el plugin de diálogos no está en el
//! proyecto y la carpeta de Descargas es lo que la persona espera de un
//! «descargar». La capa de Tauri revela el fichero resultante en el Finder.

use crate::app::AppCore;
use acta_core::ids::MeetingId;
use acta_core::library::MeetingStore;
use acta_core::time::MediaMs;
use acta_core::transcription::Transcript;
use acta_core::{PortError, PortResult};
use acta_llm::transcript::{format_date, mmss, speaker_label};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TranscriptFormat {
    Txt,
    Md,
}

impl TranscriptFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Txt => "txt",
            Self::Md => "md",
        }
    }
}

/// Fichero que se acaba de escribir (ruta absoluta).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportedFile {
    pub path: String,
}

/// Texto de la transcripción, con el nombre de cada hablante y el instante.
pub fn render_transcript(title: &str, started: &str, duration: MediaMs, transcript: &Transcript, me_name: &str, format: TranscriptFormat) -> String {
    let names: HashMap<_, _> = transcript.speakers.iter().map(|s| (s.id, speaker_label(s, me_name))).collect();
    let mut utterances: Vec<_> = transcript.utterances.iter().filter(|u| !u.text.trim().is_empty()).collect();
    utterances.sort_by_key(|u| (u.start, u.idx));
    let meta = if duration.0 > 0 { format!("{started} · {}", mmss(duration)) } else { started.to_owned() };
    let mut out = String::new();
    match format {
        TranscriptFormat::Txt => out.push_str(&format!("{title}\n{meta}\n\n")),
        TranscriptFormat::Md => out.push_str(&format!("# {title}\n\n*{meta}*\n\n")),
    }
    for u in utterances {
        let who = names.get(&u.speaker_id).map(String::as_str).unwrap_or("Desconocido");
        let at = mmss(u.start);
        let text = u.text.trim();
        match format {
            TranscriptFormat::Txt => out.push_str(&format!("[{at}] {who}: {text}\n")),
            TranscriptFormat::Md => out.push_str(&format!("**[{at}] {who}:** {text}\n\n")),
        }
    }
    out
}

/// Título apto para nombre de fichero (sin `/`, `:` ni control) y de largo razonable.
fn file_stem(title: &str) -> String {
    let cleaned: String = title.chars().map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '-' } else { c }).collect();
    let cleaned = cleaned.trim().trim_matches('.').trim();
    let short: String = cleaned.chars().take(80).collect();
    if short.is_empty() { "Reunión".to_owned() } else { short }
}

/// `dir/stem.ext`, o `dir/stem (2).ext`... si ya existe.
fn free_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}).{ext}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

fn io(e: std::io::Error) -> PortError {
    PortError::Internal(format!("No se pudo guardar el fichero: {e}"))
}

impl AppCore {
    fn downloads_dir(&self) -> PathBuf {
        self.home.join("Downloads")
    }

    pub async fn meeting_export_transcript(&self, id: MeetingId, format: TranscriptFormat) -> PortResult<ExportedFile> {
        let meeting = self.store.get_meeting(id).await?.ok_or_else(|| PortError::NotFound("No existe esa reunión.".into()))?;
        let transcript = self.store.get_transcript(id).await?.filter(|t| !t.utterances.is_empty());
        let transcript = transcript.ok_or_else(|| PortError::NotFound("Esta reunión todavía no tiene transcripción.".into()))?;
        let me = crate::pipeline::read(&self.settings).user.display_name;
        let body = render_transcript(&meeting.title, &format_date(meeting.started_at), meeting.duration, &transcript, &me, format);
        let dir = self.downloads_dir();
        std::fs::create_dir_all(&dir).map_err(io)?;
        let path = free_path(&dir, &format!("{} - transcripción", file_stem(&meeting.title)), format.extension());
        std::fs::write(&path, body).map_err(io)?;
        Ok(ExportedFile { path: path.to_string_lossy().into_owned() })
    }

    pub async fn meeting_export_audio(&self, id: MeetingId) -> PortResult<ExportedFile> {
        let meeting = self.store.get_meeting(id).await?.ok_or_else(|| PortError::NotFound("No existe esa reunión.".into()))?;
        let mix = self.layout.mix_wav(id);
        if !mix.is_file() {
            return Err(PortError::NotFound("Esta reunión no tiene audio disponible.".into()));
        }
        let dir = self.downloads_dir();
        std::fs::create_dir_all(&dir).map_err(io)?;
        let path = free_path(&dir, &format!("{} - audio", file_stem(&meeting.title)), "wav");
        std::fs::copy(&mix, &path).map_err(io)?;
        Ok(ExportedFile { path: path.to_string_lossy().into_owned() })
    }

    /// Carpeta de la reunión (audio, capturas, transcript). Se crea si falta.
    pub async fn meeting_folder(&self, id: MeetingId) -> PortResult<PathBuf> {
        self.store.get_meeting(id).await?.ok_or_else(|| PortError::NotFound("No existe esa reunión.".into()))?;
        let dir = self.layout.meeting_dir(id);
        std::fs::create_dir_all(&dir).map_err(io)?;
        Ok(dir)
    }

    /// Carpeta de datos general (`appInfo.dataDir`).
    pub fn data_folder(&self) -> PathBuf {
        self.layout.root().to_path_buf()
    }
}
