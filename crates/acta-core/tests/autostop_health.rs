#![allow(clippy::expect_used, clippy::unwrap_used)]
//! Reglas de la política de autoparada y del monitor de salud (WP1).

use acta_core::permissions::PermissionKind;
use acta_core::recording::*;
use acta_core::time::UnixMs;

const T0: UnixMs = UnixMs(1_000_000);

fn silent(at: UnixMs) -> Levels {
    Levels {
        mic: Level::SILENT,
        system: Level::SILENT,
        at,
    }
}
fn voice(at: UnixMs) -> Levels {
    Levels {
        mic: Level {
            rms_dbfs: -25.0,
            peak_dbfs: -12.0,
            all_zero: false,
        },
        system: Level::SILENT,
        at,
    }
}
fn quiet_but_alive(at: UnixMs) -> Levels {
    // Ruido de fondo por debajo del umbral, pero NO ceros exactos.
    let l = Level {
        rms_dbfs: -70.0,
        peak_dbfs: -60.0,
        all_zero: false,
    };
    Levels {
        mic: l,
        system: l,
        at,
    }
}

fn cfg() -> AutoStopConfig {
    AutoStopConfig {
        silence_threshold_dbfs: -50.0,
        silence_prompt_after_ms: 1_000,
        auto_stop_after_unanswered_ms: 1_000,
        max_duration_ms: 10_000,
    }
}

// ---------------- AutoStopPolicy ----------------

#[test]
fn silence_below_threshold_counts_even_with_background_noise() {
    let mut p = AutoStopPolicy::new(cfg());
    assert_eq!(p.on_levels(&quiet_but_alive(T0)), None);
    assert_eq!(
        p.on_levels(&quiet_but_alive(T0.plus_ms(1_000))),
        Some(SessionEvent::SilenceDetected {
            at: T0.plus_ms(1_000)
        })
    );
}

#[test]
fn silence_prompt_is_emitted_only_once() {
    let mut p = AutoStopPolicy::new(cfg());
    p.on_levels(&silent(T0));
    assert!(p.on_levels(&silent(T0.plus_ms(1_000))).is_some());
    assert_eq!(p.on_levels(&silent(T0.plus_ms(1_100))), None);
    assert_eq!(p.on_levels(&silent(T0.plus_ms(5_000))), None);
}

#[test]
fn speech_on_one_track_prevents_silence_prompt() {
    let mut p = AutoStopPolicy::new(cfg());
    p.on_levels(&silent(T0));
    assert_eq!(p.on_levels(&voice(T0.plus_ms(900))), None);
    assert_eq!(p.on_levels(&silent(T0.plus_ms(1_500))), None);
}

#[test]
fn decline_does_not_reprompt_until_speech_then_silence_again() {
    let mut p = AutoStopPolicy::new(cfg());
    p.on_levels(&silent(T0));
    assert!(p.on_levels(&silent(T0.plus_ms(1_000))).is_some());
    p.on_stop_prompt_opened(T0.plus_ms(1_000));
    p.on_stop_prompt_closed(); // "No"
    // Silencio eterno: no se vuelve a preguntar ni se para.
    assert_eq!(p.on_levels(&silent(T0.plus_ms(2_000))), None);
    assert_eq!(p.on_levels(&silent(T0.plus_ms(60_000))), None);
    // Hay voz y luego otra vez silencio: ahora sí.
    assert_eq!(p.on_levels(&voice(T0.plus_ms(61_000))), None);
    assert_eq!(p.on_levels(&silent(T0.plus_ms(61_100))), None);
    assert!(matches!(
        p.on_levels(&silent(T0.plus_ms(62_200))),
        Some(SessionEvent::SilenceDetected { .. })
    ));
}

#[test]
fn unanswered_prompt_counts_from_prompt_opening_not_from_old_silence() {
    // MeetingEnded abre la pregunta tras mucho silencio previo: hay que dar
    // `auto_stop_after_unanswered_ms` completos para responder.
    let mut p = AutoStopPolicy::new(AutoStopConfig {
        silence_prompt_after_ms: 10_000_000,
        ..cfg()
    });
    p.on_levels(&silent(T0));
    p.on_stop_prompt_opened(T0.plus_ms(5_000));
    assert_eq!(p.on_levels(&silent(T0.plus_ms(5_500))), None);
    assert!(matches!(
        p.on_levels(&silent(T0.plus_ms(6_000))),
        Some(SessionEvent::AutoStopDue {
            reason: StopReason::AutoSilence,
            ..
        })
    ));
}

#[test]
fn auto_stop_fires_once() {
    let mut p = AutoStopPolicy::new(cfg());
    p.on_levels(&silent(T0));
    p.on_stop_prompt_opened(T0);
    assert!(p.on_levels(&silent(T0.plus_ms(1_000))).is_some());
    assert_eq!(p.on_levels(&silent(T0.plus_ms(1_100))), None);
}

#[test]
fn max_duration_excludes_paused_time() {
    let mut p = AutoStopPolicy::new(cfg());
    assert_eq!(p.on_tick(T0), None);
    assert_eq!(p.on_tick(T0.plus_ms(4_000)), None); // 4 s grabados
    p.on_paused(T0.plus_ms(4_000));
    assert_eq!(p.on_tick(T0.plus_ms(100_000)), None); // pausado: no cuenta
    p.on_resumed(T0.plus_ms(100_000));
    assert_eq!(p.on_tick(T0.plus_ms(105_000)), None); // 9 s en total
    assert!(matches!(
        p.on_tick(T0.plus_ms(106_500)),
        Some(SessionEvent::AutoStopDue {
            reason: StopReason::MaxDuration,
            ..
        })
    )); // 10,5 s
    assert_eq!(p.on_tick(T0.plus_ms(107_000)), None, "solo una vez");
}

#[test]
fn levels_while_paused_are_ignored() {
    let mut p = AutoStopPolicy::new(cfg());
    p.on_levels(&silent(T0));
    p.on_paused(T0.plus_ms(100));
    assert_eq!(p.on_levels(&silent(T0.plus_ms(50_000))), None);
    p.on_resumed(T0.plus_ms(60_000));
    // el contador de silencio arranca de cero tras reanudar
    assert_eq!(p.on_levels(&silent(T0.plus_ms(60_100))), None);
    assert!(p.on_levels(&silent(T0.plus_ms(61_200))).is_some());
}

// ---------------- HealthMonitor ----------------

fn hcfg() -> HealthConfig {
    HealthConfig {
        stall_ms: 3_000,
        zeros_ms: 10_000,
        segment_grace_ms: 10_000,
        min_free_bytes: 1_000,
    }
}

fn monitor() -> HealthMonitor {
    HealthMonitor::new(hcfg(), 30, &Track::ALL, T0)
}

fn feed(m: &mut HealthMonitor, track: Track, all_zero: bool, at: UnixMs) {
    m.on_frame(track, all_zero, at);
}

fn raised(w: &[HealthWarning]) -> Vec<&HealthIssue> {
    w.iter().filter(|w| !w.cleared).map(|w| &w.issue).collect()
}

#[test]
fn healthy_tracks_emit_nothing() {
    let mut m = monitor();
    for i in 0..100 {
        let at = T0.plus_ms(i * 100);
        feed(&mut m, Track::Mic, false, at);
        feed(&mut m, Track::System, false, at);
    }
    assert!(m.tick(T0.plus_ms(10_000), Some(1_000_000)).is_empty());
}

#[test]
fn all_zero_frames_for_ten_seconds_raise_all_zeros_and_clear_on_signal() {
    let mut m = monitor();
    for i in 0..110 {
        let at = T0.plus_ms(i * 100);
        feed(&mut m, Track::Mic, false, at);
        feed(&mut m, Track::System, true, at);
    }
    let w = m.tick(T0.plus_ms(10_900), None);
    assert!(raised(&w).contains(&&HealthIssue::TrackAllZeros {
        track: Track::System
    }));
    assert!(!raised(&w).contains(&&HealthIssue::TrackAllZeros { track: Track::Mic }));
    // No se repite en el siguiente tick.
    assert!(m.tick(T0.plus_ms(11_000), None).iter().all(|w| w.cleared));
    // Llega señal real: se limpia.
    feed(&mut m, Track::System, false, T0.plus_ms(11_000));
    let w = m.tick(T0.plus_ms(11_100), None);
    assert!(w.iter().any(|w| w.cleared
        && w.issue
            == HealthIssue::TrackAllZeros {
                track: Track::System
            }));
}

#[test]
fn zeros_below_threshold_do_not_warn() {
    let mut m = monitor();
    for i in 0..90 {
        let at = T0.plus_ms(i * 100);
        feed(&mut m, Track::Mic, true, at);
        feed(&mut m, Track::System, false, at);
    }
    assert!(raised(&m.tick(T0.plus_ms(8_900), None)).is_empty());
}

#[test]
fn stalled_track_warns_after_stall_ms_and_clears_when_frames_resume() {
    let mut m = monitor();
    feed(&mut m, Track::Mic, false, T0);
    feed(&mut m, Track::System, false, T0);
    for i in 1..=20 {
        feed(&mut m, Track::Mic, false, T0.plus_ms(i * 100));
    }
    let w = m.tick(T0.plus_ms(3_500), None);
    assert_eq!(
        raised(&w),
        vec![&HealthIssue::TrackStalled {
            track: Track::System
        }]
    );
    feed(&mut m, Track::System, false, T0.plus_ms(3_600));
    let w = m.tick(T0.plus_ms(3_700), None);
    assert!(w.iter().any(|w| w.cleared
        && w.issue
            == HealthIssue::TrackStalled {
                track: Track::System
            }));
}

#[test]
fn track_that_never_delivers_stalls_from_start() {
    let mut m = monitor();
    feed(&mut m, Track::Mic, false, T0.plus_ms(3_000));
    let w = m.tick(T0.plus_ms(3_100), None);
    assert!(raised(&w).contains(&&HealthIssue::TrackStalled {
        track: Track::System
    }));
}

#[test]
fn restarting_device_suppresses_stall_until_running_again() {
    let mut m = monitor();
    feed(&mut m, Track::Mic, false, T0);
    feed(&mut m, Track::System, false, T0);
    m.on_track_state(Track::Mic, TrackState::Restarting, T0.plus_ms(100));
    feed(&mut m, Track::System, false, T0.plus_ms(5_000));
    assert!(
        raised(&m.tick(T0.plus_ms(5_000), None)).is_empty(),
        "reconectando no es un fallo"
    );
    m.on_track_state(Track::Mic, TrackState::Running, T0.plus_ms(6_000));
    // Margen de gracia tras volver a Running.
    assert!(raised(&m.tick(T0.plus_ms(7_000), None)).is_empty());
    feed(&mut m, Track::System, false, T0.plus_ms(9_900));
    let w = m.tick(T0.plus_ms(9_500), None);
    assert!(raised(&w).contains(&&HealthIssue::TrackStalled { track: Track::Mic }));
}

#[test]
fn failed_track_state_is_reported_immediately_as_stalled() {
    let mut m = monitor();
    feed(&mut m, Track::Mic, false, T0);
    feed(&mut m, Track::System, false, T0);
    m.on_track_state(Track::Mic, TrackState::Failed, T0.plus_ms(100));
    let w = m.tick(T0.plus_ms(200), None);
    assert!(raised(&w).contains(&&HealthIssue::TrackStalled { track: Track::Mic }));
}

#[test]
fn segments_stalled_after_segment_seconds_plus_grace() {
    let mut m = monitor();
    for i in 0..400 {
        let at = T0.plus_ms(i * 100);
        feed(&mut m, Track::Mic, false, at);
        feed(&mut m, Track::System, false, at);
    }
    m.on_segment_finalized(Track::Mic, T0.plus_ms(30_000));
    let w = m.tick(T0.plus_ms(40_000), None);
    assert!(raised(&w).contains(&&HealthIssue::SegmentsStalled {
        track: Track::System
    }));
    assert!(!raised(&w).contains(&&HealthIssue::SegmentsStalled { track: Track::Mic }));
    m.on_segment_finalized(Track::System, T0.plus_ms(40_000));
    let w = m.tick(T0.plus_ms(40_000), None);
    assert!(w.iter().any(|w| w.cleared
        && w.issue
            == HealthIssue::SegmentsStalled {
                track: Track::System
            }));
}

#[test]
fn pause_suspends_all_checks_and_resume_regraces() {
    let mut m = monitor();
    feed(&mut m, Track::Mic, false, T0);
    feed(&mut m, Track::System, false, T0);
    m.on_paused(T0.plus_ms(100));
    assert!(m.tick(T0.plus_ms(120_000), None).is_empty());
    m.on_resumed(T0.plus_ms(120_000));
    assert!(m.tick(T0.plus_ms(121_000), None).is_empty());
}

#[test]
fn disk_low_raises_and_clears() {
    let mut m = monitor();
    feed(&mut m, Track::Mic, false, T0);
    feed(&mut m, Track::System, false, T0);
    let w = m.tick(T0.plus_ms(100), Some(500));
    assert!(raised(&w).contains(&&HealthIssue::DiskLow { free_bytes: 500 }));
    let w = m.tick(T0.plus_ms(200), Some(5_000));
    assert!(
        w.iter()
            .any(|w| w.cleared && matches!(w.issue, HealthIssue::DiskLow { .. }))
    );
}

#[test]
fn one_shot_events_are_reported_once_in_spanish() {
    let mut m = monitor();
    feed(&mut m, Track::Mic, false, T0);
    feed(&mut m, Track::System, false, T0);
    m.on_overrun(Track::Mic, 3, T0.plus_ms(10));
    m.on_write_failed(Track::System, "disco lleno".into(), T0.plus_ms(20));
    m.on_sidecar_restarted(T0.plus_ms(30));
    m.on_sidecar_restarted(T0.plus_ms(40));
    m.on_permission_missing(PermissionKind::Microphone, T0.plus_ms(50));
    let w = m.tick(T0.plus_ms(100), None);
    let issues = raised(&w);
    assert!(issues.contains(&&HealthIssue::Overrun {
        track: Track::Mic,
        dropped_frames: 3
    }));
    assert!(issues.contains(&&HealthIssue::WriteFailed {
        track: Track::System,
        message: "disco lleno".into()
    }));
    assert!(issues.contains(&&HealthIssue::SidecarRestarted { count: 2 }));
    assert!(
        issues
            .iter()
            .any(|i| matches!(i, HealthIssue::PermissionMissing { .. }))
    );
    assert!(w.iter().all(|w| !w.message.is_empty()));
    assert!(m.tick(T0.plus_ms(200), None).is_empty());
}

/// Caso real (16-nov-2026): MacBook con la tapa cerrada y monitor externo →
/// el micro integrado entrega ceros exactos con el permiso CONCEDIDO.
#[test]
fn mic_all_zeros_message_mentions_closed_lid_and_choosing_another_mic() {
    let mut m = monitor();
    for i in 0..110 {
        let at = T0.plus_ms(i * 100);
        feed(&mut m, Track::Mic, true, at);
        feed(&mut m, Track::System, false, at);
    }
    let w = m.tick(T0.plus_ms(10_900), None);
    let mic = w.iter().find(|w| w.issue == HealthIssue::TrackAllZeros { track: Track::Mic }).expect("aviso");
    assert!(mic.message.contains("tapa cerrada"), "{}", mic.message);
    assert!(mic.message.contains("Ajustes"), "{}", mic.message);
}

/// Real (17-nov-2026): sin permiso de pantalla la pastilla decía «Falta un
/// permiso del sistema para grabar», como si no se grabara nada. El audio iba
/// bien: el aviso debe decir QUÉ falta y que solo afecta a las capturas.
#[test]
fn a_missing_permission_says_which_one_and_screen_recording_does_not_alarm_about_audio() {
    let mut m = monitor();
    m.on_permission_missing(PermissionKind::ScreenRecording, T0);
    let w = m.tick(T0.plus_ms(10), None);
    let msg = &w.iter().find(|w| matches!(w.issue, HealthIssue::PermissionMissing { .. })).expect("aviso").message;
    assert!(msg.starts_with("Grabación de pantalla sin permiso: no habrá capturas"), "{msg}");
    assert!(msg.contains("el audio se graba bien"), "{msg}");
    let mut m = monitor();
    m.on_permission_missing(PermissionKind::Microphone, T0);
    let w = m.tick(T0.plus_ms(10), None);
    assert!(w[0].message.contains("Micrófono"), "{}", w[0].message);
}

/// Real (19-nov-2026): el aviso de pantalla era de un solo disparo y nunca se
/// limpiaba; la pastilla lo enseñaba toda la reunión aunque el permiso
/// funcionara después. Ahora dura mientras falte y se limpia al concederse.
#[test]
fn screen_permission_warning_stays_until_granted_and_then_clears() {
    let mut m = monitor();
    for t in [Track::Mic, Track::System] {
        feed(&mut m, t, false, T0);
    }
    m.on_permission_missing(PermissionKind::ScreenRecording, T0);
    let w = m.tick(T0.plus_ms(10), None);
    assert_eq!(raised(&w), vec![&HealthIssue::PermissionMissing { permission: PermissionKind::ScreenRecording }]);
    // Sigue activo: no se repite, pero tampoco se limpia.
    for t in [Track::Mic, Track::System] {
        feed(&mut m, t, false, T0.plus_ms(500));
    }
    assert!(m.tick(T0.plus_ms(600), None).is_empty());
    // Volver a fallar no duplica el aviso.
    m.on_permission_missing(PermissionKind::ScreenRecording, T0.plus_ms(700));
    assert!(m.tick(T0.plus_ms(800), None).is_empty());
    m.on_permission_granted(PermissionKind::ScreenRecording);
    let w = m.tick(T0.plus_ms(900), None);
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].cleared);
    assert_eq!(w[0].issue, HealthIssue::PermissionMissing { permission: PermissionKind::ScreenRecording });
    // Conceder algo que no faltaba no emite nada.
    m.on_permission_granted(PermissionKind::Accessibility);
    assert!(m.tick(T0.plus_ms(950), None).is_empty());
}

/// Si en Ajustes ya aparece activado (app sin firmar reinstalada: macOS
/// guarda el permiso de la versión anterior), el aviso dice cómo arreglarlo.
#[test]
fn screen_permission_warning_explains_the_stale_grant_after_reinstalling() {
    let mut m = monitor();
    m.on_permission_missing(PermissionKind::ScreenRecording, T0);
    let w = m.tick(T0.plus_ms(10), None);
    let msg = &w.iter().find(|w| matches!(w.issue, HealthIssue::PermissionMissing { .. })).expect("aviso").message;
    assert!(msg.contains("si ya está activado"), "{msg}");
    assert!(msg.contains("quita acta"), "{msg}");
    assert!(msg.contains("reintenta"), "{msg}");
}

#[test]
fn a_permission_warning_survives_a_pause() {
    let mut m = monitor();
    m.on_permission_missing(PermissionKind::ScreenRecording, T0);
    assert_eq!(raised(&m.tick(T0.plus_ms(10), None)).len(), 1);
    m.on_paused(T0.plus_ms(20));
    let w = m.tick(T0.plus_ms(30), None);
    assert!(!w.iter().any(|w| w.cleared && matches!(w.issue, HealthIssue::PermissionMissing { .. })), "{w:?}");
}
