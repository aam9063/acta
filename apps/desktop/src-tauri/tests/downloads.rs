#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Descargar la transcripción y el audio de una reunión y abrir sus carpetas.
//! Todo cae en un `$HOME` temporal: nada toca el del usuario.

use acta_audio::supervisor::SupervisorConfig;
use acta_core::detection::Platform;
use acta_core::ids::*;
use acta_core::layout::DataLayout;
use acta_core::library::*;
use acta_core::recording::Track;
use acta_core::time::{MediaMs, SystemClock, UnixMs};
use acta_core::transcription::{NameSource, Speaker, Transcript, Utterance};
use acta_core::PortError;
use acta_desktop_lib::app::{AppCore, CoreDeps, Emit};
use acta_desktop_lib::export::TranscriptFormat;
use acta_desktop_lib::native::NativePorts;
use acta_store::SqliteStore;
use acta_store::secrets::InMemorySecretStore;
use std::sync::Arc;
use std::time::Duration;

mod support;

struct Quiet;
impl Emit for Quiet {
    fn emit(&self, _event: &str, _payload: serde_json::Value) {}
}

struct Rig {
    _dir: tempfile::TempDir,
    layout: DataLayout,
    core: AppCore,
}

async fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("data")).unwrap();
    let layout = DataLayout::new(dir.path().join("data"));
    let store = Arc::new(SqliteStore::open(&layout.db_path()).unwrap().with_layout(layout.clone()));
    let mut core = AppCore::start(CoreDeps {
        layout: layout.clone(),
        clock: Arc::new(SystemClock),
        store,
        secrets: Arc::new(InMemorySecretStore::default()),
        factory: None,
        local_llm: None,
        native: NativePorts::unavailable("tests"),
        emitter: Arc::new(Quiet),
        supervisor_config: SupervisorConfig { tick: Duration::from_millis(30), stop_drain: Duration::from_millis(200) },
    })
    .await
    .unwrap();
    core.home = dir.path().join("home");
    core.settings_update(support::configured_settings()).await.unwrap();
    Rig { _dir: dir, layout, core }
}

async fn meeting(core: &AppCore, n: u128, title: &str) -> MeetingId {
    let id = MeetingId::from_u128(n);
    core.store
        .create_meeting(NewMeeting { id, title: title.into(), platform: Platform::Teams, app_bundle_id: None, started_at: UnixMs(1_794_821_400_000), language: "es".into() })
        .await
        .unwrap();
    let sp = |k: u128, label: &str, name: Option<&str>, me: bool| Speaker {
        id: SpeakerId::from_u128(k),
        meeting_id: id,
        label: label.into(),
        display_name: name.map(str::to_owned),
        is_me: me,
        track: if me { Track::Mic } else { Track::System },
        diarization_label: None,
        name_source: NameSource::Generic,
        confirmed: false,
        voiceprint_id: None,
    };
    let utt = |idx: u32, k: u128, start: u64, text: &str| Utterance { idx, speaker_id: SpeakerId::from_u128(k), track: Track::Mic, start: MediaMs(start), end: MediaMs(start + 2_000), text: text.into(), confidence: None };
    core.store
        .save_transcript(Transcript {
            meeting_id: id,
            speakers: vec![sp(1, "Yo", None, true), sp(2, "Speaker 2", Some("Ana García"), false)],
            utterances: vec![utt(1, 1, 6_000, "Sí, yo me encargo."), utt(0, 2, 1_000, "¿Puedes enviar el presupuesto?"), utt(2, 2, 3_725_000, "Cerramos aquí.")],
        })
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn the_transcript_is_downloaded_as_text_with_names_and_timestamps() {
    let r = rig().await;
    let id = meeting(&r.core, 1, "Daily / equipo: ¿qué?").await;
    let file = r.core.meeting_export_transcript(id, TranscriptFormat::Txt).await.unwrap();
    let path = std::path::PathBuf::from(&file.path);
    assert_eq!(path.parent().unwrap(), r.core.home.join("Downloads"), "va a Descargas");
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.ends_with(".txt") && !name.contains('/') && !name.contains(':'), "{name}");
    let body = std::fs::read_to_string(&path).unwrap();
    assert!(body.starts_with("Daily / equipo: ¿qué?\n"), "{body}");
    // Orden temporal, nombre real, «Ana (yo)» para el usuario y horas pasada la primera.
    let lines: Vec<&str> = body.lines().filter(|l| l.starts_with('[')).collect();
    assert_eq!(
        lines,
        vec!["[00:01] Ana García: ¿Puedes enviar el presupuesto?", "[00:06] Ana (yo): Sí, yo me encargo.", "[1:02:05] Ana García: Cerramos aquí."]
    );
}

#[tokio::test]
async fn the_markdown_transcript_has_a_title_and_bold_speakers() {
    let r = rig().await;
    let id = meeting(&r.core, 2, "Retro").await;
    let file = r.core.meeting_export_transcript(id, TranscriptFormat::Md).await.unwrap();
    assert!(file.path.ends_with(".md"));
    let body = std::fs::read_to_string(&file.path).unwrap();
    assert!(body.starts_with("# Retro\n"), "{body}");
    assert!(body.contains("**[00:01] Ana García:** ¿Puedes enviar el presupuesto?"), "{body}");
}

#[tokio::test]
async fn downloading_twice_never_overwrites() {
    let r = rig().await;
    let id = meeting(&r.core, 3, "Retro").await;
    let a = r.core.meeting_export_transcript(id, TranscriptFormat::Txt).await.unwrap();
    let b = r.core.meeting_export_transcript(id, TranscriptFormat::Txt).await.unwrap();
    assert_ne!(a.path, b.path);
    assert!(std::path::Path::new(&a.path).exists() && std::path::Path::new(&b.path).exists());
}

#[tokio::test]
async fn the_mixdown_audio_is_copied_to_downloads_and_missing_audio_is_a_clear_error() {
    let r = rig().await;
    let id = meeting(&r.core, 4, "Retro").await;
    assert!(matches!(r.core.meeting_export_audio(id).await, Err(PortError::NotFound(_))));
    let mix = r.layout.mix_wav(id);
    std::fs::create_dir_all(mix.parent().unwrap()).unwrap();
    std::fs::write(&mix, b"RIFFfake").unwrap();
    let file = r.core.meeting_export_audio(id).await.unwrap();
    assert!(file.path.ends_with(".wav"), "{}", file.path);
    assert_eq!(std::fs::read(&file.path).unwrap(), b"RIFFfake");
    assert!(mix.exists(), "el original se queda donde está");
}

#[tokio::test]
async fn folders_resolve_to_the_data_layout_and_unknown_meetings_fail() {
    let r = rig().await;
    let id = meeting(&r.core, 5, "Retro").await;
    let dir = r.core.meeting_folder(id).await.unwrap();
    assert_eq!(dir, r.layout.meeting_dir(id));
    assert!(dir.is_dir(), "se crea si aún no existe para que haya algo que abrir");
    assert_eq!(r.core.data_folder(), r.layout.root());
    assert!(matches!(r.core.meeting_folder(MeetingId::from_u128(99)).await, Err(PortError::NotFound(_))));
}
