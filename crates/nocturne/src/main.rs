mod about;
mod app;
mod audio;
mod calibrate;
mod db;
mod demo;
mod game;
mod home;
mod import;
mod jobs;
mod library;
mod mapedit;
mod paths;
mod probe;
mod results;
mod settings;
mod settings_ui;
mod stats;
mod theme;
mod toast;
mod ui;

use app::{AppDb, ReturnTo, Screen};
use bevy::camera::ScalingMode;
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::window::{MonitorSelection, PresentMode, PrimaryWindow, WindowMode, WindowResizeConstraints};
use nocturne_core::beatmap::Difficulty;
use settings::Settings;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const USAGE: &str = "\
Nocturne: tu música, convertida en mapa de ritmo.

Uso: nocturne [opciones]

  --import <ruta>        analiza un archivo de audio o todos los de una carpeta (repetible)
  --play <archivo>       importa si hace falta (bloquea hasta terminar el análisis) y empieza a jugar
  --edit <archivo>       lo mismo, pero abre el mapa en el editor
  --difficulty <nivel>   easy | normal | hard | expert (con --play o --edit; por defecto normal)
  --screen <pantalla>    home | library | settings | stats | about | calibrate
  --demo                 mapa de demostración sin audio
  --autoplay             el autopiloto juega solo (verificación)
  --probe-audio          mide el reloj del dispositivo de audio y sale
  --shot <seg> <png>     guarda una captura a los <seg> segundos y sale
  --exit-after <seg>     sale pasados <seg> segundos
  --version, --help

Variables: NOCTURNE_HOME redirige configuración y datos a otro directorio.";

/// Opciones de línea de órdenes para verificación y automatización.
#[derive(Resource, Default, Clone)]
struct Cli {
    screen: Option<String>,
    demo: bool,
    autoplay: bool,
    play: Option<PathBuf>,
    edit: Option<PathBuf>,
    difficulty: Option<Difficulty>,
    imports: Vec<PathBuf>,
    shot: Option<(f32, String)>,
    exit_after: Option<f32>,
    help: bool,
    version: bool,
}

fn parse_difficulty(s: &str) -> Option<Difficulty> {
    Difficulty::ALL.into_iter().find(|d| d.label().eq_ignore_ascii_case(s))
}

fn parse_cli(a: &[String]) -> Cli {
    let mut c = Cli::default();
    let mut i = 1;
    while i < a.len() {
        match a[i].as_str() {
            "--demo" => c.demo = true,
            "--autoplay" => c.autoplay = true,
            "--help" | "-h" => c.help = true,
            "--version" | "-V" => c.version = true,
            "--screen" => { i += 1; c.screen = a.get(i).cloned() }
            "--play" => { i += 1; c.play = a.get(i).map(Into::into) }
            "--edit" => { i += 1; c.edit = a.get(i).map(Into::into) }
            "--import" => { i += 1; if let Some(p) = a.get(i) { c.imports.push(p.into()) } }
            "--difficulty" => { i += 1; c.difficulty = a.get(i).and_then(|s| parse_difficulty(s)) }
            "--shot" => { c.shot = a.get(i + 1).and_then(|s| s.parse().ok()).zip(a.get(i + 2).cloned()); i += 2 }
            "--exit-after" => { i += 1; c.exit_after = a.get(i).and_then(|s| s.parse().ok()) }
            _ => {}
        }
        i += 1;
    }
    c
}

fn screen_by_name(s: &str) -> Screen {
    match s { "library" => Screen::Library, "settings" => Screen::Settings, "stats" => Screen::Stats, "editor" => Screen::Editor,
              "about" => Screen::About, "calibrate" => Screen::Calibrate, _ => Screen::Home }
}

/// Importación síncrona de `--play`: la ventana aún no existe, así que bloquear aquí es aceptable.
fn prepare_play(path: &Path, diff: Difficulty, settings: &Settings, db: &db::Db) -> Result<game::PlayRequest, String> {
    let song = import::import_blocking(db, path, settings, &paths::maps_dir()).map_err(|e| e.to_string())?;
    let map = import::load_map(&paths::maps_dir(), &song.fingerprint, diff).map_err(|e| e.to_string())?;
    if map.notes.is_empty() { return Err(format!("el mapa {} no tiene notas", diff.label())) }
    Ok(game::PlayRequest::new(map, Some(PathBuf::from(&song.path))))
}

/// Como `prepare_play`, pero abre el mapa en el editor.
fn prepare_edit(path: &Path, diff: Difficulty, settings: &Settings, db: &db::Db) -> Result<mapedit::EditSession, String> {
    let song = import::import_blocking(db, path, settings, &paths::maps_dir()).map_err(|e| e.to_string())?;
    mapedit::open_session(&song, diff)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--probe-audio") { probe::run(None); return }
    let cli = parse_cli(&args);
    let (cli_autoplay, cli_demo) = (cli.autoplay, cli.demo);
    if cli.help { println!("{USAGE}"); return }
    if cli.version { println!("{} {}", theme::APP_NAME, env!("CARGO_PKG_VERSION")); return }
    let settings = Settings::load(&paths::settings_path());
    let (db, db_warn) = db::Db::open(&paths::db_path());
    let _ = db.recover_interrupted();
    let audio = audio::Audio::new(settings.audio_device.as_deref());
    let mut jobs = jobs::Jobs::default();
    if let Some(w) = db_warn { jobs.notice(w, jobs::NoticeKind::Error) }
    if let Some(e) = &audio.device_error { jobs.notice(format!("Audio no disponible: {}. Se usará un reloj silencioso.", e.trim_end_matches('.')), jobs::NoticeKind::Error) }
    for w in &settings.warnings { jobs.notice(w.clone(), jobs::NoticeKind::Info) }
    for p in &cli.imports { library::queue_path(&mut jobs, &settings, p); }
    let prepared = cli.play.as_ref().and_then(|p| match prepare_play(p, cli.difficulty.unwrap_or(Difficulty::Normal), &settings, &db) {
        Ok(r) => Some(r),
        Err(e) => { eprintln!("no se pudo preparar la partida: {e}"); jobs.notice(format!("No se pudo preparar la partida: {e}"), jobs::NoticeKind::Error); None }
    });

    let window = Window {
        title: theme::APP_NAME.into(), name: Some("nocturne".into()),
        resolution: (1600u32, 900u32).into(),
        resize_constraints: WindowResizeConstraints { min_width: 960.0, min_height: 540.0, ..default() },
        present_mode: if settings.vsync { PresentMode::AutoVsync } else { PresentMode::AutoNoVsync },
        mode: if settings.fullscreen { WindowMode::BorderlessFullscreen(MonitorSelection::Primary) } else { WindowMode::Windowed },
        ..default()
    };
    let session = cli.edit.as_ref().and_then(|p| match prepare_edit(p, cli.difficulty.unwrap_or(Difficulty::Normal), &settings, &db) {
        Ok(s) => Some(s),
        Err(e) => { eprintln!("no se pudo abrir el editor: {e}"); jobs.notice(format!("No se pudo abrir el editor: {e}"), jobs::NoticeKind::Error); None }
    });
    let initial = if session.is_some() { Screen::Editor } else if prepared.is_some() || cli.demo { Screen::Playing }
        else if let Some(s) = &cli.screen { screen_by_name(s) }
        else if !cli.imports.is_empty() || cli.play.is_some() { Screen::Library }
        else { Screen::Home };
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin { primary_window: Some(window), ..default() }))
        .insert_resource(ClearColor(theme::BG))
        .insert_resource(settings)
        .insert_resource(AppDb(Mutex::new(db)))
        .insert_resource(audio)
        .insert_resource(jobs)
        .insert_resource(cli)
        .insert_resource(Initial(initial))
        .init_state::<Screen>()
        .add_systems(PreStartup, theme::load_fonts)
        .add_systems(Startup, (setup_camera, boot))
        .add_systems(Update, (scale_ui, jobs::poll, window_sync, toggle_fullscreen, frame_limit, diagnostics, cli_shot))
        .add_plugins((ui::plugin, home::plugin, library::plugin, game::plugin, results::plugin, toast::plugin,
                      settings_ui::plugin, stats::plugin, about::plugin, calibrate::plugin, mapedit::plugin));
    if let Some(s) = session {
        app.insert_resource(s).init_resource::<ReturnTo>();
    } else if let Some(p) = prepared {
        app.insert_resource(game::PlayRequest { autoplay: cli_autoplay, ..p }).insert_resource(ReturnTo(Screen::Library));
    } else if cli_demo {
        app.insert_resource(game::PlayRequest { autoplay: cli_autoplay, practice: true, ..game::PlayRequest::new(demo::demo_map(), None) })
            .insert_resource(ReturnTo(Screen::Home));
    } else {
        app.init_resource::<ReturnTo>();
    }
    app.run();
}

/// Pantalla con la que arranca la aplicación según la línea de órdenes.
#[derive(Resource)]
struct Initial(Screen);

fn boot(initial: Res<Initial>, mut next: ResMut<NextState<Screen>>) { next.set(initial.0) }

fn setup_camera(mut commands: Commands) {
    commands.spawn((Camera2d, Projection::Orthographic(OrthographicProjection {
        scaling_mode: ScalingMode::FixedVertical { viewport_height: theme::DESIGN_H }, ..OrthographicProjection::default_2d() })));
}

/// UI y fuentes se componen a 1080 px de alto y se escalan con la ventana y el ajuste de escala.
fn scale_ui(mut ui_scale: ResMut<UiScale>, window: Single<&Window, With<PrimaryWindow>>, settings: Res<Settings>) {
    let s = window.height() / theme::DESIGN_H * settings.ui_scale;
    if (ui_scale.0 - s).abs() > 1e-4 { ui_scale.0 = s }
}

/// Aplica a la ventana los ajustes de pantalla completa y sincronización vertical.
fn window_sync(settings: Res<Settings>, mut window: Single<&mut Window, With<PrimaryWindow>>) {
    if !settings.is_changed() { return }
    let want = if settings.fullscreen { WindowMode::BorderlessFullscreen(MonitorSelection::Current) } else { WindowMode::Windowed };
    if window.mode != want { window.mode = want }
    let pm = if settings.vsync { PresentMode::AutoVsync } else { PresentMode::AutoNoVsync };
    if window.present_mode != pm { window.present_mode = pm }
}

/// F11 alterna la pantalla completa y lo recuerda.
fn toggle_fullscreen(keys: Res<ButtonInput<KeyCode>>, mut settings: ResMut<Settings>, mut jobs: ResMut<jobs::Jobs>) {
    if keys.just_pressed(KeyCode::F11) {
        settings.fullscreen = !settings.fullscreen;
        if let Err(e) = settings.save(&paths::settings_path()) { jobs.notice(format!("No se pudieron guardar los ajustes: {e}"), jobs::NoticeKind::Error) }
    }
}

/// Límite de FPS (espera corta con sleep).
fn frame_limit(settings: Res<Settings>, mut last: Local<Option<std::time::Instant>>) {
    if settings.fps_limit > 0 {
        let target = std::time::Duration::from_secs_f64(1.0 / settings.fps_limit as f64);
        if let Some(l) = *last {
            let el = l.elapsed();
            if el < target { std::thread::sleep(target - el) }
        }
        *last = Some(std::time::Instant::now());
    }
}

#[derive(Component)] struct DiagText;

/// Contador de FPS y estado del audio en una esquina (ajuste «Mostrar diagnóstico»).
fn diagnostics(
    mut commands: Commands, settings: Res<Settings>, time: Res<Time>, audio: Res<audio::Audio>, fonts: Option<Res<theme::Fonts>>,
    mut q: Query<(Entity, &mut Text), With<DiagText>>, mut ema: Local<f32>,
) {
    let dt = time.delta_secs().max(1e-4);
    *ema = if *ema == 0.0 { dt } else { *ema * 0.95 + dt * 0.05 };
    let line = format!("{:.0} FPS · {:.1} ms · audio {:.0} ms · xruns {}", 1.0 / *ema, *ema * 1000.0, audio.latency_ms(), audio.xruns());
    match (settings.show_diagnostics, q.iter_mut().next()) {
        (true, Some((_, mut t))) => { if **t != line { **t = line } }
        (true, None) => {
            let Some(fonts) = fonts else { return };
            commands.spawn((DiagText, GlobalZIndex(90),
                Node { position_type: PositionType::Absolute, right: px(16), top: px(8), ..default() },
                ui::text(&fonts, line, 16.0, theme::alpha(theme::TEXT_DIM, 0.9), theme::Weight::Regular)));
        }
        (false, Some((e, _))) => commands.entity(e).despawn(),
        (false, None) => {}
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> { std::iter::once("nocturne").chain(s.split_whitespace()).map(String::from).collect() }

    #[test]
    fn cli_parsing() {
        let c = parse_cli(&args("--import a.mp3 --import /m --play b.wav --edit c.flac --difficulty EXPERT --screen library --autoplay --shot 3.5 out.png --exit-after 9"));
        assert_eq!(c.imports, vec![PathBuf::from("a.mp3"), PathBuf::from("/m")]);
        assert_eq!(c.play, Some(PathBuf::from("b.wav")));
        assert_eq!(c.edit, Some(PathBuf::from("c.flac")));
        assert_eq!(c.difficulty, Some(Difficulty::Expert));
        assert_eq!(c.screen.as_deref(), Some("library"));
        assert!(c.autoplay && !c.demo);
        assert_eq!(c.shot, Some((3.5, "out.png".to_string())));
        assert_eq!(c.exit_after, Some(9.0));
        assert!(parse_cli(&args("--help")).help && parse_cli(&args("-V")).version);
        assert_eq!(parse_cli(&args("--difficulty nada")).difficulty, None);
        assert!(parse_cli(&args("--import")).imports.is_empty(), "argumento ausente no entra en pánico");
    }

    #[test]
    fn screens_by_name() {
        assert_eq!(screen_by_name("calibrate"), Screen::Calibrate);
        assert_eq!(screen_by_name("lo-que-sea"), Screen::Home);
    }
}
