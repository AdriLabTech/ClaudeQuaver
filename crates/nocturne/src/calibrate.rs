//! Calibración de la compensación de audio: suena un metrónomo a 120 BPM, el jugador pulsa al
//! ritmo y la mediana de sus errores (pulsación − clic, en ms) es el desfase que hay que
//! compensar. Con `offset = error medio`, una pulsación igual de tardía acierta en el centro.

use crate::app::Screen;
use crate::audio::Audio;
use crate::jobs::{Jobs, NoticeKind};
use crate::paths;
use crate::settings::Settings;
use crate::theme::*;
use crate::ui;
use bevy::color::Mix;
use bevy::prelude::*;
use nocturne_core::Micros;
use std::sync::Arc;

const FIRST_CLICK_S: f64 = 2.0;
const BEAT_S: f64 = 0.5;
const TRACK_S: f64 = 60.0;
/// Pulsaciones mínimas para fiarse de la estimación.
pub const MIN_TAPS: usize = 8;
/// Las últimas pulsaciones que cuentan (el jugador se asienta tras las primeras).
const WINDOW: usize = 24;

/// Pista de clics estéreo a la frecuencia del dispositivo: un tono corto cada `BEAT_S`.
pub fn click_track(rate: u32) -> Arc<Vec<f32>> {
    let n = (TRACK_S * rate as f64) as usize;
    let mut buf = vec![0f32; n * 2];
    let len = (0.03 * rate as f64) as usize;
    let mut k = 0usize;
    loop {
        let start = ((FIRST_CLICK_S + k as f64 * BEAT_S) * rate as f64).round() as usize;
        if start + len >= n { break }
        for i in 0..len {
            let env = (-(i as f32) / (len as f32 * 0.25)).exp();
            let s = (std::f32::consts::TAU * 1500.0 * i as f32 / rate as f32).sin() * env * 0.8;
            buf[(start + i) * 2] = s;
            buf[(start + i) * 2 + 1] = s;
        }
        k += 1;
    }
    Arc::new(buf)
}

/// Error (ms) de una pulsación en `t_us` respecto al clic más cercano. `None` si cae antes del
/// primer clic o a más de 200 ms de cualquiera (no se sabe a cuál apuntaba).
pub fn tap_error_ms(t_us: i64) -> Option<f64> {
    let t = t_us as f64 / 1000.0;
    let first = FIRST_CLICK_S * 1000.0;
    if t < first - 200.0 { return None }
    let k = ((t - first) / (BEAT_S * 1000.0)).round().max(0.0);
    let e = t - (first + k * BEAT_S * 1000.0);
    (e.abs() <= 200.0).then_some(e)
}

/// Estimación robusta: (mediana, dispersión MAD) de las últimas pulsaciones; `None` si son pocas.
pub fn estimate(errors: &[f64]) -> Option<(f64, f64)> {
    if errors.len() < MIN_TAPS { return None }
    let recent = &errors[errors.len().saturating_sub(WINDOW)..];
    let median = |v: &mut Vec<f64>| { v.sort_by(|a, b| a.partial_cmp(b).unwrap()); let n = v.len(); if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 } };
    let m = median(&mut recent.to_vec());
    let mad = median(&mut recent.iter().map(|e| (e - m).abs()).collect());
    Some((m, mad))
}

#[derive(Resource, Default)]
struct Calib { taps: Vec<f64>, dirty: bool, error: Option<String> }

#[derive(Component)] struct Pulse;
#[derive(Component)] struct CalibDyn;

pub fn plugin(app: &mut App) {
    app.init_resource::<Calib>()
        .add_systems(OnEnter(Screen::Calibrate), (start, build).chain())
        .add_systems(OnExit(Screen::Calibrate), stop)
        .add_systems(Update, (tap_input, pulse, render).chain().run_if(in_state(Screen::Calibrate)));
}

fn start(mut c: ResMut<Calib>, mut audio: ResMut<Audio>, settings: Res<Settings>) {
    *c = Calib { dirty: true, ..Default::default() };
    if !audio.has_device() {
        c.error = Some(format!("Sin dispositivo de audio ({}). No se puede calibrar.", audio.device_error.clone().unwrap_or_default().trim_end_matches('.')));
        return;
    }
    audio.set_volume(settings.music_volume.max(0.45), settings.sfx_volume);
    let track = click_track(audio.sample_rate());
    audio.install(track, Micros::ZERO, true);
}

fn stop(mut audio: ResMut<Audio>, settings: Res<Settings>) {
    audio.pause();
    audio.unload();
    audio.set_volume(settings.music_volume, settings.sfx_volume);
}

fn build(mut commands: Commands, fonts: Res<Fonts>) {
    commands.spawn((DespawnOnExit(Screen::Calibrate),
        Node { flex_direction: FlexDirection::Column, padding: UiRect::new(px(96), px(96), px(64), px(40)), row_gap: px(26), ..ui::full_screen_root() }))
        .with_children(|root| {
            root.spawn(ui::spaced(&fonts, "CALIBRACIÓN", 38.0, TEXT, Weight::Light, 10.0));
            root.spawn(ui::text(&fonts, "Pulsa Espacio (o cualquier tecla de carril, o clic) justo cuando suene cada clic. Mantén el ritmo al menos 10 s.", 22.0, TEXT_DIM, Weight::Regular));
            root.spawn(Node { flex_grow: 1.0, column_gap: px(120), align_items: AlignItems::Center, ..default() }).with_children(|row| {
                row.spawn((Pulse, Node { width: px(260), height: px(260), flex_shrink: 0.0, border_radius: BorderRadius::MAX, ..default() }, BackgroundColor(SURFACE_2)));
                row.spawn((CalibDyn, Node { flex_direction: FlexDirection::Column, row_gap: px(10), ..default() }));
            });
            root.spawn(ui::divider(percent(100)));
            root.spawn(ui::text(&fonts, "Intro aplica la compensación   R reinicia   Esc cancela", 16.0, alpha(TEXT_DIM, 0.8), Weight::Regular));
        });
}

#[allow(clippy::too_many_arguments)]
fn tap_input(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut c: ResMut<Calib>,
    mut audio: ResMut<Audio>,
    mut settings: ResMut<Settings>,
    mut jobs: ResMut<Jobs>,
    mut next: ResMut<NextState<Screen>>,
) {
    if keys.just_pressed(KeyCode::Escape) { next.set(Screen::Settings); return }
    if keys.just_pressed(KeyCode::KeyR) && !keys.any_pressed(settings.keys()) { c.taps.clear(); c.dirty = true }
    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter) {
        match estimate(&c.taps) {
            Some((m, _)) => {
                settings.global_offset_ms = (m.round() as f32).clamp(-300.0, 300.0);
                match settings.save(&paths::settings_path()) {
                    Ok(()) => jobs.notice(format!("Compensación de audio: {:+.0} ms", settings.global_offset_ms), NoticeKind::Ok),
                    Err(e) => jobs.notice(format!("No se pudieron guardar los ajustes: {e}"), NoticeKind::Error),
                }
                next.set(Screen::Settings);
            }
            None => jobs.notice(format!("Hacen falta al menos {MIN_TAPS} pulsaciones al ritmo"), NoticeKind::Info),
        }
        return;
    }
    if c.error.is_some() { return }
    if audio.is_finished() { audio.restart() }
    let tapped = keys.just_pressed(KeyCode::Space) || keys.get_just_pressed().any(|k| settings.keys().contains(k)) || mouse.just_pressed(MouseButton::Left);
    if tapped {
        if let Some(e) = tap_error_ms(audio.clock().0) { c.taps.push(e); c.dirty = true }
    }
}

/// El círculo se ilumina en cada clic y se apaga en ~150 ms.
fn pulse(mut audio: ResMut<Audio>, settings: Res<Settings>, c: Res<Calib>, mut q: Query<&mut BackgroundColor, With<Pulse>>) {
    if c.error.is_some() { return }
    let t = audio.clock().as_secs() - FIRST_CLICK_S;
    let k = if t < 0.0 { 0.0 } else { let since = t % BEAT_S; (1.0 - since / 0.15).clamp(0.0, 1.0) as f32 };
    let k = if settings.motion_reduced() { k.round() } else { k };
    for mut bg in &mut q { bg.0 = SURFACE_2.mix(&WHITE, k * 0.9) }
}

fn render(mut commands: Commands, mut c: ResMut<Calib>, fonts: Res<Fonts>, root: Query<Entity, With<CalibDyn>>) {
    if !c.dirty { return }
    let Ok(root) = root.single() else { return };
    c.dirty = false;
    commands.entity(root).despawn_children();
    let c = &*c;
    commands.entity(root).with_children(|p| {
        if let Some(e) = &c.error {
            p.spawn(ui::text(&fonts, e.clone(), 24.0, DANGER, Weight::Regular));
            return;
        }
        match estimate(&c.taps) {
            Some((m, mad)) => {
                p.spawn(ui::text(&fonts, format!("{:+.0} ms", m), 84.0, WHITE, Weight::Light));
                p.spawn(ui::text(&fonts, if m > 3.0 { "sueles pulsar tarde: la compensación sube las notas" } else if m < -3.0 { "sueles pulsar pronto: la compensación adelanta las notas" } else { "estás casi centrado" }, 20.0, TEXT, Weight::Regular));
                p.spawn(ui::text(&fonts, format!("{} pulsaciones · dispersión ±{:.0} ms{}", c.taps.len(), mad, if mad > 35.0 { " (irregular: sigue un rato más)" } else { "" }), 18.0, TEXT_DIM, Weight::Regular));
            }
            None => {
                p.spawn(ui::text(&fonts, "…", 84.0, TEXT_DIM, Weight::Light));
                p.spawn(ui::text(&fonts, format!("{} de {MIN_TAPS} pulsaciones mínimas", c.taps.len()), 20.0, TEXT_DIM, Weight::Regular));
            }
        }
        let recent = &c.taps[c.taps.len().saturating_sub(WINDOW)..];
        ui::spawn_histogram(p, &fonts, &crate::db::histogram(&recent.iter().map(|e| *e as f32).collect::<Vec<_>>()), 560.0, 120.0);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clicks_land_on_the_beat_grid() {
        let rate = 48_000;
        let t = click_track(rate);
        assert_eq!(t.len(), (TRACK_S * rate as f64) as usize * 2);
        // Silencio antes del primer clic y energía justo en cada pulso.
        let at = |s: f64| (s * rate as f64) as usize * 2;
        assert!(t[..at(FIRST_CLICK_S)].iter().all(|v| *v == 0.0));
        for k in 0..10 {
            let a = at(FIRST_CLICK_S + k as f64 * BEAT_S);
            assert!(t[a..a + 400].iter().any(|v| v.abs() > 0.1), "clic {k}");
            let quiet = at(FIRST_CLICK_S + k as f64 * BEAT_S + 0.2);
            assert!(t[quiet..quiet + 400].iter().all(|v| *v == 0.0));
        }
        assert!(t.iter().all(|v| v.abs() <= 1.0));
    }

    #[test]
    fn tap_errors_are_signed_and_bounded() {
        assert_eq!(tap_error_ms(2_000_000), Some(0.0));
        assert!((tap_error_ms(2_030_000).unwrap() - 30.0).abs() < 1e-9, "tarde = positivo");
        assert!((tap_error_ms(2_470_000).unwrap() + 30.0).abs() < 1e-9, "pronto = negativo, respecto al clic de 2,5 s");
        assert!((tap_error_ms(3_500_000 + 12_000).unwrap() - 12.0).abs() < 1e-9);
        assert_eq!(tap_error_ms(1_000_000), None, "antes del primer clic");
        assert!((tap_error_ms(1_850_000).unwrap() + 150.0).abs() < 1e-9, "algo antes del primer clic vale");
        assert_eq!(tap_error_ms(2_250_000), None, "a medio camino entre dos clics: ambiguo");
    }

    #[test]
    fn estimate_is_robust_to_outliers_and_needs_enough_taps() {
        assert_eq!(estimate(&[10.0; MIN_TAPS - 1]), None);
        let mut taps: Vec<f64> = vec![22.0, 18.0, 25.0, 20.0, 21.0, 19.0, 23.0, 20.0, 24.0, 22.0];
        taps.push(-180.0); // un despiste
        taps.push(190.0);
        let (m, mad) = estimate(&taps).unwrap();
        assert!((m - 21.5).abs() <= 2.0, "{m}");
        assert!(mad < 6.0, "{mad}");
    }

    #[test]
    fn only_recent_taps_count() {
        let mut taps = vec![100.0; 40];
        taps.extend(vec![5.0; WINDOW]);
        let (m, _) = estimate(&taps).unwrap();
        assert_eq!(m, 5.0);
    }

    #[test]
    fn applying_the_estimate_recentres_a_late_player() {
        // El jugador pulsa 30 ms tarde; con offset = 30 el error de juicio medio es 0.
        let errors: Vec<f64> = (0..16).map(|i| 30.0 + (i % 3) as f64 - 1.0).collect();
        let (offset, _) = estimate(&errors).unwrap();
        let judged: Vec<f64> = errors.iter().map(|e| e - offset).collect();
        let mean = judged.iter().sum::<f64>() / judged.len() as f64;
        assert!(mean.abs() < 1.0, "{mean}");
    }
}
