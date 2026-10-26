//! Estadísticas: totales, sincronización acumulada (histograma de errores) y partidas recientes.
//! Todo sale de la base de datos; con la base vacía no se inventa nada.

use crate::app::{AppDb, Screen};
use crate::db::{now, ScoreRow};
use crate::theme::*;
use crate::ui;
use bevy::prelude::*;
use nocturne_core::scoring::grade_for;

#[derive(Resource, Default)]
struct StatsData {
    games: i64,
    played_ms: i64,
    mean_accuracy: Option<f64>,
    best_combo: Option<i64>,
    hist: Vec<u32>,
    recent: Vec<(ScoreRow, String)>,
}

pub fn plugin(app: &mut App) {
    app.init_resource::<StatsData>()
        .add_systems(OnEnter(Screen::Stats), build)
        .add_systems(Update, back.run_if(in_state(Screen::Stats)));
}

fn back(keys: Res<ButtonInput<KeyCode>>, mouse: Res<ButtonInput<MouseButton>>, mut next: ResMut<NextState<Screen>>, mut clicks: ResMut<ui::Clicks>) {
    if keys.just_pressed(KeyCode::Escape) || keys.just_pressed(KeyCode::Backspace) || clicks.take(1) { next.set(Screen::Home) }
    let _ = mouse;
}

/// Sincronización típica a partir del histograma agregado: media (ms) de los casilleros centrales,
/// sin los dos extremos (que acumulan todo lo que cae fuera de ±150 ms). `None` con pocas pulsaciones.
pub fn timing_bias_ms(hist: &[u32]) -> Option<f64> {
    if hist.len() < 4 { return None }
    let inner = &hist[1..hist.len() - 1];
    let n: u32 = inner.iter().sum();
    if n < 30 { return None }
    let center = hist.len() as f64 / 2.0 * 10.0;
    let sum: f64 = inner.iter().enumerate().map(|(i, c)| ((i + 1) as f64 * 10.0 + 5.0 - center) * *c as f64).sum();
    Some(sum / n as f64)
}

pub fn ago(now: i64, t: i64) -> String {
    let d = (now - t).max(0);
    match d {
        0..=59 => "hace un momento".into(),
        60..=3599 => format!("hace {} min", d / 60),
        3600..=86399 => format!("hace {} h", d / 3600),
        _ => format!("hace {} d", d / 86400),
    }
}

fn build(mut commands: Commands, mut data: ResMut<StatsData>, db: Res<AppDb>, fonts: Res<Fonts>) {
    *data = StatsData::default();
    if let Ok(db) = db.0.lock() {
        if let Ok((n, ms, acc, combo)) = db.totals() { data.games = n; data.played_ms = ms; data.mean_accuracy = acc; data.best_combo = combo }
        data.hist = db.error_histogram().unwrap_or_default();
        let recent = db.recent(9).unwrap_or_default();
        data.recent = recent.into_iter().map(|r| {
            let title = db.song_by_fingerprint(&r.fingerprint).ok().flatten().map_or("(canción eliminada)".to_string(), |s| s.title);
            (r, title)
        }).collect();
    }
    let d = &*data;
    commands.spawn((DespawnOnExit(Screen::Stats), crate::fx::appear(0.0, Vec2::new(0.0, 22.0)),
        Node { flex_direction: FlexDirection::Column, padding: UiRect::new(px(96), px(96), px(64), px(40)), row_gap: px(30), ..ui::full_screen_root() }))
        .with_children(|root| {
            root.spawn(ui::spaced(&fonts, "ESTADÍSTICAS", 38.0, TEXT, Weight::Light, 10.0));
            if d.games == 0 {
                root.spawn(Node { flex_grow: 1.0, flex_direction: FlexDirection::Column, justify_content: JustifyContent::Center, row_gap: px(12), ..default() }).with_children(|c| {
                    c.spawn(ui::text(&fonts, "Aún no has jugado ninguna partida", 44.0, TEXT, Weight::Light));
                    c.spawn(ui::text(&fonts, "Cuando termines una canción aparecerán aquí tu precisión, tu sincronización y tus últimas partidas.", 22.0, TEXT_DIM, Weight::Regular));
                });
            } else {
                root.spawn(Node { column_gap: px(90), ..default() }).with_children(|row| {
                    let mut big = |label: &str, value: String| {
                        row.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(4), ..default() }).with_children(|c| {
                            c.spawn(ui::text(&fonts, value, 58.0, WHITE, Weight::Light));
                            c.spawn(ui::text(&fonts, label, 18.0, TEXT_DIM, Weight::Regular));
                        });
                    };
                    big("partidas", d.games.to_string());
                    big("tiempo jugado", format!("{}:{:02} h", d.played_ms / 3_600_000, d.played_ms / 60_000 % 60));
                    big("precisión media", d.mean_accuracy.map_or("—".into(), |a| format!("{a:.2} %")));
                    big("mejor combo", d.best_combo.map_or("—".into(), |c| c.to_string()));
                });
                root.spawn(Node { column_gap: px(90), flex_grow: 1.0, ..default() }).with_children(|row| {
                    row.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(8), width: px(600), ..default() }).with_children(|c| {
                        c.spawn(ui::text(&fonts, "TU SINCRONIZACIÓN", 16.0, TEXT_DIM, Weight::Medium));
                        ui::spawn_histogram(c, &fonts, &d.hist, 600.0, 190.0);
                        match timing_bias_ms(&d.hist) {
                            Some(b) if b.abs() >= 5.0 => {
                                c.spawn(ui::text(&fonts, format!("Sueles pulsar {:.0} ms {}.", b.abs(), if b > 0.0 { "tarde" } else { "pronto" }), 22.0, TEXT, Weight::Regular));
                                c.spawn(ui::text(&fonts, format!("Prueba una compensación de audio de {:+.0} ms en Ajustes, o usa la calibración.", b), 18.0, TEXT_DIM, Weight::Regular));
                            }
                            Some(_) => { c.spawn(ui::text(&fonts, "Tu sincronización está centrada.", 22.0, TEXT, Weight::Regular)); }
                            None => { c.spawn(ui::text(&fonts, "Hacen falta más partidas para estimar tu sincronización.", 18.0, TEXT_DIM, Weight::Regular)); }
                        }
                    });
                    row.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(4), flex_grow: 1.0, ..default() }).with_children(|c| {
                        c.spawn(ui::text(&fonts, "PARTIDAS RECIENTES", 16.0, TEXT_DIM, Weight::Medium));
                        c.spawn(ui::gap(6.0));
                        let t0 = now();
                        for (r, title) in &d.recent {
                            c.spawn(Node { width: percent(100), justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::Center, padding: UiRect::vertical(px(6)), ..default() }).with_children(|l| {
                                l.spawn(Node { flex_direction: FlexDirection::Column, ..default() }).with_children(|n| {
                                    n.spawn(ui::text(&fonts, ui::ellipsize(title, 34), 22.0, TEXT, Weight::Regular));
                                    n.spawn(ui::text(&fonts, format!("{} · {}{}", r.difficulty, ago(t0, r.played_at), if r.failed { " · fallo" } else { "" }), 15.0, if r.failed { DANGER } else { TEXT_DIM }, Weight::Regular));
                                });
                                l.spawn(Node { align_items: AlignItems::Center, column_gap: px(24), ..default() }).with_children(|n| {
                                    n.spawn(ui::text(&fonts, format!("{:.2} %", r.accuracy), 18.0, TEXT_DIM, Weight::Regular));
                                    n.spawn(ui::text(&fonts, format!("{:07}", r.score), 20.0, TEXT, Weight::Regular));
                                    n.spawn(ui::text(&fonts, grade_for(r.accuracy), 30.0, WHITE, Weight::Light));
                                });
                            });
                        }
                    });
                });
            }
            root.spawn(ui::divider(percent(100)));
            root.spawn(Node { justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::Center, ..default() }).with_children(|b| {
                b.spawn(ui::text(&fonts, "Esc volver", 16.0, alpha(TEXT_DIM, 0.8), Weight::Regular));
                b.spawn(ui::pill(&fonts, 1, "Volver · Esc"));
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bias_is_signed_and_ignores_outlier_bins() {
        let mut h = vec![0u32; 30];
        h[15] = 40; // +0…+10 ms → +5 ms
        h[16] = 40; // +10…+20 ms → +15 ms
        h[0] = 500; // lejanos: no cuentan
        h[29] = 500;
        assert!((timing_bias_ms(&h).unwrap() - 10.0).abs() < 1e-9);
        let mut early = vec![0u32; 30];
        early[12] = 100; // −30…−20 ms
        assert!((timing_bias_ms(&early).unwrap() + 25.0).abs() < 1e-9);
    }

    #[test]
    fn bias_needs_enough_data() {
        assert_eq!(timing_bias_ms(&vec![0; 30]), None);
        let mut h = vec![0u32; 30];
        h[15] = 29;
        assert_eq!(timing_bias_ms(&h), None);
        h[15] = 30;
        assert!(timing_bias_ms(&h).is_some());
        assert_eq!(timing_bias_ms(&[]), None);
    }

    #[test]
    fn relative_time() {
        assert_eq!(ago(1000, 990), "hace un momento");
        assert_eq!(ago(1000, 1000 - 300), "hace 5 min");
        assert_eq!(ago(100_000, 100_000 - 7300), "hace 2 h");
        assert_eq!(ago(1_000_000, 1_000_000 - 3 * 86400 - 5), "hace 3 d");
        assert_eq!(ago(10, 20), "hace un momento", "un reloj desajustado no da tiempos negativos");
    }
}
