#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Regresión: «si el chat da una respuesta larga, de repente desaparece».
//! La interfaz, al recibir el último `chat:delta` (`done`), suelta el texto en
//! streaming y relee los mensajes guardados; si la respuesta aún no se ha
//! guardado en ese instante, la burbuja desaparece. Por eso la respuesta tiene
//! que estar YA en la base de datos cuando sale el delta final.

use acta_audio::supervisor::SupervisorConfig;
use acta_core::chat::{ChatScope, ChatStore};
use acta_core::detection::Platform;
use acta_core::ids::*;
use acta_core::layout::DataLayout;
use acta_core::library::*;
use acta_core::llm::*;
use acta_core::time::{SystemClock, UnixMs};
use acta_core::{EventStream, PortResult};
use acta_desktop_lib::app::{AppCore, CoreDeps, Emit};
use acta_desktop_lib::native::NativePorts;
use acta_store::SqliteStore;
use acta_store::secrets::InMemorySecretStore;
use async_trait::async_trait;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

mod support;

/// Al salir el delta final apunta cuántos mensajes había ya guardados.
#[derive(Default)]
struct Probe {
    store: OnceLock<Arc<SqliteStore>>,
    saved_at_done: Mutex<Option<Vec<String>>>,
}
impl Emit for Probe {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        if event != "chat:delta" || payload["done"] != true {
            return;
        }
        let thread: ChatThreadId = serde_json::from_value(payload["threadId"].clone()).unwrap();
        let store = self.store.get().unwrap().clone();
        let msgs = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move { store.list_messages(thread).await.unwrap() })
        });
        *self.saved_at_done.lock().unwrap() = Some(msgs.into_iter().map(|m| m.content).collect());
    }
}

struct LongProvider(ProviderId);
#[async_trait]
impl LlmProvider for LongProvider {
    fn id(&self) -> &ProviderId {
        &self.0
    }
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities { streaming: true, native_json_schema: false, context_tokens: 100_000 }
    }
    async fn complete(&self, _r: LlmRequest) -> PortResult<LlmResponse> {
        Ok(LlmResponse { text: "ok".into(), json: None, model: None, usage: Usage::default() })
    }
    async fn stream(&self, _r: LlmRequest) -> PortResult<EventStream<PortResult<String>>> {
        let chunks: Vec<PortResult<String>> = (0..400).map(|i| Ok(format!("Párrafo {i} con bastante texto [1]. "))).collect();
        Ok(Box::pin(futures::stream::iter(chunks)))
    }
}
struct LongFactory;
impl LlmProviderFactory for LongFactory {
    fn build(&self, config: &ProviderConfig) -> PortResult<Arc<dyn LlmProvider>> {
        Ok(Arc::new(LongProvider(config.id.clone())))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_long_answer_is_already_saved_when_the_final_delta_goes_out() {
    let dir = tempfile::tempdir().unwrap();
    let layout = DataLayout::new(dir.path());
    let store = Arc::new(SqliteStore::open(&layout.db_path()).unwrap().with_layout(layout.clone()));
    let probe = Arc::new(Probe::default());
    probe.store.set(store.clone()).ok();
    let core = AppCore::start(CoreDeps {
        layout: layout.clone(),
        clock: Arc::new(SystemClock),
        store,
        secrets: Arc::new(InMemorySecretStore::default()),
        factory: Some(Arc::new(LongFactory)),
        local_llm: None,
        native: NativePorts::unavailable("tests"),
        emitter: probe.clone(),
        supervisor_config: SupervisorConfig { tick: Duration::from_millis(30), stop_drain: Duration::from_millis(200) },
    })
    .await
    .unwrap();
    core.settings_update(support::configured_settings()).await.unwrap();
    let id = MeetingId::from_u128(1);
    core.store
        .create_meeting(NewMeeting {
            id,
            title: "Daily".into(),
            platform: Platform::Teams,
            app_bundle_id: None,
            started_at: UnixMs(1_700_000_000_000),
            language: "es".into(),
        })
        .await
        .unwrap();
    let thread = core.chat_thread_create(ChatScope::Meeting { meeting_id: id }).await.unwrap();
    core.chat_send(thread.id, "Cuéntamelo todo".into(), None, None).await.unwrap();
    for _ in 0..200 {
        if probe.saved_at_done.lock().unwrap().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let saved = probe.saved_at_done.lock().unwrap().clone().expect("llegó el delta final");
    assert_eq!(saved.len(), 2, "la respuesta debe estar guardada antes del delta final: {saved:?}");
    assert!(saved[1].contains("Párrafo 399"), "se guardó entera");
}
