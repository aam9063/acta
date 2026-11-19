//! `SpeakerNamer` (LLM) con el esquema `acta.speaker_names.v1`: el nombre de
//! cada "Speaker N" según lo que se dice en la reunión ("Hola, soy María",
//! "gracias, Pepe"). El LLM propone; aquí se filtra todo lo que no tenga
//! base en la transcripción (nombres inventados, el del usuario, repetidos).

use crate::structured::complete_structured;
use crate::transcript::{Line, estimate_tokens, join_lines, me_label, of_user, render_lines, render_template, user_ref};
use crate::{prompts, schemas};
use acta_core::PortResult;
use acta_core::ids::SpeakerId;
use acta_core::llm::{LlmMessage, LlmProvider, LlmRequest, Role};
use acta_core::transcription::{SpeakerNameGuess, SpeakerNamer, SpeakerNamingInput};
use async_trait::async_trait;
use serde::Deserialize;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpeakerNamesV1 {
    hablantes: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    etiqueta: String,
    nombre: Option<String>,
    #[serde(default)]
    confianza: String,
    #[serde(default)]
    evidencia: String,
}

/// Tope de transcript en el prompt (tokens estimados); se recorta además al
/// contexto del proveedor.
const MAX_TRANSCRIPT_TOKENS: usize = 24_000;
/// Líneas de contexto a cada lado de una línea con un posible nombre.
const CONTEXT_LINES: usize = 2;

pub struct LlmSpeakerNamer {
    provider: Arc<dyn LlmProvider>,
}

impl LlmSpeakerNamer {
    pub fn new(provider: Arc<dyn LlmProvider>) -> Self {
        Self { provider }
    }

    fn budget(&self) -> usize {
        let ctx = self.provider.capabilities().context_tokens as usize;
        MAX_TRANSCRIPT_TOKENS.min((ctx * 6 / 10).saturating_sub(1_500)).max(1_000)
    }
}

/// Minúsculas y sin tildes: «María» == «maria».
fn fold(s: &str) -> String {
    s.trim()
        .chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            other => other,
        })
        .collect()
}

fn words(s: &str) -> Vec<String> {
    fold(s).split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_owned).collect()
}

/// ¿Puede haber un nombre en la línea? Una palabra en mayúscula que no
/// empieza frase ("gracias, Marta"), o una presentación ("soy marta").
fn mentions_a_name(text: &str) -> bool {
    let mut sentence_start = true;
    for raw in text.split_whitespace() {
        let w = raw.trim_start_matches(|c: char| !c.is_alphanumeric());
        if !sentence_start && w.chars().next().is_some_and(char::is_uppercase) {
            return true;
        }
        sentence_start = raw.ends_with(['.', '?', '!', ':']) || w.is_empty();
    }
    let low = fold(text);
    ["soy ", "me llamo ", "mi nombre es ", "i'm ", "my name is ", "this is "].iter().any(|p| low.contains(p))
}

/// Todo el transcript si cabe; si no, las líneas con posibles nombres y su
/// contexto, en orden, hasta el presupuesto.
fn excerpt(lines: &[Line], budget: usize) -> Vec<Line> {
    let total: usize = lines.iter().map(|l| estimate_tokens(&l.text) + 1).sum();
    if total <= budget {
        return lines.to_vec();
    }
    let mut keep = BTreeSet::new();
    for (i, l) in lines.iter().enumerate() {
        // El texto sin "[mm:ss|ms] Nombre: ".
        let text = l.text.split_once(": ").map_or(l.text.as_str(), |(_, t)| t);
        if mentions_a_name(text) {
            keep.extend(i.saturating_sub(CONTEXT_LINES)..=(i + CONTEXT_LINES).min(lines.len() - 1));
        }
    }
    let mut out = Vec::new();
    let mut used = 0;
    let mut last: Option<usize> = None;
    for i in keep {
        let t = estimate_tokens(&lines[i].text) + 1;
        if used + t > budget {
            break;
        }
        if last.is_some_and(|p| p + 1 != i) {
            out.push(Line { at_ms: lines[i].at_ms, text: "[…]".into() });
        }
        used += t;
        out.push(lines[i].clone());
        last = Some(i);
    }
    out
}

fn rank(confianza: &str) -> Option<u8> {
    match fold(confianza).as_str() {
        "alta" | "high" => Some(2),
        "media" | "medium" => Some(1),
        _ => None,
    }
}

fn is_generic(name: &str) -> bool {
    let f = fold(name);
    ["speaker", "hablante", "desconocido", "participante", "yo", "unknown"].iter().any(|g| f == *g || f.starts_with(&format!("{g} ")))
}

/// Candidato válido: hablante, nombre, evidencia y confianza (2 alta, 1 media).
type Cand = (SpeakerId, String, String, u8);

/// Lo que se acepta de la respuesta del LLM.
fn accept(input: &SpeakerNamingInput, entries: Vec<Entry>, transcript_words: &BTreeSet<String>) -> Vec<SpeakerNameGuess> {
    let targets: HashMap<String, SpeakerId> = input
        .speakers
        .iter()
        .filter(|s| input.targets.contains(&s.id))
        .map(|s| (fold(&s.label), s.id))
        .collect();
    let mut self_words: BTreeSet<String> = BTreeSet::new();
    let mut self_full: BTreeSet<String> = BTreeSet::new();
    for n in input.self_names.iter().chain(std::iter::once(&input.me_name)).filter(|n| !n.trim().is_empty()) {
        self_full.insert(fold(n));
        if let Some(first) = words(n).into_iter().next() {
            self_words.insert(first);
        }
    }
    let taken: BTreeSet<String> = input
        .speakers
        .iter()
        .filter(|s| !input.targets.contains(&s.id) && !s.is_me)
        .filter_map(|s| s.display_name.as_deref().map(fold))
        .collect();
    let participant_words: BTreeSet<String> = input.participants.iter().flat_map(|p| words(p)).collect();

    let mut ok: Vec<Cand> = Vec::new();
    for e in entries {
        let Some(&id) = targets.get(&fold(&e.etiqueta)) else { continue };
        let Some(conf) = rank(&e.confianza) else { continue };
        let name = e.nombre.unwrap_or_default().trim().trim_matches(['"', '«', '»', '\'', '.', ',']).trim().to_owned();
        let ws = words(&name);
        if ws.is_empty() || ws.len() > 4 || name.chars().count() > 60 || is_generic(&name) {
            continue;
        }
        if self_full.contains(&fold(&name)) || self_words.contains(&ws[0]) || taken.contains(&fold(&name)) {
            continue;
        }
        // Con base: el nombre de pila se oye en la reunión; el resto, o se
        // oye o viene de un participante conocido.
        let grounded = transcript_words.contains(&ws[0]) && ws.iter().all(|w| transcript_words.contains(w) || participant_words.contains(w));
        if !grounded {
            continue;
        }
        ok.push((id, name, e.evidencia.trim().to_owned(), conf));
    }
    // Un nombre por hablante y un hablante por nombre: gana la mayor
    // confianza; empate = ambiguo, fuera.
    let unique = |ok: &[Cand], key: &dyn Fn(&Cand) -> String| {
        let mut best: HashMap<String, (u8, usize)> = HashMap::new();
        for e in ok {
            let k = key(e);
            let entry = best.entry(k).or_insert((e.3, 0));
            if e.3 > entry.0 {
                *entry = (e.3, 1);
            } else if e.3 == entry.0 {
                entry.1 += 1;
            }
        }
        ok.iter()
            .filter(|e| best.get(&key(e)).is_some_and(|(c, n)| *c == e.3 && *n == 1))
            .cloned()
            .collect::<Vec<_>>()
    };
    // Varias entradas iguales (mismo hablante y nombre) cuentan como una.
    let mut dedup: Vec<Cand> = Vec::new();
    for e in ok {
        match dedup.iter_mut().find(|d| d.0 == e.0 && fold(&d.1) == fold(&e.1)) {
            Some(d) if e.3 > d.3 => *d = e,
            Some(_) => {}
            None => dedup.push(e),
        }
    }
    let by_name = unique(&dedup, &|e| fold(&e.1));
    let by_speaker = unique(&by_name, &|e| e.0.to_string());
    by_speaker
        .into_iter()
        .map(|(speaker_id, name, evidence, _)| SpeakerNameGuess { speaker_id, name, evidence })
        .collect()
}

#[async_trait]
impl SpeakerNamer for LlmSpeakerNamer {
    async fn name_speakers(&self, input: SpeakerNamingInput) -> PortResult<Vec<SpeakerNameGuess>> {
        let target_labels: Vec<&str> = input
            .speakers
            .iter()
            .filter(|s| input.targets.contains(&s.id))
            .map(|s| s.label.as_str())
            .collect();
        if target_labels.is_empty() || input.utterances.is_empty() {
            return Ok(vec![]);
        }
        // Los genéricos salen con su etiqueta ("Speaker 2"), que es la que
        // tiene que devolver el LLM.
        let mut speakers = input.speakers.clone();
        for s in speakers.iter_mut().filter(|s| input.targets.contains(&s.id)) {
            s.display_name = None;
        }
        let lines = render_lines(&speakers, &input.utterances, &input.me_name);
        let transcript_words: BTreeSet<String> = input.utterances.iter().flat_map(|u| words(&u.text)).collect();
        let known: Vec<String> = speakers
            .iter()
            .filter(|s| !s.is_me && !input.targets.contains(&s.id))
            .filter_map(|s| s.display_name.clone())
            .collect();
        let participants = if input.participants.is_empty() { "ninguno".to_owned() } else { input.participants.join(", ") };
        let user = format!(
            "Reunión: {}\nParticipantes conocidos (de la app de la reunión): {}\nHablantes ya identificados: {}\nHablantes sin nombre: {}\n\n# Transcripción\n{}",
            input.title.trim(),
            participants,
            if known.is_empty() { "ninguno".to_owned() } else { known.join(", ") },
            target_labels.join(", "),
            join_lines(&excerpt(&lines, self.budget())),
        );
        let self_names = {
            let mut v: Vec<&str> = input.self_names.iter().map(String::as_str).filter(|n| !n.trim().is_empty()).collect();
            if !input.me_name.trim().is_empty() && !v.iter().any(|n| fold(n) == fold(&input.me_name)) {
                v.insert(0, input.me_name.trim());
            }
            if v.is_empty() { "ninguno conocido".to_owned() } else { v.join(", ") }
        };
        let request = LlmRequest {
            system: Some(render_template(
                prompts::SPEAKER_NAMES_SYSTEM,
                &[
                    ("of_user", &of_user(&input.me_name)),
                    ("user_ref", user_ref(&input.me_name)),
                    ("me_label", &me_label(&input.me_name)),
                    ("self_names", &self_names),
                ],
            )),
            messages: vec![LlmMessage { role: Role::User, content: user }],
            json_schema: serde_json::from_str(schemas::SPEAKER_NAMES_V1).ok(),
            max_tokens: Some(800),
            temperature: Some(0.0),
        };
        let v: SpeakerNamesV1 = complete_structured(self.provider.as_ref(), request, |_| Ok(())).await?;
        Ok(accept(&input, v.hablantes, &transcript_words))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structured::testing::FakeProvider;
    use crate::testutil::{speaker, utt};
    use acta_core::ids::SpeakerId;

    /// Yo (Edu, 1), Speaker 2 y Speaker 3 sin nombre; Ana (4) ya conocida.
    fn input(lines: &[(u128, &str)]) -> SpeakerNamingInput {
        let mut ana = speaker(4, false, Some("Ana"));
        ana.label = "Speaker 3".into();
        let mut s3 = speaker(3, false, None);
        s3.label = "Speaker 2".into();
        let mut s2 = speaker(2, false, None);
        s2.label = "Speaker 1".into();
        SpeakerNamingInput {
            title: "Refinamiento".into(),
            me_name: "Edu".into(),
            self_names: vec!["Edu Navajas".into(), "Eduardo".into()],
            participants: vec!["Marta López".into()],
            speakers: vec![speaker(1, true, None), s2, s3, ana],
            utterances: lines.iter().enumerate().map(|(i, (sp, t))| utt(i as u32, *sp, i as u64 * 3_000, t)).collect(),
            targets: vec![SpeakerId::from_u128(2), SpeakerId::from_u128(3)],
        }
    }

    fn reply(entries: &[(&str, Option<&str>, &str)]) -> String {
        let items: Vec<_> = entries
            .iter()
            .map(|(l, n, c)| serde_json::json!({"etiqueta": l, "nombre": n, "confianza": c, "evidencia": "cita"}))
            .collect();
        serde_json::json!({ "hablantes": items }).to_string()
    }

    fn names(g: &[SpeakerNameGuess]) -> Vec<(u128, &str)> {
        g.iter().map(|g| (g.speaker_id.0.as_u128(), g.name.as_str())).collect()
    }

    /// Caso real (19-nov-2026): «Hola, soy María» y el hablante seguía «Speaker 5».
    #[tokio::test]
    async fn a_self_introduction_and_a_vocative_name_the_generic_speakers() {
        let lines = [(2, "Hola, soy María, de Orbio."), (1, "Encantado. ¿Tú qué opinas, Pepe?"), (3, "Pues yo lo haría en un único HTML.")];
        let p = Arc::new(FakeProvider::new(vec![&reply(&[("Speaker 1", Some("María"), "alta"), ("Speaker 2", Some("Pepe"), "media")])]));
        let g = LlmSpeakerNamer::new(p.clone()).name_speakers(input(&lines)).await.expect("ok");
        assert_eq!(names(&g), vec![(2, "María"), (3, "Pepe")]);
        let req = &p.requests.lock().unwrap()[0];
        let user = &req.messages[0].content;
        assert!(user.contains("Speaker 1: Hola, soy María"), "{user}");
        assert!(user.contains("Edu (yo): Encantado"), "{user}");
        assert!(user.contains("Hablantes sin nombre: Speaker 1, Speaker 2"), "{user}");
        assert!(user.contains("Marta López"), "los participantes conocidos van como pista: {user}");
        let sys = req.system.clone().expect("sys");
        assert!(sys.contains("Edu Navajas") && !sys.contains("{{"), "{sys}");
        assert!(req.json_schema.is_some());
    }

    #[tokio::test]
    async fn invented_own_duplicated_unknown_or_doubtful_names_are_dropped() {
        let lines = [(2, "Vale, dale, Edu."), (1, "Gracias, Javi."), (3, "Javi, mira esto."), (2, "Sí, Ana, ahora lo miro.")];
        let p = Arc::new(FakeProvider::new(vec![&reply(&[
            // El usuario: nunca es otro hablante.
            ("Speaker 1", Some("Edu"), "alta"),
            // Duplicado con la misma confianza: ambiguo, fuera los dos.
            ("Speaker 1", Some("Javi"), "media"),
            ("Speaker 2", Some("Javi"), "media"),
            // Inventado: no aparece en la transcripción.
            ("Speaker 2", Some("Lucía"), "alta"),
            // Ya es de otro hablante (Ana).
            ("Speaker 2", Some("Ana"), "alta"),
            // Dudoso y etiqueta desconocida.
            ("Speaker 1", Some("Javi"), "baja"),
            ("Speaker 9", Some("Javi"), "alta"),
            ("Speaker 2", None, "baja"),
        ])]));
        let g = LlmSpeakerNamer::new(p).name_speakers(input(&lines)).await.expect("ok");
        assert!(g.is_empty(), "{g:?}");
    }

    #[tokio::test]
    async fn the_best_guess_wins_and_spelling_can_differ_in_accents() {
        let lines = [(2, "Hola, soy Maria."), (3, "Hola, Maria, bienvenida.")];
        let p = Arc::new(FakeProvider::new(vec![&reply(&[("Speaker 1", Some("María"), "alta"), ("Speaker 2", Some("María"), "media")])]));
        let g = LlmSpeakerNamer::new(p).name_speakers(input(&lines)).await.expect("ok");
        assert_eq!(names(&g), vec![(2, "María")]);
    }

    #[tokio::test]
    async fn a_known_participant_counts_as_evidence_for_the_full_name() {
        let lines = [(2, "Soy Marta, la de producto."), (3, "Perfecto.")];
        let p = Arc::new(FakeProvider::new(vec![&reply(&[("Speaker 1", Some("Marta López"), "alta")])]));
        let g = LlmSpeakerNamer::new(p).name_speakers(input(&lines)).await.expect("ok");
        assert_eq!(names(&g), vec![(2, "Marta López")]);
    }

    #[tokio::test]
    async fn no_targets_means_no_llm_call() {
        let p = Arc::new(FakeProvider::new(vec![]));
        let mut i = input(&[(2, "Hola")]);
        i.targets.clear();
        assert!(LlmSpeakerNamer::new(p.clone()).name_speakers(i).await.expect("ok").is_empty());
        assert_eq!(p.calls(), 0);
    }

    /// Una reunión larga no cabe entera: van las líneas con nombres y su contexto.
    #[tokio::test]
    async fn long_meetings_send_the_lines_around_names_within_budget() {
        let mut lines: Vec<(u128, String)> = (0..4_000).map(|i| (2 + (i % 2) as u128, format!("seguimos con el punto {i} de la lista de cosas pendientes"))).collect();
        lines[3_000] = (3, "Vale, gracias, Pepe.".into());
        let refs: Vec<(u128, &str)> = lines.iter().map(|(s, t)| (*s, t.as_str())).collect();
        let p = Arc::new(FakeProvider::new(vec![&reply(&[])]));
        LlmSpeakerNamer::new(p.clone()).name_speakers(input(&refs)).await.expect("ok");
        let user = p.requests.lock().unwrap()[0].messages[0].content.clone();
        assert!(user.contains("gracias, Pepe") && user.contains("punto 2999") && user.contains("punto 3001"), "contexto del vocativo");
        assert!(crate::transcript::estimate_tokens(&user) < 30_000, "{}", crate::transcript::estimate_tokens(&user));
    }

    #[tokio::test]
    async fn an_unparseable_answer_is_an_error_for_the_caller_to_ignore() {
        let p = Arc::new(FakeProvider::new(vec!["no sé", "sigo sin saber"]));
        assert!(LlmSpeakerNamer::new(p).name_speakers(input(&[(2, "Hola")])).await.is_err());
    }
}
