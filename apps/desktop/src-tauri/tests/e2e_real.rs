#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Pruebas de extremo a extremo con TODO real: sidecar `acta-mac`, FluidAudio,
//! SQLite en un directorio temporal, el `claude` del usuario y el Llavero.
//! Ignoradas por defecto (tardan minutos y necesitan hardware real):
//!
//! ```bash
//! ACTA_E2E=1 ACTA_MAC_SIDECAR=$PWD/sidecars/acta-mac/.build/release/acta-mac \
//! ACTA_E2E_MEETING=<carpeta con mic.wav y system.wav> \
//!   cargo test -p acta-desktop --features local-embeddings --test e2e_real -- --ignored --test-threads=1 --nocapture
//! ```
//! Los modelos de FluidAudio se leen de `~/Library/Application Support/acta/models`
//! (el sidecar no admite otra ruta); los DATOS van siempre a un directorio temporal.

use acta_audio::supervisor::SupervisorConfig;
use acta_core::chat::{ChatRole, ChatScope};
use acta_core::ids::*;
use acta_core::layout::DataLayout;
use acta_core::library::*;
use acta_core::llm::ProviderKind;
use acta_core::recording::{SessionState, Track};
use acta_core::time::SystemClock;
use acta_desktop_lib::app::{AppCore, CoreDeps, Emit};
use acta_store::SqliteStore;
use acta_store::secrets::{InMemorySecretStore, KeychainSecretStore, SecretStore};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod support;

#[derive(Default)]
struct Events(Mutex<Vec<(String, serde_json::Value)>>);
impl Emit for Events {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        self.0.lock().unwrap().push((event.to_owned(), payload));
    }
}
impl Events {
    fn names(&self) -> Vec<String> {
        self.0.lock().unwrap().iter().map(|(n, _)| n.clone()).collect()
    }
}

fn enabled() -> bool {
    std::env::var("ACTA_E2E").is_ok()
}

/// Con `ACTA_E2E_TRACE=1`, los logs (tiempos de cada paso) van a stderr.
fn trace() {
    if std::env::var("ACTA_E2E_TRACE").is_ok() {
        let _ = tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).with_writer(std::io::stderr).try_init();
    }
}

struct Rig {
    layout: DataLayout,
    core: AppCore,
    events: Arc<Events>,
}

async fn rig(root: &Path) -> Rig {
    rig_with(root, |n| n).await
}

/// Igual que [`rig`], pero deja sustituir algún puerto nativo (p. ej. el
/// detector, que no se puede provocar sin una llamada de Teams de verdad).
async fn rig_with(root: &Path, patch: impl FnOnce(acta_desktop_lib::native::NativePorts) -> acta_desktop_lib::native::NativePorts) -> Rig {
    let layout = DataLayout::new(root);
    for d in [layout.logs_dir(), layout.models_dir(), layout.meetings_dir()] {
        std::fs::create_dir_all(d).unwrap();
    }
    let store = SqliteStore::open(&layout.db_path()).unwrap().with_layout(layout.clone());
    #[cfg(feature = "local-embeddings")]
    let store = store.with_embedder(Arc::new(acta_store::embeddings::FastEmbedder::new(shared_fastembed_dir())));
    let native = acta_desktop_lib::native::start(&layout).await;
    assert!(native.error.is_none(), "sidecar: {:?}", native.error);
    let native = patch(native);
    let events = Arc::new(Events::default());
    let core = AppCore::start(CoreDeps {
        layout: layout.clone(),
        clock: Arc::new(SystemClock),
        store: Arc::new(store),
        secrets: Arc::new(InMemorySecretStore::default()),
        factory: None,
        local_llm: None,
        native,
        emitter: events.clone(),
        supervisor_config: SupervisorConfig::default(),
    })
    .await
    .unwrap();
    // El claude del usuario tal y como lo deja "Detectar" (alias con CLAUDE_CONFIG_DIR).
    let detected = core.llm_cli_detect().await.unwrap();
    let mut s = support::configured_settings();
    for p in &mut s.llm.providers {
        if let ProviderKind::ClaudeCli { binary_path, extra_env, .. } = &mut p.kind
            && let Some(path) = &detected.claude
        {
            *binary_path = PathBuf::from(path);
            *extra_env = detected.claude_env.clone().into_iter().collect();
        }
    }
    core.settings_update(s).await.unwrap();
    Rig { layout, core, events }
}

/// Caché de fastembed compartida entre pruebas (470 MB, no se descarga cada vez).
#[cfg(feature = "local-embeddings")]
fn shared_fastembed_dir() -> PathBuf {
    std::env::temp_dir().join("acta-e2e-fastembed")
}

async fn wait_status(core: &AppCore, id: MeetingId, timeout: Duration) -> Meeting {
    let start = Instant::now();
    loop {
        if let Ok(d) = core.meeting_get(id).await
            && matches!(d.meeting.status, MeetingStatus::Ready | MeetingStatus::Failed)
        {
            return d.meeting;
        }
        assert!(start.elapsed() < timeout, "la reunión {id} no terminó en {timeout:?}");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Tras `meeting_reprocess` la reunión sigue en Ready/Failed hasta que la cola
/// la coge: espera a verla en `Processing` y luego a que termine.
async fn wait_reprocessed(core: &AppCore, id: MeetingId, timeout: Duration) -> Meeting {
    let start = Instant::now();
    loop {
        let m = core.meeting_get(id).await.unwrap().meeting;
        if m.status == MeetingStatus::Processing {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(30), "la cola no cogió la reunión {id}: {:?}", m.status);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    wait_status(core, id, timeout).await
}

/// Deja una reunión "a medias" como tras un kill -9: manifiesto en
/// `recording`, sin segmentos cerrados y un `.wav.tmp` con la cabecera sin
/// actualizar (tamaño 0) por pista.
fn plant_crashed_session(layout: &DataLayout, id: MeetingId, mic: &Path, system: &Path) {
    let dir = layout.meeting_dir(id);
    let manifest = serde_json::json!({
        "version": 1, "meetingId": id, "status": "recording",
        "createdAt": 1_794_840_000_000_i64, "updatedAt": 1_794_840_000_000_i64,
        "app": null, "format": {"sampleRate": 16000, "channels": 1}, "segmentSeconds": 30,
        "spans": [{"wallStart": 1_794_840_000_000_i64, "wallEnd": null, "mediaStart": 0}],
        "tracks": [{"track": "mic", "segments": []}, {"track": "system", "segments": []}],
        "stopReason": null
    });
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(layout.manifest_path(id), serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    for (track, src) in [(Track::Mic, mic), (Track::System, system)] {
        std::fs::create_dir_all(layout.track_dir(id, track)).unwrap();
        let mut bytes = std::fs::read(src).unwrap();
        bytes[4..8].copy_from_slice(&0u32.to_le_bytes());
        bytes[40..44].copy_from_slice(&0u32.to_le_bytes());
        std::fs::write(layout.segment_tmp_path(id, track, 0), bytes).unwrap();
    }
}

fn meeting_fixture() -> Option<(PathBuf, PathBuf)> {
    let dir = PathBuf::from(std::env::var("ACTA_E2E_MEETING").ok()?);
    Some((dir.join("mic.wav"), dir.join("system.wav")))
}

// ---------------------------------------------------------------------------
// 1. Comandos de diagnóstico y ajustes con el sidecar real
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "real: sidecar + claude + Llavero (ACTA_E2E=1)"]
async fn e2e_settings_providers_models_permissions_and_keychain() {
    if !enabled() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let r = rig(tmp.path()).await;

    let info = r.core.app_info();
    eprintln!("app_info: {info:?}");
    assert!(info.sidecar_version.is_some() && info.macos_version.is_some());

    let d = r.core.llm_cli_detect().await.unwrap();
    eprintln!("llm_cli_detect: {d:?}");
    assert_eq!(d.claude.as_deref(), Some("/opt/homebrew/bin/claude"));
    // Solo en un Mac cuyo `claude` es un alias con `CLAUDE_CONFIG_DIR`: tiene que llegar expandida.
    assert!(d.claude_env.get("CLAUDE_CONFIG_DIR").is_none_or(|v| v.starts_with('/')));

    let t = r.core.llm_provider_test("claude-cli".into()).await.unwrap();
    eprintln!("llm_provider_test claude: {t:?}");
    assert!(t.ok, "{}", t.message);
    let t = r.core.llm_provider_test("codex-cli".into()).await.unwrap();
    eprintln!("llm_provider_test codex: {t:?}");

    // Ajustes: guardar → leer → sobrevive a un reinicio del core.
    let mut s = r.core.settings_get().await.unwrap();
    s.auto_stop.max_duration_ms = 3_600_000;
    s.detection.denylist.push("com.example.dictado".into());
    r.core.settings_update(s.clone()).await.unwrap();
    assert_eq!(r.core.settings_get().await.unwrap(), s);
    r.core.shutdown().await;
    drop(r);
    let r = rig(tmp.path()).await;
    let reloaded = r.core.settings_get().await.unwrap();
    assert_eq!(reloaded.auto_stop.max_duration_ms, 3_600_000);
    assert!(reloaded.detection.denylist.contains(&"com.example.dictado".to_string()));

    let models = r.core.models_status().await.unwrap();
    eprintln!("models_status: {models:?}");
    assert!(models.iter().any(|m| m.name == "asr" && m.downloaded));
    // Descarga real de un modelo que falte (el VAD pesa ~2 MB).
    for missing in models.iter().filter(|m| !m.downloaded) {
        r.core.models_download(missing.name.clone()).await.unwrap();
        let start = Instant::now();
        let last = loop {
            let ev = r.events.0.lock().unwrap().iter().rev().find(|(n, _)| n == "model:download").map(|(_, v)| v.clone());
            if let Some(v) = ev
                && (v["done"] == true || !v["error"].is_null())
            {
                break v;
            }
            assert!(start.elapsed() < Duration::from_secs(300), "la descarga de {} no terminó", missing.name);
            tokio::time::sleep(Duration::from_millis(300)).await;
        };
        eprintln!("descarga de {}: {last}", missing.name);
        assert!(last["error"].is_null(), "descarga de {}: {last}", missing.name);
        let after = r.core.models_status().await.unwrap();
        assert!(after.iter().any(|m| m.name == missing.name && m.downloaded), "{after:?}");
    }
    let inputs = r.core.audio_inputs_list().await.unwrap();
    eprintln!("audio_inputs_list: {:?}", inputs.iter().map(|i| (&i.name, i.is_default)).collect::<Vec<_>>());
    assert!(inputs.iter().any(|i| i.is_default));
    let perms = r.core.permissions_status().await.unwrap();
    eprintln!("permissions_status: {perms:?}");
    assert!(!perms.is_empty());
    assert!(r.core.health_get().await.unwrap().is_empty());

    // Llavero real con un servicio de pruebas (nunca el de acta).
    let kc = KeychainSecretStore::new();
    let sref = acta_core::llm::SecretRef { service: "acta-e2e-test".into(), account: "provider:dummy".into() };
    kc.set(&sref, "sk-dummy-123").unwrap();
    assert_eq!(kc.get(&sref).unwrap().as_deref(), Some("sk-dummy-123"));
    kc.delete(&sref).unwrap();
    assert_eq!(kc.get(&sref).unwrap(), None);
    r.core.shutdown().await;
}

// ---------------------------------------------------------------------------
// 2. Grabación real (mic + tap del sistema) → parada → procesado
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "real: graba audio (ACTA_E2E=1)"]
async fn e2e_live_recording_with_system_tap() {
    if !enabled() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let r = rig(tmp.path()).await;
    // Igual que `forward_events` en lib.rs: SupervisorEvent → nombre Tauri.
    let mut rx = r.core.supervisor.subscribe();
    let seen = Arc::new(Mutex::new(std::collections::BTreeSet::new()));
    let seen2 = seen.clone();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    seen2.lock().unwrap().insert(acta_desktop_lib::events::to_tauri(&ev).0.to_string());
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
    let snap = r.core.session_start(None).await.unwrap();
    let SessionState::Recording(rec) = &snap.state else { panic!("no graba: {:?}", snap.state) };
    let id = rec.meeting_id;
    for _ in 0..4 {
        let _ = tokio::process::Command::new("afplay")
            .arg("/System/Library/Sounds/Glass.aiff")
            .kill_on_drop(true)
            .status()
            .await;
    }
    // Voz por la salida del sistema: con AirPods el micro abierto los pasa a
    // HFP y el tap entrega a 24 kHz aunque anunció 48 kHz (salía ininteligible).
    let _ = tokio::process::Command::new("say")
        .args(["-v", "Mónica", "Vamos a revisar el presupuesto del proyecto y la fecha de entrega de la próxima semana."])
        .kill_on_drop(true)
        .status()
        .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let health = r.core.health_get().await.unwrap();
    eprintln!("health tras 12 s: {health:?}");
    let snap = r.core.session_stop().await.unwrap();
    eprintln!("tras parar: {:?}", snap.state);
    let m = wait_status(&r.core, id, Duration::from_secs(300)).await;
    eprintln!("estado final: {:?} error={:?}", m.status, m.error);
    let detail = r.core.meeting_get(id).await.unwrap();
    eprintln!("audio: {:?}", detail.audio);
    let sys = std::fs::read(r.layout.meeting_dir(id).join("audio/system.wav")).unwrap();
    let nonzero = sys[44..].as_chunks::<2>().0.iter().filter(|c| **c != [0, 0]).count();
    eprintln!("system.wav: {} muestras, {nonzero} no nulas", (sys.len() - 44) / 2);
    assert!(nonzero > 1_000, "el tap del sistema no grabó el sonido");
    let said: Vec<String> = detail.transcript.utterances.iter().map(|u| u.text.to_lowercase()).collect();
    eprintln!("transcript: {said:?}");
    assert!(said.iter().any(|t| t.contains("presupuesto")), "la voz del sistema no se entiende: {said:?}");
    let names = seen.lock().unwrap().clone();
    eprintln!("eventos: {names:?}");
    for ev in ["session:state", "session:levels", "processing:progress", "meeting:updated"] {
        assert!(names.contains(ev), "falta el evento {ev}");
    }
    // `session:health` solo se emite si hay algún aviso (micro a ceros con la
    // tapa cerrada o TCC denegado); con el micro sano la pista tiene señal.
    let mic = std::fs::read(r.layout.meeting_dir(id).join("audio/mic.wav")).unwrap();
    let mic_nonzero = mic[44..].as_chunks::<2>().0.iter().filter(|c| **c != [0, 0]).count();
    eprintln!("mic.wav: {} muestras, {mic_nonzero} no nulas", (mic.len() - 44) / 2);
    if health.is_empty() {
        assert!(mic_nonzero > 1_000, "micro sin avisos pero a ceros");
    } else {
        assert!(names.contains("session:health"), "hay avisos {health:?} pero no se emitió session:health");
    }
    r.core.shutdown().await;
}

// ---------------------------------------------------------------------------
// 3. Pipeline completo desde una sesión "crasheada" de ~50 s y dos pistas
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "real: ASR + diarización + claude (ACTA_E2E=1, ACTA_E2E_MEETING)"]
async fn e2e_full_pipeline_search_chat_and_groups() {
    if !enabled() {
        return;
    }
    let Some((mic, system)) = meeting_fixture() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let layout = DataLayout::new(tmp.path());
    let ids: Vec<MeetingId> = (1..=3).map(|_| MeetingId::new()).collect();
    for id in &ids {
        plant_crashed_session(&layout, *id, &mic, &system);
    }
    let started = Instant::now();
    let r = rig(tmp.path()).await;
    for id in &ids {
        let m = wait_status(&r.core, *id, Duration::from_secs(900)).await;
        eprintln!("{id}: {:?} error={:?} ({} s)", m.status, m.error, started.elapsed().as_secs());
        assert_eq!(m.status, MeetingStatus::Ready, "{:?}", m.error);
        assert!(m.recovered);
    }
    let id = ids[0];
    let d = r.core.meeting_get(id).await.unwrap();
    let speakers: Vec<String> = d.transcript.speakers.iter().map(|s| s.display_name.clone().unwrap_or(s.label.clone())).collect();
    eprintln!("hablantes: {speakers:?}");
    for u in &d.transcript.utterances {
        let who = d.transcript.speakers.iter().find(|s| s.id == u.speaker_id).map(|s| s.label.clone()).unwrap_or_default();
        eprintln!("  [{:>6}] {who}: {}", u.start.0, u.text);
    }
    assert!(d.transcript.speakers.iter().any(|s| s.is_me));
    assert!(d.transcript.speakers.iter().filter(|s| !s.is_me).count() >= 2, "diarización: {speakers:?}");
    let minutes = d.minutes.expect("acta");
    eprintln!("título: {}\nresumen:\n{}", d.meeting.title, minutes.summary_md);
    eprintln!("decisiones: {:?}", minutes.decisions.iter().map(|x| &x.text).collect::<Vec<_>>());
    eprintln!("tareas: {:?}", minutes.action_items.iter().map(|a| (&a.text, &a.owner_name, a.is_mine)).collect::<Vec<_>>());
    assert!(!minutes.action_items.is_empty());
    assert!(minutes.action_items.iter().any(|a| a.is_mine), "la tarea del presupuesto es mía");
    let esc = d.escaleta.expect("escaleta");
    eprintln!("escaleta: {:?}", esc.temas.iter().map(|t| &t.titulo).collect::<Vec<_>>());
    assert!(!esc.temas.is_empty());
    eprintln!("timeline: {} eventos", d.timeline.len());
    assert!(!d.timeline.is_empty());

    // Búsqueda (FTS + vectores) y chat sobre la reunión.
    let hits = r
        .core
        .search(SearchQuery { text: "presupuesto finanzas".into(), scope: SearchScope::All, limit: 10 })
        .await
        .unwrap();
    eprintln!("búsqueda: {:?}", hits.iter().map(|h| (&h.snippet, h.score)).collect::<Vec<_>>());
    assert!(!hits.is_empty());
    let sem = r
        .core
        .search(SearchQuery { text: "dinero para el departamento financiero".into(), scope: SearchScope::All, limit: 5 })
        .await
        .unwrap();
    eprintln!("búsqueda semántica: {} resultados", sem.len());

    let thread = r.core.chat_thread_create(ChatScope::Meeting { meeting_id: id }).await.unwrap();
    let res = r.core.chat_send(thread.id, "¿Quién prepara la demo y para qué cliente?".into(), None, None).await.unwrap();
    let start = Instant::now();
    let answer = loop {
        let msgs = r.core.chat_messages_list(thread.id).await.unwrap();
        if let Some(m) = msgs.iter().find(|m| m.id == res.assistant_message_id) {
            break m.clone();
        }
        assert!(start.elapsed() < Duration::from_secs(300), "el chat no respondió");
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    eprintln!("chat: {} | citas={:?}", answer.content, answer.citations.len());
    assert_eq!(answer.role, ChatRole::Assistant);
    assert!(answer.content.to_lowercase().contains("barcelona"));
    assert!(r.events.names().iter().any(|n| n == "chat:delta"));

    // Grupos sugeridos (3 reuniones casi iguales) con nombre del LLM.
    let sugg = r.core.group_suggestions_refresh().await.unwrap();
    eprintln!("sugerencias: {:?}", sugg.iter().map(|s| (&s.name, s.meeting_ids.len(), s.cohesion)).collect::<Vec<_>>());
    if cfg!(feature = "local-embeddings") {
        assert!(!sugg.is_empty(), "3 reuniones iguales deberían agruparse");
        assert!(sugg[0].name.as_deref().is_some_and(|n| !n.is_empty()));
        let g = r.core.group_suggestion_accept(sugg[0].id, None).await.unwrap();
        assert!(r.core.groups_list().await.unwrap().iter().any(|x| x.id == g.id));
    }

    // Renombrar con "recordar" crea la huella de voz y se reutiliza.
    let other = d.transcript.speakers.iter().find(|s| !s.is_me).unwrap();
    r.core.speaker_rename(id, other.id, "Ana García".into(), true).await.unwrap();
    let vps = r.core.voiceprints_list().await.unwrap();
    eprintln!("huellas: {vps:?}");
    assert_eq!(vps.len(), 1);
    r.core.meeting_reprocess(acta_core::processing::ReprocessRequest { meeting_id: ids[1], steps: vec![], llm: None }).await.unwrap();
    let m = wait_reprocessed(&r.core, ids[1], Duration::from_secs(600)).await;
    assert_eq!(m.status, MeetingStatus::Ready, "{:?}", m.error);
    let d1 = r.core.meeting_get(ids[1]).await.unwrap();
    let names: Vec<_> = d1.transcript.speakers.iter().filter_map(|s| s.display_name.clone()).collect();
    eprintln!("tras reprocesar con huella: {names:?}");
    r.core.shutdown().await;
}

// ---------------------------------------------------------------------------
// 4. kill -9 a mitad de grabación → recuperación al volver
// ---------------------------------------------------------------------------

/// Hijo: graba hasta que lo matan. Solo corre si lo lanza el padre.
#[tokio::test]
#[ignore = "lo lanza e2e_kill9_during_recording_is_recovered"]
async fn e2e_child_records_until_killed() {
    let Ok(dir) = std::env::var("ACTA_E2E_CHILD_DIR") else { return };
    let r = rig(Path::new(&dir)).await;
    let snap = r.core.session_start(None).await.unwrap();
    assert!(matches!(snap.state, SessionState::Recording(_)));
    std::fs::write(Path::new(&dir).join("child.ready"), b"1").unwrap();
    tokio::time::sleep(Duration::from_secs(600)).await;
}

#[tokio::test]
#[ignore = "real: kill -9 (ACTA_E2E=1)"]
async fn e2e_kill9_during_recording_is_recovered() {
    if !enabled() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let exe = std::env::current_exe().unwrap();
    let mut child = tokio::process::Command::new(&exe)
        .args(["--ignored", "--exact", "e2e_child_records_until_killed", "--nocapture"])
        .env("ACTA_E2E_CHILD_DIR", tmp.path())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let start = Instant::now();
    while !tmp.path().join("child.ready").exists() {
        assert!(start.elapsed() < Duration::from_secs(60), "el hijo no empezó a grabar");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    for _ in 0..3 {
        let _ = tokio::process::Command::new("afplay").arg("/System/Library/Sounds/Glass.aiff").kill_on_drop(true).status().await;
    }
    tokio::time::sleep(Duration::from_secs(32)).await; // > 1 segmento de 30 s
    let child_pid = child.id().unwrap();
    child.start_kill().unwrap(); // SIGKILL
    let _ = child.wait().await;
    // El sidecar del hijo debe morir solo (EOF en stdin / getppid cambia).
    let killed_at = Instant::now();
    let orphan = loop {
        let out = tokio::process::Command::new("pgrep").args(["-P", "1", "-f", "acta-mac"]).output().await.unwrap();
        let pids = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if pids.is_empty() || killed_at.elapsed() > Duration::from_secs(3) {
            break pids;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    eprintln!("hijo {child_pid} muerto; sidecars huérfanos tras {:?}: {orphan:?}", killed_at.elapsed());

    let r = rig(tmp.path()).await;
    let page = r.core.meetings_list(MeetingQuery::default()).await.unwrap();
    assert_eq!(page.items.len(), 1, "una reunión recuperada");
    let id = page.items[0].id;
    let m = wait_status(&r.core, id, Duration::from_secs(600)).await;
    eprintln!("recuperada: {:?} recovered={} duración={:?} error={:?}", m.status, m.recovered, m.duration, m.error);
    assert!(m.recovered);
    assert!(m.duration.0 >= 30_000, "se perdió audio: {:?}", m.duration);
    assert!(orphan.is_empty(), "sidecar huérfano: {orphan}");
    r.core.shutdown().await;
}

// ---------------------------------------------------------------------------
// 5. "Reintentar" sobre una COPIA de los datos reales del usuario
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "real: ACTA_E2E=1 ACTA_E2E_DATA_COPY=<copia de Application Support/acta>"]
async fn e2e_retry_failed_meetings_on_a_data_copy() {
    use acta_core::processing::{ProcessingStep, ReprocessRequest};
    if !enabled() {
        return;
    }
    let Ok(dir) = std::env::var("ACTA_E2E_DATA_COPY") else { return };
    trace();
    let r = rig(Path::new(&dir)).await;
    let page = r.core.meetings_list(MeetingQuery::default()).await.unwrap();
    let failed: Vec<_> = page.items.iter().filter(|m| m.status == MeetingStatus::Failed).map(|m| m.id).collect();
    eprintln!("reuniones fallidas: {failed:?}");
    // Como en la app tras una grabación con transcripción en vivo: modelos ya cargados.
    if std::env::var("ACTA_E2E_WARM").is_ok() {
        let t = Instant::now();
        for name in ["asr", "diarizer"] {
            r.core.models_download(name.into()).await.unwrap();
            while !r.events.0.lock().unwrap().iter().any(|(n, p)| n == "model:download" && p["name"] == name && p["done"] == true) {
                assert!(t.elapsed() < Duration::from_secs(300), "no cargó {name}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        eprintln!("modelos cargados en {:.1} s", t.elapsed().as_secs_f32());
    }
    assert!(!failed.is_empty());
    // Los mismos pasos que manda el botón «Reintentar» de la UI.
    let steps = vec![
        ProcessingStep::Transcribing,
        ProcessingStep::Diarizing,
        ProcessingStep::NamingSpeakers,
        ProcessingStep::Summarizing,
        ProcessingStep::Indexing,
        ProcessingStep::PruningCaptures,
    ];
    for id in &failed {
        r.core.meeting_reprocess(ReprocessRequest { meeting_id: *id, steps: steps.clone(), llm: None }).await.unwrap();
    }
    for id in &failed {
        let t0 = Instant::now();
        let m = wait_reprocessed(&r.core, *id, Duration::from_secs(600)).await;
        eprintln!("{id}: {:.1} s en total", t0.elapsed().as_secs_f32());
        let d = r.core.meeting_get(*id).await.unwrap();
        eprintln!("{id}: {:?} «{}» error={:?}\n{}", m.status, m.title, m.error, d.minutes.as_ref().map(|x| x.summary_md.as_str()).unwrap_or(""));
        assert_eq!(m.status, MeetingStatus::Ready, "{:?}", m.error);
    }
    r.core.shutdown().await;
}

// ---------------------------------------------------------------------------
// 6. Detección simulada → «¿Grabar?» → grabación real → colgar → «¿Parar?»
//    sin responder → autoparada por silencio
// ---------------------------------------------------------------------------

/// Detector guionizado: lo único falso de la prueba (el audio, el sidecar y
/// el procesado son reales).
#[derive(Default)]
struct ScriptedDetector(Mutex<Option<futures::channel::mpsc::UnboundedSender<acta_core::detection::DetectionEvent>>>);
impl ScriptedDetector {
    fn push(&self, ev: acta_core::detection::DetectionEvent) {
        self.0.lock().unwrap().as_ref().expect("detector arrancado").unbounded_send(ev).unwrap();
    }
}
#[async_trait::async_trait]
impl acta_core::detection::MeetingDetector for ScriptedDetector {
    async fn start(
        &self,
        _config: acta_core::detection::DetectionConfig,
    ) -> acta_core::PortResult<acta_core::EventStream<acta_core::detection::DetectionEvent>> {
        let (tx, rx) = futures::channel::mpsc::unbounded();
        *self.0.lock().unwrap() = Some(tx);
        Ok(Box::pin(rx))
    }
    async fn stop(&self) -> acta_core::PortResult<()> {
        self.0.lock().unwrap().take();
        Ok(())
    }
}

async fn wait_state(core: &AppCore, what: &str, timeout: Duration, pred: impl Fn(&SessionState) -> bool) -> SessionState {
    let start = Instant::now();
    loop {
        let s = core.session_get_state().await.unwrap().state;
        if pred(&s) {
            return s;
        }
        assert!(start.elapsed() < timeout, "no se llegó a «{what}» en {timeout:?}: {s:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
#[ignore = "real: graba audio con detección simulada (ACTA_E2E=1)"]
async fn e2e_simulated_detection_prompts_and_auto_stops() {
    use acta_core::detection::{Confidence, DetectionEvent, MeetingApp, Platform};
    use acta_core::recording::{PromptAnswer, StopReason};
    use acta_core::time::Clock;
    if !enabled() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let detector = Arc::new(ScriptedDetector::default());
    let d2 = detector.clone();
    let r = rig_with(tmp.path(), move |mut n| {
        n.detector = d2;
        n
    })
    .await;
    let mut s = r.core.settings_get().await.unwrap();
    assert!(s.detection.enabled, "la detección viene activada de serie");
    s.auto_stop.silence_threshold_dbfs = -20.0; // el ruido de fondo del micro cuenta como silencio
    s.auto_stop.auto_stop_after_unanswered_ms = 4_000;
    r.core.settings_update(s).await.unwrap();

    // Sin PIDs de audio: el tap espera a que la app aparezca (aquí no hay llamada real).
    let app = MeetingApp {
        platform: Platform::Teams,
        bundle_id: "com.microsoft.teams2".into(),
        pid: i32::try_from(std::process::id()).unwrap(),
        audio_pids: vec![],
        name: "Microsoft Teams".into(),
        title: Some("Sincro semanal | Microsoft Teams".into()),
        url: None,
    };
    let now = || SystemClock.now();
    detector.push(DetectionEvent::MeetingStarted { app: app.clone(), confidence: Confidence::High, at: now() });
    let SessionState::Detected { prompt, .. } =
        wait_state(&r.core, "detected", Duration::from_secs(5), |s| matches!(s, SessionState::Detected { .. })).await
    else {
        unreachable!()
    };
    eprintln!("pregunta de inicio: {prompt:?}");

    let snap = r.core.prompt_answer(prompt.id, PromptAnswer::Accept).await.unwrap();
    let SessionState::Recording(rec) = &snap.state else { panic!("no graba: {:?}", snap.state) };
    let id = rec.meeting_id;
    assert_eq!(r.core.meeting_get(id).await.unwrap().meeting.title, "Sincro semanal");
    tokio::time::sleep(Duration::from_secs(4)).await;

    detector.push(DetectionEvent::MeetingEnded { app, at: now() });
    let s = wait_state(&r.core, "¿Parar?", Duration::from_secs(5), |s| {
        matches!(s, SessionState::Recording(r) if r.stop_prompt.is_some())
    })
    .await;
    eprintln!("pregunta de parada: {s:?}");
    // Nadie responde y hay silencio: se para sola.
    wait_state(&r.core, "autoparada", Duration::from_secs(30), |s| !matches!(s, SessionState::Recording(_))).await;
    let manifest = acta_audio::writer::read_manifest(&r.layout, id).unwrap();
    eprintln!("motivo de parada: {:?}", manifest.stop_reason);
    assert_eq!(manifest.stop_reason, Some(StopReason::AutoSilence));
    let m = wait_status(&r.core, id, Duration::from_secs(300)).await;
    eprintln!("estado final: {:?} error={:?}", m.status, m.error);
    r.core.shutdown().await;
}

// ---------------------------------------------------------------------------
// 7. v3 sobre una COPIA de los datos reales: favoritas, limpieza y el modelo
//    local integrado (descarga real de llama.cpp + Qwen3.5 2B, 1,3 GB)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "real: ACTA_E2E=1 ACTA_E2E_DATA_COPY=<copia de Application Support/acta> (descarga 1,3 GB)"]
async fn e2e_v3_favourites_cleanup_and_built_in_model_on_a_data_copy() {
    use acta_core::library::cleanup::DAY_MS;
    use acta_core::llm::ProviderConfig;
    use acta_core::llm::local::{BUILTIN_PROVIDER_ID, RuntimeSource};
    use acta_core::processing::{ProcessingStep, ReprocessRequest};
    if !enabled() {
        return;
    }
    let Ok(dir) = std::env::var("ACTA_E2E_DATA_COPY") else { return };
    assert!(!dir.contains("Application Support/acta"), "siempre sobre una copia");
    let r = rig(Path::new(&dir)).await;

    // --- Migraciones 3 y 4 sobre la base real: todo sigue ahí, sin estrellas.
    let all = r.core.meetings_list(MeetingQuery::default()).await.unwrap().items;
    eprintln!("reuniones en la copia: {}", all.len());
    assert!(all.len() >= 2 && all.iter().all(|m| !m.favourite));
    let with_text: Vec<&MeetingSummary> = all.iter().filter(|m| m.status == MeetingStatus::Ready && m.duration.0 > 10_000).collect();
    assert!(with_text.len() >= 2, "hacen falta dos reuniones con contenido");
    let (starred, plain) = (with_text[0].id, with_text[1].id);

    // --- Favoritas: estrella, filtro y búsqueda acotada.
    assert!(r.core.meeting_set_favourite(starred, true).await.unwrap().favourite);
    let favs = r.core.meetings_list(MeetingQuery { favourites_only: true, ..MeetingQuery::default() }).await.unwrap().items;
    assert_eq!(favs.iter().map(|m| m.id).collect::<Vec<_>>(), vec![starred]);
    let everywhere = r.core.search(SearchQuery { text: "versión".into(), scope: SearchScope::All, limit: 20 }).await.unwrap();
    let in_favs = r.core.search(SearchQuery { text: "versión".into(), scope: SearchScope::Favourites, limit: 20 }).await.unwrap();
    eprintln!("búsqueda «versión»: {} en todo, {} en favoritas", everywhere.len(), in_favs.len());
    assert!(in_favs.iter().all(|h| h.meeting_id == starred));
    assert!(in_favs.len() <= everywhere.len());

    // --- Limpieza: nada que sugerir (son de hoy y se acaban de buscar)…
    assert!(r.core.cleanup_suggestions().await.unwrap().candidates.is_empty());
    // …hasta que dos envejecen 200 días y dejan de constar como usadas (solo en la copia).
    let old = acta_core::time::Clock::now(&SystemClock).0 - 200 * DAY_MS;
    r.core
        .store
        .with_conn(move |conn| {
            conn.execute(
                "UPDATE meetings SET started_at = ?1, last_opened_at = NULL, last_used_at = NULL WHERE id IN (?2, ?3)",
                (old, starred.to_string(), plain.to_string()),
            )
            .map_err(|e| acta_core::PortError::Io(e.to_string()))?;
            Ok(())
        })
        .await
        .unwrap();
    let s = r.core.cleanup_suggestions().await.unwrap();
    eprintln!("sugerencias: {:?}", s.candidates.iter().map(|c| (&c.title, c.bytes, &c.reasons)).collect::<Vec<_>>());
    assert_eq!(s.candidates.iter().map(|c| c.meeting_id).collect::<Vec<_>>(), vec![plain], "la favorita nunca se sugiere");
    assert!(s.total_bytes > 0);
    let plain_dir = r.layout.meeting_dir(plain);
    assert!(plain_dir.exists());
    let res = r.core.cleanup_delete(vec![plain, starred]).await.unwrap();
    eprintln!("borradas {:?}, saltadas {:?}, liberados {} bytes", res.deleted, res.skipped, res.freed_bytes);
    assert_eq!((res.deleted.clone(), res.skipped.clone()), (vec![plain], vec![starred]));
    assert_eq!(res.freed_bytes, s.total_bytes);
    assert!(!plain_dir.exists() && r.layout.meeting_dir(starred).exists());
    assert!(r.core.search(SearchQuery { text: "versión".into(), scope: SearchScope::All, limit: 20 }).await.unwrap().iter().all(|h| h.meeting_id != plain));

    // --- Modelo local integrado: este Mac, descarga real y «Probar».
    let status = r.core.local_llm_status().await.unwrap();
    eprintln!(
        "Mac: {} · {} GB · libres {:?} · recomendado {:?}",
        status.hardware.chip,
        status.hardware.ram_gb(),
        status.hardware.free_disk_bytes.map(|b| b / 1_000_000_000),
        status.recommended
    );
    for m in &status.models {
        eprintln!("  {} fit={:?} ≈{} tokens/s descargado={}", m.model.name, m.fit, m.expected_tokens_per_s, m.downloaded);
    }
    let tiny = status.models[0].model.id.clone();
    let started = Instant::now();
    r.core.local_llm_download(tiny.clone()).await.unwrap();
    loop {
        let done = r.events.0.lock().unwrap().iter().find(|(n, p)| n == "localmodel:download" && p["done"] == true).map(|(_, p)| p.clone());
        if let Some(done) = done {
            assert!(done["error"].is_null(), "{done}");
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(1_200), "la descarga no terminó");
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    eprintln!("descarga del modelo: {:.1} s", started.elapsed().as_secs_f64());
    let status = r.core.local_llm_status().await.unwrap();
    assert!(status.models[0].downloaded);
    eprintln!("llama.cpp: {:?} en {:?}", status.runtime.source, status.runtime.path);
    assert!(matches!(status.runtime.source, Some(RuntimeSource::Downloaded | RuntimeSource::Env | RuntimeSource::Bundled)));
    let t = r.core.local_llm_test(tiny.clone()).await.unwrap();
    eprintln!("probar: ok={} «{}» {:?} ms", t.ok, t.message, t.latency_ms);
    assert!(t.ok, "{}", t.message);

    // --- El acta de una reunión REAL de la copia, escrita por el modelo integrado.
    let mut settings = r.core.settings_get().await.unwrap();
    settings.llm.providers.push(ProviderConfig {
        id: BUILTIN_PROVIDER_ID.into(),
        label: "Modelo local integrado".into(),
        enabled: true,
        kind: ProviderKind::BuiltIn { model_id: tiny.clone() },
        timeout_s: 600,
    });
    settings.llm.minutes_provider = Some(BUILTIN_PROVIDER_ID.into());
    r.core.settings_update(settings).await.unwrap();
    let t0 = Instant::now();
    r.core
        .meeting_reprocess(ReprocessRequest { meeting_id: starred, steps: vec![ProcessingStep::Summarizing, ProcessingStep::Indexing], llm: None })
        .await
        .unwrap();
    let m = wait_reprocessed(&r.core, starred, Duration::from_secs(600)).await;
    let d = r.core.meeting_get(starred).await.unwrap();
    let minutes = d.minutes.expect("acta");
    eprintln!("acta local en {:.1} s: {:?} «{}» proveedor={} modelo={:?}", t0.elapsed().as_secs_f64(), m.status, m.title, minutes.provider, minutes.model);
    eprintln!("{}", minutes.summary_md);
    for a in &minutes.action_items {
        eprintln!("- tarea [{}] {}", if a.is_mine { "mía" } else { "otro" }, a.text);
    }
    assert_eq!(m.status, MeetingStatus::Ready, "{:?}", m.error);
    assert_eq!(minutes.provider.0, BUILTIN_PROVIDER_ID);
    assert!(!minutes.summary_md.trim().is_empty());
    for banned in ["(yo)", "transcripción no", "único hablante", "Grabación muy breve"] {
        assert!(!minutes.summary_md.contains(banned), "metacomentario «{banned}»");
    }
    assert_eq!(r.core.local_llm_status().await.unwrap().running.as_deref(), Some(tiny.as_str()));

    // --- Borrar el modelo para el servidor y libera el disco.
    r.core.local_llm_delete(tiny).await.unwrap();
    let status = r.core.local_llm_status().await.unwrap();
    assert!(!status.models[0].downloaded && status.running.is_none());
    r.core.shutdown().await;
}

// ---------------------------------------------------------------------------
// Borrar las pruebas del 16-nov de los datos REALES de Edu (lo pidió él), por
// el mismo camino que el botón «Borrar» (filas, índice y carpeta). Con la app
// CERRADA y una copia hecha antes (`acta-backup-<fecha>`).
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "BORRA datos reales: ACTA_E2E=1 ACTA_DELETE_TEST_DATA=<carpeta de datos>"]
async fn e2e_delete_the_smoke_test_meetings_and_group() {
    use acta_desktop_lib::native::NativePorts;
    if !enabled() {
        return;
    }
    let Ok(dir) = std::env::var("ACTA_DELETE_TEST_DATA") else { return };
    let layout = DataLayout::new(&dir);
    let store = Arc::new(SqliteStore::open(&layout.db_path()).unwrap().with_layout(layout.clone()));
    let core = AppCore::start(CoreDeps {
        layout: layout.clone(),
        clock: Arc::new(SystemClock),
        store,
        secrets: Arc::new(InMemorySecretStore::default()),
        factory: None,
        local_llm: None,
        native: NativePorts::unavailable("borrado"),
        emitter: Arc::new(Events::default()),
        supervisor_config: SupervisorConfig::default(),
    })
    .await
    .unwrap();
    // 16-nov-2026 en hora de Madrid (UTC+1): [15-nov 23:00Z, 16-nov 23:00Z).
    let (from, to) = (1_794_783_600_000_i64, 1_794_870_000_000_i64);
    let test_titles = ["Grabación", "Prueba de humo", "Segunda prueba de humo", "Grabación breve sin contenido"];
    let all = core.meetings_list(MeetingQuery { limit: Some(500), ..Default::default() }).await.unwrap().items;
    for m in &all {
        let day16 = (from..to).contains(&m.started_at.0);
        let is_test = test_titles.iter().any(|t| m.title.starts_with(t));
        // Grabaciones de prueba de la propia sesión de pruebas (por id).
        let extra = std::env::var("ACTA_DELETE_IDS").unwrap_or_default().split(',').any(|x| x.trim() == m.id.to_string());
        if (day16 && is_test) || extra {
            eprintln!("borro {} «{}»", m.id, m.title);
            core.meeting_delete(m.id).await.unwrap();
            assert!(!layout.meeting_dir(m.id).exists());
        } else {
            eprintln!("mantengo {} «{}»", m.id, m.title);
        }
    }
    for g in core.groups_list().await.unwrap() {
        if g.name.trim() == "test" {
            eprintln!("borro el grupo {} «{}»", g.id, g.name);
            core.group_delete(g.id).await.unwrap();
        }
    }
    // «Transcripción en vivo» pasa a venir activada: sus ajustes guardados
    // tenían el `false` del valor por defecto antiguo.
    let mut settings = core.settings_get().await.unwrap();
    if !settings.transcription.live_preview {
        settings.transcription.live_preview = true;
        core.settings_update(settings).await.unwrap();
        eprintln!("transcripción en vivo activada");
    }
    let left = core.meetings_list(MeetingQuery { limit: Some(500), ..Default::default() }).await.unwrap().items;
    eprintln!("quedan {} reuniones: {:?}", left.len(), left.iter().map(|m| &m.title).collect::<Vec<_>>());
    assert!(left.iter().all(|m| !(from..to).contains(&m.started_at.0)));
    assert!(left.iter().any(|m| m.id.to_string().starts_with("01a1e8af")), "la reunión real de Teams se conserva");
    core.shutdown().await;
}
