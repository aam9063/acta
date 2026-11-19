//! `GroupNamer` (WP3) con el esquema `acta.group_name.v1`.
//! `speakers`: `SpeakerNamer` con el esquema `acta.speaker_names.v1`.

pub mod speakers;

use crate::structured::complete_structured;
use crate::transcript::render_template;
use crate::{prompts, schemas};
use acta_core::llm::{LlmMessage, LlmProvider, LlmRequest, Role};
use acta_core::minutes::{GroupName, GroupNamer};
use acta_core::{PortError, PortResult};
use async_trait::async_trait;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupNameV1 {
    nombre: String,
    descripcion: String,
}

fn validate(v: &GroupNameV1) -> Result<(), String> {
    let words = v.nombre.split_whitespace().count();
    if words == 0 {
        return Err("`nombre` está vacío".into());
    }
    if words > 8 {
        return Err("`nombre` debe tener entre 2 y 5 palabras".into());
    }
    Ok(())
}

/// Máx. de caracteres de resumen por reunión (evita prompts enormes).
const SUMMARY_CHARS: usize = 600;

pub struct LlmGroupNamer {
    provider: Arc<dyn LlmProvider>,
    /// `Settings.transcription.language`; `auto` = el idioma de las reuniones.
    language: String,
}

impl LlmGroupNamer {
    /// El nombre del usuario no entra en el prompt (se conserva el parámetro por compatibilidad).
    pub fn new(provider: Arc<dyn LlmProvider>, _user_name: impl Into<String>) -> Self {
        Self { provider, language: acta_core::settings::language::AUTO.to_owned() }
    }

    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = language.into();
        self
    }
}

#[async_trait]
impl GroupNamer for LlmGroupNamer {
    async fn name_group(&self, meetings: &[(String, String)]) -> PortResult<GroupName> {
        if meetings.is_empty() {
            return Err(PortError::InvalidInput("no hay reuniones que nombrar".into()));
        }
        let list = meetings
            .iter()
            .enumerate()
            .map(|(i, (title, summary))| {
                let s: String = summary.chars().take(SUMMARY_CHARS).collect();
                format!("{}. {}\n{}", i + 1, title.trim(), s.trim())
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        let request = LlmRequest {
            system: Some(render_template(
                prompts::GROUP_NAME_SYSTEM,
                &[("language", &crate::transcript::output_language(&self.language, "el mismo idioma de esos títulos y resúmenes"))],
            )),
            messages: vec![LlmMessage { role: Role::User, content: list }],
            json_schema: serde_json::from_str(schemas::GROUP_NAME_V1).ok(),
            max_tokens: Some(300),
            temperature: Some(0.3),
        };
        let v: GroupNameV1 = complete_structured(self.provider.as_ref(), request, validate).await?;
        Ok(GroupName {
            name: v.nombre.trim().trim_matches(['"', '«', '»', '\'']).trim().to_owned(),
            description: v.descripcion.trim().to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structured::testing::FakeProvider;

    fn meetings() -> Vec<(String, String)> {
        vec![
            ("Kickoff Azure".into(), "Se planifica la migración".into()),
            ("Revisión de costes".into(), "Costes de cloud".into()),
        ]
    }

    #[tokio::test]
    async fn the_name_is_written_in_the_configured_language() {
        let system_of = |p: &Arc<FakeProvider>| p.requests.lock().unwrap()[0].system.clone().expect("sys");
        let p = Arc::new(FakeProvider::new(vec![r#"{"nombre":"Azure migration","descripcion":"Weekly sync."}"#]));
        LlmGroupNamer::new(p.clone(), "").with_language("en").name_group(&meetings()).await.expect("name");
        assert!(system_of(&p).contains("palabras en inglés"), "{}", system_of(&p));
        let p = Arc::new(FakeProvider::new(vec![r#"{"nombre":"Migración a Azure","descripcion":"Seguimiento."}"#]));
        LlmGroupNamer::new(p.clone(), "").name_group(&meetings()).await.expect("name");
        let sys = system_of(&p);
        assert!(sys.contains("mismo idioma") && !sys.contains("castellano") && !sys.contains("{{"), "{sys}");
    }

    #[tokio::test]
    async fn names_group_from_structured_output() {
        let p = Arc::new(FakeProvider::new(vec![r#"{"nombre":"\"Migración a Azure\"","descripcion":"  Reuniones de la migración. "}"#]));
        let n = LlmGroupNamer::new(p.clone(), "Edu").name_group(&meetings()).await.expect("name");
        assert_eq!(n.name, "Migración a Azure");
        assert_eq!(n.description, "Reuniones de la migración.");
        let req = &p.requests.lock().unwrap()[0];
        assert!(req.messages[0].content.contains("1. Kickoff Azure") && req.messages[0].content.contains("2. Revisión"));
        assert!(req.json_schema.is_some());
    }

    #[tokio::test]
    async fn too_long_name_is_repaired_once() {
        let p = Arc::new(FakeProvider::new(vec![
            r#"{"nombre":"uno dos tres cuatro cinco seis siete ocho nueve","descripcion":"x"}"#,
            r#"{"nombre":"Plan Azure","descripcion":"x"}"#,
        ]));
        let n = LlmGroupNamer::new(p.clone(), "Edu").name_group(&meetings()).await.expect("name");
        assert_eq!(n.name, "Plan Azure");
        assert_eq!(p.calls(), 2);
    }

    #[tokio::test]
    async fn empty_input_is_rejected_without_calling_llm() {
        let p = Arc::new(FakeProvider::new(vec![]));
        let e = LlmGroupNamer::new(p.clone(), "Edu").name_group(&[]).await.expect_err("err");
        assert!(matches!(e, PortError::InvalidInput(_)));
        assert_eq!(p.calls(), 0);
    }
}
