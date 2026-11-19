//! Descargar la transcripción / el audio de una reunión y abrir sus carpetas
//! en el Finder. La lógica (qué se escribe y dónde) vive en `crate::export`.

use super::Core;
use crate::export::{ExportedFile, TranscriptFormat};
use acta_core::api::{ApiError, ApiErrorCode, ApiResult};
use acta_core::ids::MeetingId;
use std::path::Path;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError { code: ApiErrorCode::Internal, message: e.to_string() }
}

/// Selecciona el fichero recién guardado en una ventana del Finder.
fn reveal_file(app: &AppHandle, file: &ExportedFile) {
    if let Err(e) = app.opener().reveal_item_in_dir(&file.path) {
        tracing::warn!("no se pudo mostrar {} en el Finder: {e}", file.path);
    }
}

fn open_folder(app: &AppHandle, dir: &Path) -> ApiResult<()> {
    app.opener().open_path(dir.to_string_lossy(), None::<&str>).map_err(internal)
}

#[tauri::command]
pub async fn meeting_export_transcript(app: AppHandle, state: State<'_, Core>, id: MeetingId, format: TranscriptFormat) -> ApiResult<ExportedFile> {
    let file = state.meeting_export_transcript(id, format).await.map_err(ApiError::from)?;
    reveal_file(&app, &file);
    Ok(file)
}

#[tauri::command]
pub async fn meeting_export_audio(app: AppHandle, state: State<'_, Core>, id: MeetingId) -> ApiResult<ExportedFile> {
    let file = state.meeting_export_audio(id).await.map_err(ApiError::from)?;
    reveal_file(&app, &file);
    Ok(file)
}

#[tauri::command]
pub async fn open_meeting_folder(app: AppHandle, state: State<'_, Core>, id: MeetingId) -> ApiResult<()> {
    let dir = state.meeting_folder(id).await.map_err(ApiError::from)?;
    open_folder(&app, &dir)
}

#[tauri::command]
pub async fn open_data_folder(app: AppHandle, state: State<'_, Core>) -> ApiResult<()> {
    open_folder(&app, &state.data_folder())
}
