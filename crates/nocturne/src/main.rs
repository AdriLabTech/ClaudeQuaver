mod app;
mod audio;
mod db;
mod demo;
mod game;
mod home;
mod import;
mod jobs;
mod paths;
mod probe;
mod results;
mod settings;
mod stubs;
mod theme;
mod toast;
mod ui;

use app::{AppDb, Screen};
use bevy::camera::ScalingMode;
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::window::{MonitorSelection, PresentMode, PrimaryWindow, WindowMode};
use settings::Settings;
use std::sync::Mutex;

/// Opciones de línea de órdenes para verificación y automatización.
#[derive(Resource, Default, Clone)]
struct Cli {
    screen: Option<String>,
    demo: bool,
    autoplay: bool,
    play: Option<std::path::PathBuf>,
    shot: Option<(f32, String)>,
    exit_after: Option<f32>,
}

fn parse_cli() -> Cli {
    let a: Vec<String> = std::env::args().collect();
    let mut c = Cli::default();
    let mut i = 1;
    while i < a.len() {
        match a[i].as_str() {
            "--demo" => c.demo = true,
            "--autoplay" => c.autoplay = true,
            "--screen" => { i += 1; c.screen = a.get(i).cloned() }
            "--play" => { i += 1; c.play = a.get(i).map(Into::into) }
            "--shot" => { c.shot = a.get(i + 1).and_then(|s| s.parse().ok()).zip(a.get(i + 2).cloned()); i += 2 }
            "--exit-after" => { i += 1; c.exit_after = a.get(i).and_then(|s| s.parse().ok()) }
            _ => {}
        }
        i += 1;
    }
    c
}

fn main() {
    if std::env::args().any(|a| a == "--probe-audio") { probe::run(None); return }
    let cli = parse_cli();
    let settings = Settings::load(&paths::settings_path());
    let (db, db_warn) = db::Db::open(&paths::db_path());
    let _ = db.recover_interrupted();
    let audio = audio::Audio::new(settings.audio_device.as_deref());
    let mut jobs = jobs::Jobs::default();
    if let Some(w) = db_warn { jobs.notice(w, jobs::NoticeKind::Error) }
    if let Some(e) = &audio.device_error { jobs.notice(format!("Audio no disponible: {e}. Se usará un reloj silencioso."), jobs::NoticeKind::Error) }
    for w in &settings.warnings { jobs.notice(w.clone(), jobs::NoticeKind::Info) }

    let window = Window {
        title: theme::APP_NAME.into(), name: Some("nocturne".into()),
        resolution: (1600u32, 900u32).into(), min_width: 960.0, min_height: 540.0,
        present_mode: if settings.vsync { PresentMode::AutoVsync } else { PresentMode::AutoNoVsync },
        mode: if settings.fullscreen { WindowMode::BorderlessFullscreen(MonitorSelection::Primary) } else { WindowMode::Windowed },
        ..default()
    };
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin { primary_window: Some(window), ..default() }))
        .insert_resource(ClearColor(theme::BG))
        .insert_resource(settings)
        .insert_resource(AppDb(Mutex::new(db)))
        .insert_resource(audio)
        .insert_resource(jobs)
        .insert_resource(cli)
        .init_state::<Screen>()
        .add_systems(Startup, (theme::load_fonts, setup_camera, boot).chain())
        .add_systems(Update, (scale_ui, jobs::poll, frame_limit_and_fullscreen, cli_shot))
        .add_plugins((home::plugin, game::plugin, results::plugin, toast::plugin, stubs::plugin))
        .run();
}

fn setup_camera(mut commands: Commands) {
    commands.spawn((Camera2d, Projection::Orthographic(OrthographicProjection {
        scaling_mode: ScalingMode::FixedVertical { viewport_height: theme::DESIGN_H }, ..OrthographicProjection::default_2d() })));
}

/// Primera pantalla según la línea de órdenes.
fn boot(mut commands: Commands, cli: Res<Cli>, mut next: ResMut<NextState<Screen>>) {
    if cli.demo {
        commands.insert_resource(game::PlayRequest { map: demo::demo_map(), audio_path: None, autoplay: cli.autoplay, practice: true });
        next.set(Screen::Playing);
    }
    if let Some(s) = &cli.screen {
        next.set(match s.as_str() { "library" => Screen::Library, "settings" => Screen::Settings, "stats" => Screen::Stats, "editor" => Screen::Editor, "about" => Screen::About, _ => Screen::Home });
    }
}

/// UI y fuentes se componen a 1080 px de alto y se escalan con la ventana y el ajuste de escala.
fn scale_ui(mut ui_scale: ResMut<UiScale>, window: Single<&Window, With<PrimaryWindow>>, settings: Res<Settings>) {
    let s = window.height() / theme::DESIGN_H * settings.ui_scale;
    if (ui_scale.0 - s).abs() > 1e-4 { ui_scale.0 = s }
}

/// Límite de FPS (espera activa corta + sleep) y F11 para pantalla completa.
fn frame_limit_and_fullscreen(
    settings: Res<Settings>, keys: Res<ButtonInput<KeyCode>>, mut window: Single<&mut Window, With<PrimaryWindow>>,
    mut last: Local<Option<std::time::Instant>>,
) {
    if keys.just_pressed(KeyCode::F11) {
        window.mode = if matches!(window.mode, WindowMode::Windowed) { WindowMode::BorderlessFullscreen(MonitorSelection::Current) } else { WindowMode::Windowed };
    }
    window.present_mode = if settings.vsync { PresentMode::AutoVsync } else { PresentMode::AutoNoVsync };
    if settings.fps_limit > 0 {
        let target = std::time::Duration::from_secs_f64(1.0 / settings.fps_limit as f64);
        if let Some(l) = *last {
            let el = l.elapsed();
            if el < target { std::thread::sleep(target - el) }
        }
        *last = Some(std::time::Instant::now());
    }
}

/// `--shot <segundos> <ruta>`: guarda una captura y sale poco después (verificación visual).
fn cli_shot(mut commands: Commands, cli: Res<Cli>, time: Res<Time>, mut done: Local<u8>, mut exit: MessageWriter<AppExit>) {
    if let Some((at, path)) = &cli.shot {
        let t = time.elapsed_secs();
        if *done == 0 && t >= *at {
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path.clone()));
            *done = 1;
        } else if *done == 1 && t >= *at + 1.0 { exit.write(AppExit::Success); }
    }
    if let Some(s) = cli.exit_after { if time.elapsed_secs() >= s { exit.write(AppExit::Success); } }
}
