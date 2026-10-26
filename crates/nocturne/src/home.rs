//! Pantalla de inicio: composición asimétrica con un eclipse abstracto.
use crate::app::{AppDb, Screen};
use crate::jobs::{Jobs, NoticeKind};
use crate::settings::Settings;
use crate::theme::*;
use crate::ui::{self, MenuNav};
use bevy::prelude::*;

const ID_LIBRARY: u32 = 1;
const ID_EDITOR: u32 = 2;
const ID_STATS: u32 = 3;
const ID_SETTINGS: u32 = 4;
const ID_ABOUT: u32 = 5;
const ID_QUIT: u32 = 6;

#[derive(Component)] struct Eclipse;
#[derive(Component)] struct JobLine;

pub fn plugin(app: &mut App) {
    app.add_systems(OnEnter(Screen::Home), build)
        .add_systems(Update, (ui::menu_input, ui::menu_visuals, act, spin, job_line).chain().run_if(in_state(Screen::Home)));
}

fn build(mut commands: Commands, fonts: Res<Fonts>, mut nav: ResMut<MenuNav>, db: Res<AppDb>,
         mut meshes: ResMut<Assets<Mesh>>, mut mats: ResMut<Assets<ColorMaterial>>) {
    nav.reset(6);
    // Eclipse: disco claro, disco negro desplazado → creciente; corona fina alrededor.
    let disc = meshes.add(Circle::new(1.0).mesh().resolution(180));
    let ring = meshes.add(Annulus::new(0.995, 1.0).mesh().resolution(240));
    let (c0, c1) = (480.0, 20.0);
    commands.spawn((DespawnOnExit(Screen::Home), Eclipse, Transform::from_xyz(c0, c1, 0.0), Visibility::default())).with_children(|p| {
        p.spawn((Mesh2d(disc.clone()), MeshMaterial2d(mats.add(alpha(SURFACE_2, 1.0))), Transform::from_xyz(0.0, 0.0, 0.0).with_scale(Vec3::splat(330.0))));
        p.spawn((Mesh2d(disc.clone()), MeshMaterial2d(mats.add(TEXT)), Transform::from_xyz(0.0, 0.0, 0.1).with_scale(Vec3::splat(262.0))));
        p.spawn((Mesh2d(disc.clone()), MeshMaterial2d(mats.add(BG)), Transform::from_xyz(-34.0, 18.0, 0.2).with_scale(Vec3::splat(256.0))));
        for (r, a) in [(300.0, 0.30), (330.0, 0.16), (390.0, 0.07)] {
            p.spawn((Mesh2d(ring.clone()), MeshMaterial2d(mats.add(alpha(TEXT_DIM, a))), Transform::from_xyz(0.0, 0.0, 0.05).with_scale(Vec3::splat(r))));
        }
    });
    let last = db.0.lock().ok().and_then(|d| d.last_played_song().ok().flatten());
    let recent = db.0.lock().ok().and_then(|d| d.recent(1).ok()).and_then(|v| v.into_iter().next());
    commands.spawn((DespawnOnExit(Screen::Home), ui::full_screen_root())).with_children(|p| {
        p.spawn(Node { position_type: PositionType::Absolute, left: px(140), top: px(150), flex_direction: FlexDirection::Column, ..default() }).with_children(|c| {
            c.spawn(ui::spaced(&fonts, APP_NAME.to_uppercase(), 104.0, TEXT, Weight::Light, 22.0));
            c.spawn(Node { margin: UiRect::left(px(6)), ..default() }).with_children(|t| { t.spawn(ui::text(&fonts, "Tu música, convertida en mapa de ritmo.", 22.0, TEXT_DIM, Weight::Regular)); });
            c.spawn(Node { height: px(64), ..default() });
            c.spawn(ui::menu_item(&fonts, ID_LIBRARY, 0, "Biblioteca", Some("Importa una canción y juega"), 40.0));
            c.spawn(ui::menu_item(&fonts, ID_EDITOR, 1, "Editor de mapas", None, 30.0));
            c.spawn(ui::menu_item(&fonts, ID_STATS, 2, "Estadísticas", None, 30.0));
            c.spawn(ui::menu_item(&fonts, ID_SETTINGS, 3, "Ajustes", None, 30.0));
            c.spawn(ui::menu_item(&fonts, ID_ABOUT, 4, "Acerca de", None, 30.0));
            c.spawn(ui::menu_item(&fonts, ID_QUIT, 5, "Salir", None, 30.0));
        });
        // Estado inferior: última canción, último resultado, análisis en curso.
        p.spawn(Node { position_type: PositionType::Absolute, left: px(148), bottom: px(56), flex_direction: FlexDirection::Column, row_gap: px(6), ..default() }).with_children(|c| {
            match &last {
                Some(s) => { c.spawn(ui::text(&fonts, format!("Última canción · {}", s.title), 18.0, TEXT_DIM, Weight::Regular)); }
                None => { c.spawn(ui::text(&fonts, "Aún no has jugado ninguna canción.", 18.0, TEXT_DIM, Weight::Regular)); }
            }
            if let Some(r) = &recent {
                c.spawn(ui::text(&fonts, format!("Último resultado · {} {} · {:.2} % · {} combo", r.difficulty, if r.failed { "(fallo)" } else { "" }, r.accuracy, r.max_combo), 18.0, TEXT_DIM, Weight::Regular));
            }
            c.spawn((JobLine, ui::text(&fonts, "", 18.0, TEXT, Weight::Regular)));
        });
        p.spawn(Node { position_type: PositionType::Absolute, right: px(48), bottom: px(40), ..default() })
            .with_children(|c| { c.spawn(ui::text(&fonts, "↑↓ mover   Enter elegir   Esc salir", 16.0, alpha(TEXT_DIM, 0.7), Weight::Regular)); });
    });
}

fn act(mut nav: ResMut<MenuNav>, mut next: ResMut<NextState<Screen>>, mut exit: MessageWriter<AppExit>, keys: Res<ButtonInput<KeyCode>>, mut jobs: ResMut<Jobs>) {
    if keys.just_pressed(KeyCode::Escape) { exit.write(AppExit::Success); return }
    match nav.activated.take() {
        Some(ID_LIBRARY) => next.set(Screen::Library),
        Some(ID_EDITOR) => {
            // El editor trabaja sobre el mapa de una canción: se elige en la biblioteca.
            jobs.notice("Elige una canción y pulsa E para editar su mapa", NoticeKind::Info);
            next.set(Screen::Library)
        }
        Some(ID_STATS) => next.set(Screen::Stats),
        Some(ID_SETTINGS) => next.set(Screen::Settings),
        Some(ID_ABOUT) => next.set(Screen::About),
        Some(ID_QUIT) => { exit.write(AppExit::Success); }
        _ => {}
    }
}

fn spin(time: Res<Time>, settings: Res<Settings>, mut q: Query<&mut Transform, With<Eclipse>>) {
    if settings.reduce_motion { return }
    for mut t in &mut q { t.rotation = Quat::from_rotation_z((time.elapsed_secs() * 0.04).sin() * 0.12); }
}

fn job_line(jobs: Res<Jobs>, mut q: Query<&mut Text, With<JobLine>>) {
    for mut t in &mut q {
        **t = match jobs.summary() { Some(s) => s, None => String::new() };
    }
}
