//! Acerca de: versión, cómo funciona, controles, licencias y datos útiles para diagnosticar problemas.

use crate::app::{AppDb, Screen};
use crate::audio::Audio;
use crate::paths;
use crate::theme::*;
use crate::ui;
use bevy::prelude::*;

pub fn plugin(app: &mut App) {
    app.add_systems(OnEnter(Screen::About), build).add_systems(Update, back.run_if(in_state(Screen::About)));
}

fn back(keys: Res<ButtonInput<KeyCode>>, mut next: ResMut<NextState<Screen>>, mut clicks: ResMut<ui::Clicks>) {
    if keys.just_pressed(KeyCode::Escape) || keys.just_pressed(KeyCode::Backspace) || keys.just_pressed(KeyCode::Enter) || clicks.take(1) { next.set(Screen::Home) }
}

const HOW: [&str; 4] = [
    "1. Importas una canción (MP3, WAV, FLAC, OGG, M4A/AAC o el audio de un vídeo).",
    "2. Nocturne mide el espectro, detecta los golpes, estima el tempo y la cuadrícula rítmica.",
    "3. Genera seis mapas, de Easy a Extreme, siempre iguales para el mismo audio.",
    "4. Juegas, y si algo no te convence lo corriges en el editor de mapas.",
];

const CONTROLS: [(&str, &str); 6] = [
    ("D F J K", "los cuatro carriles (se pueden cambiar en Ajustes)"),
    ("Esc", "pausa en partida · volver en los menús"),
    ("↑ ↓ ← →", "moverse por los menús · Intro elige"),
    ("F11", "pantalla completa"),
    ("Arrastrar un archivo", "lo importa desde cualquier pantalla"),
    ("Editor", "clic: nota · arrastrar: nota larga · clic derecho: borrar"),
];

const CREDITS: [&str; 3] = [
    "Licencia GPL-3.0-or-later.",
    "Tipografía Space Grotesk (SIL Open Font License 1.1).",
    "Construido con Bevy, cpal, symphonia, rustfft y rusqlite.",
];

fn build(mut commands: Commands, fonts: Res<Fonts>, audio: Res<Audio>, db: Res<AppDb>) {
    let (songs, schema) = db.0.lock().map(|d| (d.songs().map_or(0, |s| s.len()), d.schema_version())).unwrap_or((0, 0));
    let device = match audio.device_info() {
        Some(i) => format!("{} · {} Hz · {} canales · {} ms de latencia · {}", ui::ellipsize(&i.name, 36), i.sample_rate, i.channels, audio.latency_ms().round(), i.host),
        None => format!("sin dispositivo ({})", audio.device_error.clone().unwrap_or_else(|| "desconocido".into()).trim_end_matches('.')),
    };
    let rows = [
        ("Versión", format!("{} {}", APP_NAME, env!("CARGO_PKG_VERSION"))),
        ("Mapas y datos", paths::redact(&paths::data_dir())),
        ("Ajustes", paths::redact(&paths::settings_path())),
        ("Biblioteca", format!("{songs} canciones · esquema de base de datos v{schema}")),
        ("Audio", device),
        ("Formato de mapa", format!("v{} · analizador v{}", nocturne_core::beatmap::FORMAT_VERSION, nocturne_core::analysis::ANALYZER_VERSION)),
    ];
    commands.spawn((DespawnOnExit(Screen::About),
        Node { flex_direction: FlexDirection::Column, padding: UiRect::new(px(96), px(96), px(64), px(40)), row_gap: px(30), ..ui::full_screen_root() }))
        .with_children(|root| {
            root.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(6), ..default() }).with_children(|h| {
                h.spawn(ui::spaced(&fonts, APP_NAME.to_uppercase(), 76.0, TEXT, Weight::Light, 18.0));
                h.spawn(ui::text(&fonts, "Tu música, convertida en mapa de ritmo.", 22.0, TEXT_DIM, Weight::Regular));
            });
            root.spawn(Node { column_gap: px(100), flex_grow: 1.0, ..default() }).with_children(|cols| {
                cols.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(12), width: px(780), ..default() }).with_children(|c| {
                    c.spawn(ui::text(&fonts, "CÓMO FUNCIONA", 16.0, TEXT_DIM, Weight::Medium));
                    for l in HOW { c.spawn(ui::text(&fonts, l, 20.0, TEXT, Weight::Regular)); }
                    c.spawn(ui::gap(18.0));
                    c.spawn(ui::text(&fonts, "CONTROLES", 16.0, TEXT_DIM, Weight::Medium));
                    for (k, d) in CONTROLS {
                        c.spawn(Node { column_gap: px(20), ..default() }).with_children(|r| {
                            r.spawn((Node { width: px(210), flex_shrink: 0.0, ..default() }, children![ui::text(&fonts, k, 20.0, WHITE, Weight::Medium)]));
                            r.spawn(ui::text(&fonts, d, 20.0, TEXT_DIM, Weight::Regular));
                        });
                    }
                });
                cols.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(12), flex_grow: 1.0, ..default() }).with_children(|c| {
                    c.spawn(ui::text(&fonts, "TU INSTALACIÓN", 16.0, TEXT_DIM, Weight::Medium));
                    for (k, v) in rows {
                        c.spawn(Node { flex_direction: FlexDirection::Column, ..default() }).with_children(|r| {
                            r.spawn(ui::text(&fonts, k, 15.0, TEXT_DIM, Weight::Regular));
                            r.spawn(ui::text(&fonts, v, 19.0, TEXT, Weight::Regular));
                        });
                    }
                    c.spawn(ui::gap(14.0));
                    for l in CREDITS { c.spawn(ui::text(&fonts, l, 16.0, alpha(TEXT_DIM, 0.85), Weight::Regular)); }
                });
            });
            root.spawn(ui::divider(percent(100)));
            root.spawn(Node { justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::Center, ..default() }).with_children(|b| {
                b.spawn(ui::text(&fonts, "Esc volver", 16.0, alpha(TEXT_DIM, 0.8), Weight::Regular));
                b.spawn(ui::pill(&fonts, 1, "Volver · Esc"));
            });
        });
}
