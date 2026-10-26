//! Partida: sesión, reloj de juego, campo 4K, HUD, entrada, pausa y finalización.
//!
//! El tiempo de la carta es `reloj_de_audio − offset`. La posición de cada nota se calcula a partir
//! de su instante de impacto y de ese reloj (`y = línea + (t_nota − t_actual) · velocidad`), nunca
//! acumulando desplazamientos por fotograma, así que no depende de los FPS.

use crate::app::{AppDb, ReturnTo, Screen};
use crate::audio::{prepare, Audio};
use crate::settings::{Settings, NOTE_BASE_R};
use crate::theme::*;
use crate::ui::{self, MenuNav};
use bevy::prelude::*;
use nocturne_core::beatmap::{Beatmap, NoteKind};
use nocturne_core::gameplay::{Event, Judge};
use nocturne_core::scoring::{Judgement, ScoreState};
use nocturne_core::Micros;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Instant;

const TOP_Y: f32 = 640.0;
const PRE_ROLL_S: f64 = 2.5;
const RESUME_S: f64 = 3.0;

/// Lo que hace falta para empezar: se inserta antes de pasar a `Screen::Playing`.
#[derive(Resource, Clone)]
pub struct PlayRequest {
    pub map: Beatmap,
    pub audio_path: Option<PathBuf>,
    pub autoplay: bool,
    /// Si es `true` no se guardan resultados (demo, editor).
    pub practice: bool,
    /// Instante de la canción desde el que se juega (el editor prueba desde el cursor). Las notas
    /// anteriores deben haberse quitado del mapa.
    pub start: Micros,
    /// Audio ya decodificado y remuestreado al dispositivo (evita decodificar otra vez).
    pub preloaded: Option<Arc<Vec<f32>>>,
}

impl PlayRequest {
    pub fn new(map: Beatmap, audio_path: Option<PathBuf>) -> Self {
        PlayRequest { map, audio_path, autoplay: false, practice: false, start: Micros::ZERO, preloaded: None }
    }
}

#[derive(Resource)]
pub struct LastResult {
    pub score: ScoreState,
    pub map: Beatmap,
    pub failed: bool,
    pub played_ms: i64,
    pub request: PlayRequest,
}

enum Phase {
    Loading,
    Countdown { wall: Instant, from_us: f64, hold: bool, dur_s: f64, at: Micros, armed: bool },
    Running,
    Paused,
    Finished { since: Instant },
}

#[derive(Resource)]
pub struct Session {
    req: PlayRequest,
    judge: Judge,
    phase: Phase,
    load_rx: Option<Mutex<mpsc::Receiver<Result<Arc<Vec<f32>>, String>>>>,
    wall_start: Option<Instant>,
    /// Sin audio (demo): el tiempo sale del reloj monotónico.
    silent: bool,
    offset_us: i64,
    speed: f32,
    hit_y: f32,
    scale: f32,
    pitch: f32,
    vis: Vec<Option<Vis>>,
    next_spawn: usize,
    bot_cursor: usize,
    last_t: Micros,
    pub error: Option<String>,
    max_ms: i64,
}

#[derive(Clone, Copy)]
struct Vis { root: Entity, body: Option<Entity>, outer: Entity }

#[derive(Resource)]
struct Gfx {
    circle: Handle<Mesh>,
    ring: Handle<Mesh>,
    rect: Handle<Mesh>,
    note: Handle<ColorMaterial>,
    missed: Handle<ColorMaterial>,
    hold_body: Handle<ColorMaterial>,
    hold_active: Handle<ColorMaterial>,
    receptor: Handle<ColorMaterial>,
    press_fill: Handle<ColorMaterial>,
}

#[derive(Component)] struct Lane;
#[derive(Component)] struct PressFill(usize);
#[derive(Component)] struct Flash { life: f32, max: f32, base: f32 }
#[derive(Component)] struct ScoreText;
#[derive(Component)] struct AccText;
#[derive(Component)] struct ComboText;
#[derive(Component)] struct JudgeText { age: f32 }
#[derive(Component)] struct ProgressFill;
#[derive(Component)] struct LifeFill;
#[derive(Component)] struct CountdownText;
#[derive(Component)] struct PauseRoot;
#[derive(Component)] struct StatusText;

const ID_RESUME: u32 = 1;
const ID_RESTART: u32 = 2;
const ID_QUIT: u32 = 3;

pub fn plugin(app: &mut App) {
    app.add_systems(OnEnter(Screen::Playing), begin)
        .add_systems(OnExit(Screen::Playing), end)
        .add_systems(Update, (drive, input, sync_notes, hud, effects, pause_menu).chain().run_if(in_state(Screen::Playing)));
}

fn begin(
    mut commands: Commands,
    req: Res<PlayRequest>,
    settings: Res<Settings>,
    mut audio: ResMut<Audio>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<ColorMaterial>>,
    fonts: Res<Fonts>,
    mut nav: ResMut<MenuNav>,
) {
    nav.enabled = false;
    let radius = settings.note_radius();
    // El anillo es una malla unidad que se escala al radio: su radio interior sale del grosor pedido en px.
    let inner = (1.0 - settings.receptor_border / radius).clamp(0.05, 0.98);
    let gfx = Gfx {
        circle: meshes.add(Circle::new(1.0).mesh().resolution(96)),
        ring: meshes.add(Annulus::new(inner, 1.0).mesh().resolution(96)),
        rect: meshes.add(Rectangle::new(1.0, 1.0)),
        note: mats.add(WHITE),
        missed: mats.add(alpha(WHITE, 0.18)),
        hold_body: mats.add(alpha(WHITE, 0.34)),
        hold_active: mats.add(alpha(WHITE, 0.8)),
        receptor: mats.add(alpha(WHITE, 0.7)),
        press_fill: mats.add(alpha(WHITE, 0.3)),
    };
    let speed = settings.scroll_px_per_s();
    let hit_y = -DESIGN_H / 2.0 + settings.hit_line_height;
    // Pista limpia: cuatro anillos fijos sobre el fondo, sin panel, separadores, línea de impacto ni
    // letras de teclas. Las notas son discos blancos lisos que caen hasta su anillo.
    let pitch = settings.lane_pitch();
    for i in 0..4 {
        let x = (i as f32 - 1.5) * pitch;
        let r = radius;
        commands.spawn((DespawnOnExit(Screen::Playing), Lane, Mesh2d(gfx.ring.clone()), MeshMaterial2d(gfx.receptor.clone()),
            Transform::from_xyz(x, hit_y, 5.0).with_scale(Vec3::new(r, r, 1.0))));
        commands.spawn((DespawnOnExit(Screen::Playing), PressFill(i), Mesh2d(gfx.circle.clone()), MeshMaterial2d(gfx.press_fill.clone()),
            Transform::from_xyz(x, hit_y, 4.8).with_scale(Vec3::new(r, r, 1.0)), Visibility::Hidden));
    }
    commands.insert_resource(gfx);
    spawn_hud(&mut commands, &fonts, &req, &settings);

    // Carga del audio en segundo plano (decodificar + remuestrear al dispositivo).
    let mut phase = Phase::Loading;
    let mut load_rx = None;
    let mut silent = true;
    if let (Some(buf), Some(_)) = (req.preloaded.clone(), req.audio_path.as_ref()) {
        silent = false;
        audio.install(buf, req.start, false);
        phase = countdown_start(req.start);
    } else if let Some(path) = req.audio_path.clone() {
        silent = false;
        let rate = audio.sample_rate();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let r = nocturne_core::decode::decode_file(&path, &std::sync::atomic::AtomicBool::new(false))
                .map(|a| prepare(&a, rate)).map_err(|e| e.to_string());
            let _ = tx.send(r);
        });
        load_rx = Some(Mutex::new(rx));
    } else {
        audio.unload();
        phase = countdown_start(req.start);
    }
    let offset_us = ((settings.global_offset_ms + req.map.song_offset_ms as f32) * 1000.0) as i64;
    let judge = Judge::new(&req.map, settings.hit_windows);
    let n = judge.notes().len();
    let last = req.map.notes.iter().map(|n| n.end().0).max().unwrap_or(0);
    commands.insert_resource(Session {
        judge, phase, load_rx, wall_start: None, silent, offset_us, speed, hit_y, scale: settings.note_scale, pitch,
        vis: vec![None; n], next_spawn: 0, bot_cursor: 0, last_t: req.start - Micros::from_secs(3.0), error: None,
        max_ms: (last.max(req.map.duration.0 / 2) / 1000) + 1500, req: req.clone(),
    });
    audio.set_volume(settings.music_volume, settings.sfx_volume);
}

fn countdown_start(start: Micros) -> Phase {
    Phase::Countdown { wall: Instant::now(), from_us: start.0 as f64 - PRE_ROLL_S * 1e6, hold: false, dur_s: PRE_ROLL_S, at: start, armed: false }
}

fn end(mut commands: Commands, mut audio: ResMut<Audio>) {
    audio.pause();
    commands.remove_resource::<Session>();
    commands.remove_resource::<Gfx>();
}

fn spawn_hud(commands: &mut Commands, fonts: &Fonts, req: &PlayRequest, s: &Settings) {
    let root = (DespawnOnExit(Screen::Playing), ui::full_screen_root());
    commands.spawn(root).with_children(|p| {
        // Título y dificultad (izquierda)
        p.spawn(Node { position_type: PositionType::Absolute, left: px(64), top: px(52), flex_direction: FlexDirection::Column, row_gap: px(6), ..default() })
            .with_children(|c| {
                c.spawn(ui::text(fonts, &req.map.title, 30.0, TEXT, Weight::Medium));
                c.spawn(ui::text(fonts, format!("{}{}", req.map.difficulty.label(), if req.map.artist.is_empty() { String::new() } else { format!("  ·  {}", req.map.artist) }), 18.0, TEXT_DIM, Weight::Regular));
                c.spawn((StatusText, ui::text(fonts, "", 16.0, TEXT_DIM, Weight::Regular)));
            });
        // Puntuación y precisión (derecha)
        p.spawn(Node { position_type: PositionType::Absolute, right: px(64), top: px(44), flex_direction: FlexDirection::Column, align_items: AlignItems::FlexEnd, ..default() })
            .with_children(|c| {
                c.spawn((ScoreText, ui::text(fonts, "0000000", 58.0, TEXT, Weight::Light)));
                c.spawn((AccText, ui::text(fonts, "100.00%", 24.0, TEXT_DIM, Weight::Regular)));
            });
        // Combo y juicio sobre el campo
        p.spawn(Node { position_type: PositionType::Absolute, width: percent(100), top: percent(24), flex_direction: FlexDirection::Column, align_items: AlignItems::Center, ..default() })
            .with_children(|c| {
                c.spawn((ComboText, ui::text(fonts, "", 76.0, TEXT, Weight::Light)));
            });
        p.spawn(Node { position_type: PositionType::Absolute, width: percent(100), top: percent(52), justify_content: JustifyContent::Center, ..default() })
            .with_children(|c| {
                if s.show_judgement { c.spawn((JudgeText { age: 9.0 }, ui::text(fonts, "", 40.0, WHITE, Weight::Medium))); }
            });
        // Cuenta atrás grande
        p.spawn(Node { position_type: PositionType::Absolute, width: percent(100), top: percent(38), justify_content: JustifyContent::Center, ..default() })
            .with_children(|c| { c.spawn((CountdownText, ui::text(fonts, "", 120.0, alpha(TEXT, 0.9), Weight::Light))); });
        // Progreso
        if s.show_progress {
            p.spawn((Node { position_type: PositionType::Absolute, top: px(0), left: px(0), width: percent(100), height: px(3), ..default() }, BackgroundColor(alpha(BORDER, 0.6))))
                .with_children(|c| { c.spawn((ProgressFill, Node { width: percent(0), height: percent(100), ..default() }, BackgroundColor(TEXT))); });
        }
        // Vida
        if s.show_life {
            p.spawn((Node { position_type: PositionType::Absolute, left: percent(50), margin: UiRect::left(px(s.lane_pitch() * 2.0 + 48.0)), bottom: px(s.hit_line_height - 40.0),
                width: px(6), height: px(260), flex_direction: FlexDirection::ColumnReverse, ..default() }, BackgroundColor(BORDER)))
                .with_children(|c| { c.spawn((LifeFill, Node { width: percent(100), height: percent(100), ..default() }, BackgroundColor(TEXT_DIM))); });
        }
    });
}

fn chart_time(s: &mut Session, audio: &mut Audio) -> Micros {
    let raw = match &s.phase {
        Phase::Countdown { wall, from_us, hold, .. } => Micros((if *hold { *from_us } else { *from_us + wall.elapsed().as_micros() as f64 }) as i64),
        Phase::Running | Phase::Finished { .. } if s.silent => Micros(s.wall_start.map_or(0, |w| w.elapsed().as_micros() as i64)),
        Phase::Running | Phase::Finished { .. } => audio.clock(),
        Phase::Paused | Phase::Loading => s.last_t + Micros(s.offset_us),
    };
    Micros(raw.0 - s.offset_us)
}

#[allow(clippy::too_many_arguments)]
fn drive(
    mut commands: Commands,
    mut s: ResMut<Session>,
    mut audio: ResMut<Audio>,
    settings: Res<Settings>,
    db: Res<AppDb>,
    mut next: ResMut<NextState<Screen>>,
) {
    // Carga terminada
    let mut loaded = None;
    if let Some(rx) = &s.load_rx { if let Ok(r) = rx.lock().unwrap().try_recv() { loaded = Some(r) } }
    if let Some(r) = loaded {
        s.load_rx = None;
        match r {
            Ok(buf) => { let start = s.req.start; audio.install(buf, start, false); s.phase = countdown_start(start); }
            Err(e) => { s.error = Some(e); s.phase = Phase::Paused; }
        }
    }
    let t = chart_time(&mut s, &mut audio);
    s.last_t = t;
    let lat_s = if s.silent { 0.0 } else { audio.latency_ms() / 1000.0 };

    // Transiciones de fase
    let mut to_running = false;
    let silent = s.silent;
    if let Phase::Countdown { wall, dur_s, at, armed, hold, .. } = &mut s.phase {
        let e = wall.elapsed().as_secs_f64();
        if !*armed && e >= *dur_s - lat_s && !silent {
            // El audio se arranca `latencia` antes para que sea audible justo en el cero.
            audio.seek(*at);
            audio.play();
            *armed = true;
        }
        if e >= *dur_s { to_running = true }
        let _ = hold;
    }
    if to_running {
        s.phase = Phase::Running;
        s.wall_start = Some(Instant::now());
        if s.silent { s.wall_start = Some(Instant::now() - std::time::Duration::from_micros(s.last_t.0.max(0) as u64 + s.offset_us.max(0) as u64)) }
    }

    let running = matches!(s.phase, Phase::Running);
    if running {
        // Autopiloto (verificación): pulsa con un pequeño error determinista.
        if s.req.autoplay {
            let mut events = vec![];
            while s.bot_cursor < s.judge.notes().len() {
                let n = s.judge.notes()[s.bot_cursor].clone();
                let jitter = Micros::from_ms(((n.id * 37 % 21) as f64) - 10.0);
                if t < n.time + jitter { break }
                events.extend(s.judge.press(n.lane, n.time + jitter));
                s.bot_cursor += 1;
            }
            // Soltar holds al llegar a su final.
            for i in 0..s.judge.notes().len() {
                if s.judge.is_holding(i) { let n = s.judge.notes()[i].clone(); if t >= n.end() { events.extend(s.judge.release(n.lane, n.end())) } }
            }
            apply_events(&mut commands, &mut s, &audio, &settings, events);
        }
        let ev = s.judge.advance(t);
        apply_events(&mut commands, &mut s, &audio, &settings, ev);

        let failed = s.judge.score.failed && !settings.no_fail;
        let over = t.0 / 1000 > s.max_ms || (s.judge.is_finished() && t.0 / 1000 > s.max_ms - 800) || (!s.silent && audio.is_finished() && s.judge.is_finished());
        if failed || over || (s.judge.is_finished() && t > s.req.map.notes.iter().map(|n| n.end()).max().unwrap_or(Micros::ZERO) + Micros::from_secs(1.2)) {
            if !matches!(s.phase, Phase::Finished { .. }) {
                s.phase = Phase::Finished { since: Instant::now() };
                let result = LastResult { score: s.judge.score.clone(), map: s.req.map.clone(), failed, played_ms: (t.0 / 1000).max(0), request: s.req.clone() };
                if !s.req.practice {
                    if let Ok(db) = db.0.lock() {
                        let sc = &result.score;
                        let early = sc.errors_ms.iter().filter(|e| **e < 0.0).count() as u32;
                        let late = sc.errors_ms.len() as u32 - early;
                        let _ = db.add_score(&crate::db::ScoreRow { fingerprint: result.map.audio_fingerprint.clone(), difficulty: result.map.difficulty.label().into(),
                            score: sc.score(), accuracy: sc.accuracy(), max_combo: sc.max_combo, counts: sc.counts, mean_error_ms: sc.mean_error_ms(),
                            early, late, histogram: crate::db::histogram(&sc.errors_ms), played_at: crate::db::now(), duration_ms: result.played_ms, failed });
                        let _ = db.touch_played(&result.map.audio_fingerprint);
                    }
                }
                commands.insert_resource(result);
            }
        }
    }
    if let Phase::Finished { since } = s.phase {
        if since.elapsed().as_secs_f32() > 0.8 { next.set(Screen::Results) }
    }
}

fn apply_events(commands: &mut Commands, s: &mut Session, audio: &Audio, settings: &Settings, events: Vec<Event>) {
    for e in events {
        match e {
            Event::Judged { note, lane, judgement, .. } => {
                let idx = s.judge.notes().iter().position(|n| n.id == note);
                if judgement != Judgement::Miss { audio.hit_sound(); if settings.hit_effects() { spawn_flash(commands, s, lane, judgement) } }
                if let Some(i) = idx {
                    if judgement == Judgement::Miss {
                        commands.queue(move |w: &mut World| { w.resource_scope(|w, g: Mut<Gfx>| { if let Some(s) = w.get_resource::<Session>() { if let Some(Some(v)) = s.vis.get(i) { let (o, m) = (v.outer, g.missed.clone()); w.entity_mut(o).insert(MeshMaterial2d(m)); } } }); });
                    } else if !s.judge.notes()[i].is_hold() {
                        if let Some(v) = s.vis[i].take() { commands.entity(v.root).despawn() }
                    } else if let Some(Some(v)) = s.vis.get(i).copied() {
                        if let (Some(b), true) = (v.body, settings.press_highlight()) { commands.queue(move |w: &mut World| { let m = w.resource::<Gfx>().hold_active.clone(); w.entity_mut(b).insert(MeshMaterial2d(m)); }); }
                    }
                }
                show_judgement(commands, judgement);
            }
            Event::HoldEnd { note, judgement, .. } => {
                if let Some(i) = s.judge.notes().iter().position(|n| n.id == note) { if let Some(v) = s.vis[i].take() { commands.entity(v.root).despawn() } }
                if judgement == Judgement::Miss { show_judgement(commands, judgement) }
            }
            Event::GhostTap { .. } => {}
        }
    }
}

fn show_judgement(commands: &mut Commands, j: Judgement) {
    commands.queue(move |w: &mut World| {
        let mut q = w.query::<(&mut JudgeText, &mut Text, &mut TextColor)>();
        for (mut jt, mut t, mut c) in q.iter_mut(w) { jt.age = 0.0; **t = j.label().to_uppercase(); c.0 = judgement_color(j) }
    });
}

fn spawn_flash(commands: &mut Commands, s: &Session, lane: u8, j: Judgement) {
    let x = (lane as f32 - 1.5) * s.pitch;
    let r = NOTE_BASE_R * s.scale;
    let c = if matches!(j, Judgement::Marvelous | Judgement::Perfect) { WHITE } else { TEXT_DIM };
    commands.queue(move |w: &mut World| {
        let (ring, mat) = { let g = w.resource::<Gfx>(); (g.ring.clone(), c) };
        let m = w.resource_mut::<Assets<ColorMaterial>>().add(alpha(mat, 0.9));
        w.spawn((DespawnOnExit(Screen::Playing), Flash { life: 0.0, max: 0.2, base: r }, Mesh2d(ring), MeshMaterial2d(m), Transform::from_xyz(x, -DESIGN_H / 2.0 + 0.0, 6.0)));
    });
    let _ = s;
}

fn input(
    keys: Res<ButtonInput<KeyCode>>,
    settings: Res<Settings>,
    mut s: ResMut<Session>,
    mut audio: ResMut<Audio>,
    mut commands: Commands,
    mut fills: Query<(&PressFill, &mut Visibility)>,
) {
    let kb = settings.keys();
    let highlight = settings.press_highlight();
    for (f, mut v) in &mut fills { *v = if highlight && keys.pressed(kb[f.0]) { Visibility::Visible } else { Visibility::Hidden } }
    if keys.just_pressed(KeyCode::Escape) {
        match s.phase {
            Phase::Running | Phase::Countdown { hold: false, .. } => pause(&mut s, &mut audio),
            _ => {}
        }
    }
    if s.req.autoplay { return }
    let can_play = matches!(s.phase, Phase::Running) || matches!(s.phase, Phase::Countdown { hold: false, .. });
    if !can_play { return }
    let t = s.last_t;
    let mut events = vec![];
    for (lane, k) in kb.iter().enumerate() {
        if keys.just_pressed(*k) { events.extend(s.judge.press(lane as u8, t)) }
        if keys.just_released(*k) { events.extend(s.judge.release(lane as u8, t)) }
    }
    apply_events(&mut commands, &mut s, &audio, &settings, events);
}

fn pause(s: &mut Session, audio: &mut Audio) {
    if !s.silent { audio.pause() } else if let Some(w) = s.wall_start { s.last_t = Micros(w.elapsed().as_micros() as i64 - s.offset_us); }
    s.phase = Phase::Paused;
}

fn pause_menu(
    mut commands: Commands,
    mut s: ResMut<Session>,
    mut audio: ResMut<Audio>,
    mut nav: ResMut<MenuNav>,
    fonts: Res<Fonts>,
    roots: Query<Entity, With<PauseRoot>>,
    mut next: ResMut<NextState<Screen>>,
    req: Res<PlayRequest>,
    back: Res<ReturnTo>,
) {
    let paused = matches!(s.phase, Phase::Paused) && s.error.is_none();
    if let Some(err) = s.error.clone() {
        if roots.is_empty() {
            nav.reset(1);
            commands.spawn((DespawnOnExit(Screen::Playing), PauseRoot, BackgroundColor(alpha(BG, 0.92)),
                Node { justify_content: JustifyContent::Center, align_items: AlignItems::Center, flex_direction: FlexDirection::Column, row_gap: px(16), ..ui::full_screen_root() }))
                .with_children(|p| {
                    p.spawn(ui::text(&fonts, "No se pudo cargar la canción", 40.0, TEXT, Weight::Medium));
                    p.spawn(ui::text(&fonts, err, 20.0, DANGER, Weight::Regular));
                    p.spawn(ui::menu_item(&fonts, ID_QUIT, 0, "Volver", None, 28.0));
                });
        }
    } else if paused && roots.is_empty() {
        nav.reset(3);
        commands.spawn((DespawnOnExit(Screen::Playing), PauseRoot, BackgroundColor(alpha(BG, 0.82)),
            Node { justify_content: JustifyContent::Center, align_items: AlignItems::Center, flex_direction: FlexDirection::Column, row_gap: px(6), ..ui::full_screen_root() }))
            .with_children(|p| {
                p.spawn(ui::spaced(&fonts, "PAUSA", 56.0, TEXT, Weight::Light, 14.0));
                p.spawn(Node { height: px(28), ..default() });
                p.spawn(ui::menu_item(&fonts, ID_RESUME, 0, "Continuar", None, 32.0));
                p.spawn(ui::menu_item(&fonts, ID_RESTART, 1, "Reiniciar", None, 32.0));
                p.spawn(ui::menu_item(&fonts, ID_QUIT, 2, if back.0 == Screen::Editor { "Volver al editor" } else { "Salir" }, None, 32.0));
            });
    }
    if !paused && s.error.is_none() {
        for r in &roots { commands.entity(r).despawn(); }
        nav.enabled = false;
        return;
    }
    if let Some(id) = nav.activated.take() {
        for r in &roots { commands.entity(r).despawn(); }
        match id {
            ID_RESUME => {
                // Cuenta atrás con el tiempo congelado; el audio arranca `latencia` antes del cero.
                let from = (s.last_t.0 + s.offset_us) as f64;
                if s.silent {
                    s.phase = Phase::Running;
                    s.wall_start = Some(Instant::now() - std::time::Duration::from_micros(from.max(0.0) as u64));
                } else {
                    s.phase = Phase::Countdown { wall: Instant::now(), from_us: from, hold: true, dur_s: RESUME_S, at: Micros(from as i64), armed: false };
                }
                nav.enabled = false;
            }
            ID_RESTART => {
                audio.pause();
                commands.insert_resource(req.clone());
                next.set(Screen::Playing); // re-entrada: reconstruye sesión, campo y HUD
            }
            _ => { audio.pause(); next.set(back.0) }
        }
    }
}

fn sync_notes(
    mut commands: Commands,
    mut s: ResMut<Session>,
    gfx: Res<Gfx>,
    mut xf: Query<&mut Transform>,
) {
    if matches!(s.phase, Phase::Loading) { return }
    let t = s.last_t;
    let lead = Micros::from_secs(((TOP_Y - s.hit_y) / s.speed) as f64 + 0.05);
    // Crear las que entran en pantalla
    while s.next_spawn < s.judge.notes().len() {
        let n = s.judge.notes()[s.next_spawn].clone();
        if n.time > t + lead { break }
        let i = s.next_spawn;
        s.next_spawn += 1;
        if !s.judge.is_pending(i) && !s.judge.is_holding(i) { continue }
        let x = (n.lane as f32 - 1.5) * s.pitch;
        let r = NOTE_BASE_R * s.scale;
        let mut body = None;
        let root = commands.spawn((DespawnOnExit(Screen::Playing), Transform::from_xyz(x, TOP_Y + 200.0, 4.0), Visibility::default())).id();
        let outer = commands.spawn((Mesh2d(gfx.circle.clone()), MeshMaterial2d(gfx.note.clone()), Transform::from_xyz(0.0, 0.0, 0.2).with_scale(Vec3::new(r, r, 1.0)), ChildOf(root))).id();
        if let NoteKind::Hold { end } = n.kind {
            let len = (end - n.time).as_secs() as f32 * s.speed;
            body = Some(commands.spawn((Mesh2d(gfx.rect.clone()), MeshMaterial2d(gfx.hold_body.clone()),
                Transform::from_xyz(0.0, len / 2.0, -0.2).with_scale(Vec3::new(r * 0.9, len, 1.0)), ChildOf(root))).id());
        }
        s.vis[i] = Some(Vis { root, body, outer });
    }
    // Posicionar
    let hit_y = s.hit_y;
    let speed = s.speed;
    for i in 0..s.next_spawn {
        let Some(v) = s.vis[i] else { continue };
        let n = &s.judge.notes()[i];
        let holding = s.judge.is_holding(i);
        let mut y = hit_y + (n.time - t).as_secs() as f32 * speed;
        if holding { y = hit_y }
        if let Ok(mut tr) = xf.get_mut(v.root) { tr.translation.y = y }
        if let (Some(b), NoteKind::Hold { end }) = (v.body, n.kind) {
            let len = if holding { ((end - t).as_secs() as f32 * speed).max(0.0) } else { (end - n.time).as_secs() as f32 * speed };
            if let Ok(mut tr) = xf.get_mut(b) { tr.translation.y = len / 2.0; tr.scale.y = len.max(0.001) }
        }
        // Las falladas se retiran cuando salen por debajo.
        if y < -DESIGN_H / 2.0 - 120.0 && !holding { commands.entity(v.root).despawn(); s.vis[i] = None }
    }
}

fn hud(
    s: Res<Session>,
    audio: Res<Audio>,
    settings: Res<Settings>,
    mut q: ParamSet<(
        Query<&mut Text, With<ScoreText>>, Query<&mut Text, With<AccText>>, Query<&mut Text, With<ComboText>>,
        Query<(&mut JudgeText, &mut TextColor, &mut Text)>, Query<&mut Text, With<CountdownText>>, Query<&mut Text, With<StatusText>>,
    )>,
    mut prog: Query<&mut Node, (With<ProgressFill>, Without<LifeFill>)>,
    mut life: Query<(&mut Node, &mut BackgroundColor), (With<LifeFill>, Without<ProgressFill>)>,
    time: Res<Time>,
) {
    let sc = &s.judge.score;
    for mut t in q.p0().iter_mut() { **t = format!("{:07}", sc.score()) }
    for mut t in q.p1().iter_mut() { **t = format!("{:.2}%", sc.accuracy()) }
    for mut t in q.p2().iter_mut() { **t = if sc.combo >= 2 { sc.combo.to_string() } else { String::new() } }
    for (mut j, mut c, mut t) in q.p3().iter_mut() {
        j.age += time.delta_secs();
        let a = if settings.minimal_visuals { if j.age < 0.45 { 1.0 } else { 0.0 } } else if settings.reduce_motion { (1.0 - (j.age - 0.35) / 0.2).clamp(0.0, 1.0) } else { (1.0 - (j.age - 0.3) / 0.25).clamp(0.0, 1.0) };
        c.0 = c.0.with_alpha(a);
        if a <= 0.0 && !t.is_empty() { **t = String::new() }
    }
    for mut t in q.p4().iter_mut() {
        **t = match &s.phase {
            Phase::Countdown { wall, dur_s, hold: true, .. } => format!("{}", (dur_s - wall.elapsed().as_secs_f64()).ceil().max(1.0) as u32),
            Phase::Loading => String::new(),
            _ => String::new(),
        };
    }
    for mut t in q.p5().iter_mut() {
        **t = if matches!(s.phase, Phase::Loading) { "Cargando…".into() } else if s.req.autoplay { "AUTOPILOTO".into() } else if s.silent { "Sin audio (demostración)".into() } else if audio.has_device() { String::new() } else { "Sin dispositivo de audio: reloj silencioso".into() };
    }
    let dur = s.req.map.duration.0.max(1) as f32;
    for mut n in &mut prog { n.width = percent(((s.last_t.0 as f32 / dur) * 100.0).clamp(0.0, 100.0)) }
    for (mut n, mut bg) in &mut life { n.height = percent(sc.life as f32); bg.0 = if sc.life < 25.0 { DANGER } else { TEXT_DIM } }
}

fn effects(mut commands: Commands, time: Res<Time>, mut q: Query<(Entity, &mut Flash, &mut Transform, &MeshMaterial2d<ColorMaterial>)>, s: Res<Session>, mut mats: ResMut<Assets<ColorMaterial>>) {
    for (e, mut f, mut tr, m) in &mut q {
        f.life += time.delta_secs();
        let k = (f.life / f.max).min(1.0);
        tr.translation.y = s.hit_y;
        let sc = f.base * (1.0 + 0.6 * k);
        tr.scale = Vec3::new(sc, sc, 1.0);
        if let Some(mut mat) = mats.get_mut(&m.0) { mat.color = mat.color.with_alpha(0.9 * (1.0 - k)) }
        if k >= 1.0 { commands.entity(e).despawn() }
    }
}
