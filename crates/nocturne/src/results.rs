//! Pantalla de resultados.
use crate::app::{ReturnTo, Screen};
use crate::fx::{self, CountUp};
use crate::game::{LastResult, PlayRequest};
use crate::theme::*;
use crate::ui::{self, MenuNav};
use bevy::prelude::*;
use nocturne_core::scoring::Judgement;

const ID_RETRY: u32 = 1;
const ID_BACK: u32 = 2;

pub fn plugin(app: &mut App) {
    app.add_systems(OnEnter(Screen::Results), build)
        .add_systems(Update, (ui::menu_input, ui::menu_visuals, act).chain().run_if(in_state(Screen::Results)));
}

fn build(mut commands: Commands, r: Res<LastResult>, fonts: Res<Fonts>, mut nav: ResMut<MenuNav>, back: Res<ReturnTo>, mut accent: ResMut<fx::Accent>) {
    nav.reset(2);
    accent.target = fx::accent_for(&r.map.audio_fingerprint);
    let sc = &r.score;
    let hist = crate::db::histogram(&sc.errors_ms);
    commands.spawn((DespawnOnExit(Screen::Results), ui::full_screen_root())).with_children(|p| {
        p.spawn(Node { width: percent(100), height: percent(100), padding: UiRect::axes(px(120), px(80)), column_gap: px(100), ..default() }).with_children(|row| {
            // Izquierda: nota y puntuación
            row.spawn(Node { flex_direction: FlexDirection::Column, width: px(560), row_gap: px(8), ..default() }).with_children(|c| {
                c.spawn((ui::spaced(&fonts, if r.failed { "FALLO" } else { "RESULTADOS" }, 20.0, if r.failed { DANGER } else { TEXT_DIM }, Weight::Medium, 8.0), fx::appear(0.0, Vec2::new(-40.0, 0.0))));
                c.spawn((ui::text(&fonts, &r.map.title, 44.0, TEXT, Weight::Medium), fx::appear(0.08, Vec2::new(-40.0, 0.0))));
                c.spawn((ui::text(&fonts, if r.map.artist.is_empty() { r.map.difficulty.label().to_string() } else { format!("{} · {}", r.map.difficulty.label(), r.map.artist) }, 20.0, TEXT_DIM, Weight::Regular), fx::appear(0.14, Vec2::new(-40.0, 0.0))));
                c.spawn(Node { height: px(24), ..default() });
                // La nota entra desde abajo y brilla (color por encima de 1 → resplandor).
                c.spawn((ui::text(&fonts, sc.grade(), 190.0, Color::srgb(1.7, 1.7, 1.75), Weight::Light), fx::appear(0.25, Vec2::new(0.0, 46.0))));
                c.spawn((ui::text(&fonts, "0000000", 64.0, TEXT, Weight::Light), fx::appear(0.4, Vec2::new(0.0, 20.0)),
                    CountUp::new(sc.score() as f64, 0.5, 1.3, |v| format!("{:07}", v.round() as u64))));
                c.spawn((ui::text(&fonts, format!("{:.2} %   ·   combo máx. {}", sc.accuracy(), sc.max_combo), 24.0, TEXT_DIM, Weight::Regular), fx::appear(0.6, Vec2::new(0.0, 14.0))));
                c.spawn(Node { height: px(36), ..default() });
                c.spawn((ui::menu_item(&fonts, ID_RETRY, 0, "Reintentar", None, 32.0), fx::appear(0.85, Vec2::new(-40.0, 0.0))));
                c.spawn((ui::menu_item(&fonts, ID_BACK, 1, back.label(), None, 32.0), fx::appear(0.93, Vec2::new(-40.0, 0.0))));
            });
            // Derecha: recuentos y distribución temporal
            row.spawn(Node { flex_direction: FlexDirection::Column, flex_grow: 1.0, row_gap: px(10), padding: UiRect::top(px(60)), ..default() }).with_children(|c| {
                for j in Judgement::ALL {
                    c.spawn((Node { justify_content: JustifyContent::SpaceBetween, width: px(420), ..default() }, fx::appear(0.35 + 0.07 * j.index() as f32, Vec2::new(40.0, 0.0)))).with_children(|l| {
                        l.spawn(ui::text(&fonts, j.label(), 24.0, judgement_color(j), Weight::Regular));
                        l.spawn((ui::text(&fonts, "0", 24.0, TEXT, Weight::Regular),
                            CountUp::new(sc.counts[j.index()] as f64, 0.5 + 0.07 * j.index() as f32, 0.9, |v| format!("{}", v.round() as u64))));
                    });
                }
                c.spawn(Node { height: px(30), ..default() });
                let early = sc.errors_ms.iter().filter(|e| **e < 0.0).count();
                let late = sc.errors_ms.len() - early;
                c.spawn(ui::text(&fonts, "Anticipación / retraso", 18.0, TEXT_DIM, Weight::Medium));
                match sc.mean_error_ms() {
                    Some(m) => c.spawn(ui::text(&fonts, format!("media {:+.1} ms   ·   {} antes   ·   {} después", m, early, late), 20.0, TEXT, Weight::Regular)),
                    None => c.spawn(ui::text(&fonts, "sin aciertos registrados", 20.0, TEXT_DIM, Weight::Regular)),
                };
                c.spawn((Node { flex_direction: FlexDirection::Column, ..default() }, fx::appear(0.95, Vec2::new(0.0, 24.0)))).with_children(|h| ui::spawn_histogram(h, &fonts, &hist, 540.0, 150.0));
            });
        });
    });
}

fn act(mut nav: ResMut<MenuNav>, mut next: ResMut<NextState<Screen>>, r: Option<Res<LastResult>>, mut commands: Commands, keys: Res<ButtonInput<KeyCode>>, back: Res<ReturnTo>) {
    if keys.just_pressed(KeyCode::Escape) { next.set(back.0); return }
    match nav.activated.take() {
        Some(ID_RETRY) => { if let Some(r) = r { commands.insert_resource(r.request.clone() as PlayRequest); next.set(Screen::Playing) } }
        Some(ID_BACK) => next.set(back.0),
        _ => {}
    }
}
