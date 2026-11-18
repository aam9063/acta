//! `MinutesV1` (cable) → `MinutesOutput` (dominio).

use super::{EscaletaV1, MinutesV1, TareaV1};
use crate::moments::normalize;
use acta_core::ids::{ActionItemId, CaptureId, DecisionId, ProviderId, SpeakerId};
use acta_core::minutes::*;
use acta_core::time::{MediaMs, UnixMs};
use std::collections::HashMap;

pub struct ConvertCtx<'a> {
    pub input: &'a MinutesInput,
    pub provider: ProviderId,
    pub model: Option<String>,
    pub now: UnixMs,
}

fn name_tokens(s: &str) -> Vec<String> {
    normalize(s)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect()
}

/// "Ana" ~ "Ana García", "Ana María López" ~ alias "Ana María": un nombre encaja
/// si sus tokens son subconjunto de los del otro (o al revés).
pub fn names_match(a: &str, b: &str) -> bool {
    let (ta, tb) = (name_tokens(a), name_tokens(b));
    if ta.is_empty() || tb.is_empty() {
        return false;
    }
    ta.iter().all(|t| tb.contains(t)) || tb.iter().all(|t| ta.contains(t))
}

/// ¿`name` (responsable que dio el LLM) es el usuario?
pub fn matches_user(name: &str, me: &UserIdentity) -> bool {
    let n = normalize(name.trim());
    if matches!(n.as_str(), "yo" | "me" | "i" | "mi" | "mia" | "mio") {
        return true;
    }
    std::iter::once(&me.display_name)
        .chain(me.aliases.iter())
        .any(|alias| names_match(name, alias))
}

/// Nombre del dueño de «mis tareas»: el de Ajustes o, si no hay, «Yo».
fn me_name(me: &UserIdentity) -> String {
    match me.display_name.trim() {
        "" => "Yo".to_owned(),
        name => name.to_owned(),
    }
}

/// Reglas que el esquema JSON no puede expresar.
pub fn validate(v: &MinutesV1) -> Result<(), String> {
    if v.resumen.trim().is_empty() {
        return Err("`resumen` está vacío".into());
    }
    fn check(temas: &[Tema], path: &str) -> Result<(), String> {
        for (i, t) in temas.iter().enumerate() {
            let p = format!("{path}{}", i + 1);
            if t.titulo.trim().is_empty() {
                return Err(format!("el tema {p} no tiene título"));
            }
            if t.fin_ms < t.inicio_ms {
                return Err(format!("el tema {p} termina ({}) antes de empezar ({})", t.fin_ms, t.inicio_ms));
            }
            check(&t.subtemas, &format!("{p}."))?;
        }
        Ok(())
    }
    check(&v.escaleta.temas, "")
}

fn cap_map(input: &MinutesInput) -> HashMap<String, CaptureId> {
    input
        .captures
        .iter()
        .map(|c| (c.ref_id.to_lowercase(), c.capture_id))
        .collect()
}

fn resolve_caps(refs: &[String], map: &HashMap<String, CaptureId>) -> Vec<CaptureId> {
    let mut out: Vec<CaptureId> = Vec::new();
    for r in refs {
        if let Some(id) = map.get(r.trim().to_lowercase().as_str())
            && !out.contains(id)
        {
            out.push(*id);
        }
    }
    out
}

fn convert_tema(t: Tema, map: &HashMap<String, CaptureId>, max_ms: Option<u64>) -> Tema {
    let clamp = |v: u64| max_ms.map_or(v, |m| v.min(m));
    let inicio = clamp(t.inicio_ms);
    let fin = clamp(t.fin_ms).max(inicio);
    Tema {
        titulo: t.titulo.trim().to_owned(),
        resumen: t.resumen,
        inicio_ms: inicio,
        fin_ms: fin,
        subtemas: t.subtemas.into_iter().map(|s| convert_tema(s, map, max_ms)).collect(),
        capturas: resolve_caps(&t.capturas, map).iter().map(ToString::to_string).collect(),
    }
}

fn speaker_for(name: &str, input: &MinutesInput, mine: bool) -> Option<SpeakerId> {
    if mine {
        return input.speakers.iter().find(|s| s.is_me).map(|s| s.id);
    }
    input
        .speakers
        .iter()
        .filter(|s| !s.is_me)
        .find(|s| {
            names_match(name, s.shown_name())
                || s.display_name.as_deref().is_some_and(|d| names_match(name, d))
        })
        .map(|s| s.id)
}

fn convert_task(t: TareaV1, ctx: &ConvertCtx, map: &HashMap<String, CaptureId>) -> ActionItem {
    let input = ctx.input;
    // "Ana (yo)" es la etiqueta del transcript, no un nombre: fuera el marcador.
    let raw = t.responsable.map(|r| r.trim().to_owned()).filter(|r| !r.is_empty());
    let marked_me = raw.as_deref().is_some_and(|r| r.to_lowercase().ends_with("(yo)"));
    let responsable = raw
        .map(|r| if marked_me { r[..r.len() - "(yo)".len()].trim().to_owned() } else { r })
        .filter(|r| !r.is_empty())
        .or_else(|| marked_me.then(|| me_name(&input.me)));
    let is_user = marked_me || responsable.as_deref().is_some_and(|r| matches_user(r, &input.me));
    let is_other_known = responsable.as_deref().is_some_and(|r| {
        !is_user && input.speakers.iter().any(|s| !s.is_me && names_match(r, s.shown_name()))
    });
    // El nombre manda sobre el booleano del LLM cuando es inequívoco.
    let is_mine = if is_user {
        true
    } else if is_other_known {
        false
    } else {
        t.es_mia
    };
    let owner_name = match (&responsable, is_mine) {
        (Some(r), _) => Some(r.clone()),
        (None, true) => Some(me_name(&input.me)),
        (None, false) => None,
    };
    let owner_speaker_id = owner_name.as_deref().and_then(|n| speaker_for(n, input, is_mine));
    ActionItem {
        id: ActionItemId::new(),
        meeting_id: input.meeting_id,
        text: t.texto.trim().to_owned(),
        owner_name,
        owner_speaker_id,
        is_mine,
        due: t.fecha_limite.map(|d| d.trim().to_owned()).filter(|d| !d.is_empty()),
        done: false,
        at: t.ts_ms.map(MediaMs),
        capture_ids: resolve_caps(&t.capturas, map),
    }
}

pub fn convert(ctx: &ConvertCtx, v: MinutesV1) -> MinutesOutput {
    let input = ctx.input;
    let map = cap_map(input);
    let max_ms = (input.duration.0 > 0).then_some(input.duration.0);
    let clamp_at = |ms: u64| MediaMs(max_ms.map_or(ms, |m| ms.min(m)));

    let decisions = v
        .decisiones
        .into_iter()
        .filter(|d| !d.texto.trim().is_empty())
        .map(|d| Decision { id: DecisionId::new(), text: d.texto.trim().to_owned(), at: d.ts_ms.map(clamp_at) })
        .collect();
    let action_items = v
        .tareas
        .into_iter()
        .filter(|t| !t.texto.trim().is_empty())
        .map(|t| convert_task(t, ctx, &map))
        .collect();
    let EscaletaV1 { temas } = v.escaleta;
    let mut temas: Vec<Tema> = temas.into_iter().map(|t| convert_tema(t, &map, max_ms)).collect();
    temas.sort_by_key(|t| t.inicio_ms);
    let mut key_moments: Vec<KeyMoment> = v
        .momentos_clave
        .into_iter()
        .map(|m| KeyMoment { at: clamp_at(m.ts_ms), reason: m.motivo, topic: m.tema.filter(|t| !t.trim().is_empty()) })
        .collect();
    key_moments.sort_by_key(|m| m.at);

    MinutesOutput {
        minutes: Minutes {
            meeting_id: input.meeting_id,
            summary_md: v.resumen.trim().to_owned(),
            decisions,
            action_items,
            provider: ctx.provider.clone(),
            model: ctx.model.clone(),
            generated_at: ctx.now,
        },
        escaleta: Escaleta { temas },
        key_moments,
        suggested_title: v.titulo_sugerido.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::minutes::{DecisionV1, MomentoV1};
    use crate::testutil::sample_input;

    fn v1() -> MinutesV1 {
        MinutesV1 {
            titulo_sugerido: Some(" Plan Q1 ".into()),
            resumen: "- Se revisó el plan".into(),
            decisiones: vec![DecisionV1 { texto: "Lanzar el lunes".into(), ts_ms: Some(120_000) }],
            tareas: vec![],
            escaleta: EscaletaV1 {
                temas: vec![Tema {
                    titulo: "Plan".into(),
                    resumen: "r".into(),
                    inicio_ms: 0,
                    fin_ms: 600_000,
                    subtemas: vec![],
                    capturas: vec!["c1".into(), "c9".into(), "C1".into()],
                }],
            },
            momentos_clave: vec![
                MomentoV1 { ts_ms: 400_000, motivo: "b".into(), tema: Some("Plan".into()) },
                MomentoV1 { ts_ms: 100_000, motivo: "a".into(), tema: Some(" ".into()) },
            ],
        }
    }

    fn tarea(texto: &str, resp: Option<&str>, es_mia: bool) -> TareaV1 {
        TareaV1 { texto: texto.into(), responsable: resp.map(str::to_owned), es_mia, fecha_limite: Some("viernes".into()), ts_ms: Some(5_000), capturas: vec!["c1".into()] }
    }

    fn ctx(input: &MinutesInput) -> ConvertCtx<'_> {
        ConvertCtx { input, provider: "fake".into(), model: Some("m".into()), now: UnixMs(99) }
    }

    #[test]
    fn the_transcript_marker_never_leaks_into_the_owner_name() {
        // Visto con el modelo local pequeño: copia la etiqueta "Edu (yo)" del transcript.
        let input = sample_input();
        let map = cap_map(&input);
        let mine = convert_task(tarea("Enviar el informe", Some("Edu (yo)"), false), &ctx(&input), &map);
        assert_eq!((mine.owner_name.as_deref(), mine.is_mine), (Some("Edu"), true));
        let only_marker = convert_task(tarea("Revisar", Some("(yo)"), false), &ctx(&input), &map);
        assert_eq!((only_marker.owner_name.as_deref(), only_marker.is_mine), (Some("Edu"), true));
        let other = convert_task(tarea("Demo", Some("Ana García"), true), &ctx(&input), &map);
        assert_eq!((other.owner_name.as_deref(), other.is_mine), (Some("Ana García"), false));
    }

    #[test]
    fn name_matching_is_accent_case_and_partial_aware() {
        assert!(names_match("EDU", "Edu Navajas"));
        assert!(names_match("Eduardo García", "eduardo"));
        assert!(!names_match("Ana", "Edu"));
        assert!(!names_match("", "Edu"));
    }

    /// Sin nombre en Ajustes, mis tareas son de «Yo» (nunca un nombre vacío).
    #[test]
    fn without_a_user_name_my_tasks_belong_to_yo() {
        let mut input = sample_input();
        input.me = UserIdentity { display_name: String::new(), aliases: vec![] };
        let map = cap_map(&input);
        let labelled = convert_task(tarea("Enviar el informe", Some("Yo"), false), &ctx(&input), &map);
        assert_eq!((labelled.owner_name.as_deref(), labelled.is_mine), (Some("Yo"), true));
        let flagged = convert_task(tarea("Enviar el informe", None, true), &ctx(&input), &map);
        assert_eq!((flagged.owner_name.as_deref(), flagged.is_mine), (Some("Yo"), true));
        let marker = convert_task(tarea("Enviar el informe", Some("(yo)"), false), &ctx(&input), &map);
        assert_eq!((marker.owner_name.as_deref(), marker.is_mine), (Some("Yo"), true));
        assert!(flagged.owner_speaker_id.is_some());
    }

    #[test]
    fn user_matching_uses_display_name_aliases_and_first_person() {
        let me = UserIdentity { display_name: "Edu".into(), aliases: vec!["Eduardo".into(), "Edu Navajas".into()] };
        assert!(matches_user("Edu", &me) && matches_user("Eduardo", &me) && matches_user("yo", &me));
        assert!(!matches_user("Ana", &me));
    }

    #[test]
    fn converts_ids_times_and_translates_capture_refs() {
        let input = sample_input();
        let out = convert(&ctx(&input), v1());
        assert_eq!(out.suggested_title.as_deref(), Some("Plan Q1"));
        assert_eq!(out.minutes.decisions[0].at, Some(MediaMs(120_000)));
        assert_eq!(out.minutes.provider, "fake".into());
        assert_eq!(out.minutes.generated_at, UnixMs(99));
        // c1 y C1 colapsan; c9 desconocida se descarta; se guarda el CaptureId.
        let caps = &out.escaleta.temas[0].capturas;
        assert_eq!(caps, &vec![input.captures[0].capture_id.to_string()]);
        // Momentos ordenados y tema vacío -> None.
        assert_eq!(out.key_moments[0].at, MediaMs(100_000));
        assert_eq!(out.key_moments[0].topic, None);
    }

    #[test]
    fn my_tasks_split_by_name_not_only_by_llm_flag() {
        let input = sample_input(); // yo = Edu, otro = Ana García
        let mut v = v1();
        v.tareas = vec![
            tarea("Enviar presupuesto", Some("Edu"), false), // LLM se equivocó: es mía
            tarea("Preparar demo", Some("Ana"), true),       // LLM se equivocó: no es mía
            tarea("Revisar contrato", None, true),           // sin nombre, confía en LLM
            tarea("Algo del cliente", Some("Cliente"), false),
        ];
        let out = convert(&ctx(&input), v);
        let t = &out.minutes.action_items;
        assert!(t[0].is_mine && t[0].owner_speaker_id == input.speakers.iter().find(|s| s.is_me).map(|s| s.id));
        assert!(!t[1].is_mine);
        assert_eq!(t[1].owner_speaker_id, input.speakers.iter().find(|s| !s.is_me).map(|s| s.id));
        assert!(t[2].is_mine && t[2].owner_name.as_deref() == Some("Edu"));
        assert!(!t[3].is_mine && t[3].owner_speaker_id.is_none());
        assert_eq!(t[0].due.as_deref(), Some("viernes"));
        assert_eq!(t[0].at, Some(MediaMs(5_000)));
        assert_eq!(t[0].capture_ids, vec![input.captures[0].capture_id]);
    }

    #[test]
    fn clamps_times_to_meeting_duration() {
        let mut input = sample_input();
        input.duration = MediaMs(300_000);
        let out = convert(&ctx(&input), v1());
        assert_eq!(out.escaleta.temas[0].fin_ms, 300_000);
        assert_eq!(out.key_moments.last().map(|m| m.at), Some(MediaMs(300_000)));
    }

    #[test]
    fn validation_rejects_inverted_topics_and_empty_summary() {
        let mut v = v1();
        v.escaleta.temas[0].fin_ms = 0;
        v.escaleta.temas[0].inicio_ms = 5;
        assert!(validate(&v).expect_err("err").contains("antes de empezar"));
        let mut v = v1();
        v.resumen = "  ".into();
        assert!(validate(&v).is_err());
        assert!(validate(&v1()).is_ok());
    }
}
