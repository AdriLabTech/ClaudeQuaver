//! Editor de mapas. La lógica de edición (validación, deshacer, cuadrícula) vive en
//! `nocturne_core::editor`; aquí solo hay vista, entrada y reproducción.
//!
//! El campo se dibuja como en la partida (futuro hacia arriba), pero el cursor es la línea de
//! impacto y el tiempo lo manda el jugador: se desplaza con las flechas o la rueda, o lo mueve
//! el audio al reproducir. Las notas y las líneas de la cuadrícula salen de *pools* fijos de
//! entidades que se reasignan cada fotograma: no se crean ni destruyen entidades al desplazarse.

use crate::app::{ReturnTo, Screen};
use crate::audio::{prepare, Audio};
use crate::db::Song;
use crate::game::PlayRequest;
use crate::import;
use crate::jobs::{Jobs, NoticeKind};
use crate::paths;
use crate::settings::{key_label, Settings};
use crate::theme::*;
use crate::ui::{self, Clicks, KeyRepeat};
use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use nocturne_core::beatmap::{Difficulty, NoteKind};
use nocturne_core::editor::{division_label, snap_free, Editor, Grid, LineKind, DIVISIONS};
use nocturne_core::Micros;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc, Mutex};

const NOTE_R: f32 = 40.0;
const NOTE_POOL: usize = 240;
const GRID_POOL: usize = 200;
const CURSOR_FROM_BOTTOM: f32 = 280.0;
const ZOOM_MIN: f32 = 120.0;
const ZOOM_MAX: f32 = 2400.0;
/// Arrastre mínimo (px) para que un clic cuente como nota larga y no como nota suelta.
const DRAG_MIN_PX: f32 = 22.0;
/// Ancho de carril del editor (independiente del de la partida, que es configurable).
const LANE_W: f32 = 116.0;
const FIELD_W: f32 = LANE_W * 4.0;

const ID_SAVE_EXIT: u32 = 1;
const ID_DISCARD: u32 = 2;
const ID_KEEP: u32 = 3;

type Loader = Mutex<mpsc::Receiver<Result<Arc<Vec<f32>>, String>>>;

enum AudioState { Loading(Loader), Ready { buf: Arc<Vec<f32>>, rate: u32 }, Missing(String) }

struct Drag { lane: u8, t0: Micros, y0: f32 }

#[derive(Resource)]
pub struct EditSession {
    editor: Editor,
    song: Song,
    audio_path: PathBuf,
    audio: AudioState,
    cur: Micros,
    playing: bool,
    zoom: f32,
    div: usize,
    free: bool,
    pending: Option<(u8, Micros)>,
    drag: Option<Drag>,
    hover: Option<(u8, Micros)>,
    modal: bool,
    status: (String, f32),
}

/// Abre el mapa de una canción para editarlo.
pub fn open_session(song: &Song, diff: Difficulty) -> Result<EditSession, String> {
    let map = import::load_map(&paths::maps_dir(), &song.fingerprint, diff).map_err(|e| format!("No se pudo abrir el mapa {}: {e}", diff.label()))?;
    let editor = Editor::new(map).map_err(|e| format!("El mapa {} no es válido: {e}", diff.label()))?;
    Ok(EditSession {
        editor, song: song.clone(), audio_path: PathBuf::from(&song.path), audio: AudioState::Missing("sin cargar".into()),
        cur: Micros::ZERO, playing: false, zoom: 520.0, div: 1, free: false, pending: None, drag: None, hover: None, modal: false,
        status: (String::new(), 9.0),
    })
}

// --- Geometría y formato (puro, con tests) ---------------------------------------------------------

fn hit_y() -> f32 { -DESIGN_H / 2.0 + CURSOR_FROM_BOTTOM }

pub fn time_to_y(t: Micros, cur: Micros, zoom: f32) -> f32 { hit_y() + (t - cur).as_secs() as f32 * zoom }
pub fn y_to_time(y: f32, cur: Micros, zoom: f32) -> Micros { cur + Micros::from_secs(((y - hit_y()) / zoom) as f64) }
fn lane_x(lane: u8) -> f32 { (lane as f32 - 1.5) * LANE_W }

/// Carril bajo la coordenada `x` del mundo, si está dentro del campo.
pub fn lane_at(x: f32) -> Option<u8> {
    let l = ((x + FIELD_W / 2.0) / LANE_W).floor();
    (0.0..4.0).contains(&l).then_some(l as u8)
}

pub fn fmt_time(t: Micros) -> String {
    let ms = (t.0.max(0) / 1000) as u64;
    format!("{:02}:{:02}.{:03}", ms / 60_000, ms / 1000 % 60, ms % 1000)
}

/// «compás 12 · pulso 3» respecto a la cuadrícula (compases de 4 pulsos desde el primer pulso).
pub fn bar_beat(t: Micros, g: &Grid) -> Option<(i64, i64)> {
    if t < g.origin { return None }
    let beats = ((t.0 - g.origin.0) as f64 / g.beat_us).floor() as i64;
    Some((beats.div_euclid(4) + 1, beats.rem_euclid(4) + 1))
}

impl EditSession {
    fn offset(&self, s: &Settings) -> Micros { Micros::from_ms(s.global_offset_ms as f64 + self.editor.map().song_offset_ms) }
    fn grid(&self) -> Option<Grid> { if self.free { None } else { Grid::for_map(self.editor.map(), DIVISIONS[self.div]) } }
    fn duration(&self) -> Micros { self.editor.map().duration }
    fn clamp(&self, t: Micros) -> Micros { Micros(t.0.clamp(0, self.duration().0)) }
    fn snap(&self, t: Micros) -> Micros { self.clamp(match self.grid() { Some(g) => g.snap(t), None => snap_free(t) }) }
    fn step(&self, t: Micros, dir: i32) -> Micros {
        self.clamp(match self.grid() { Some(g) => g.step(t, dir), None => Micros(t.0 + dir as i64 * 10_000) })
    }
    fn bar(&self, t: Micros, dir: i32, bars: i32) -> Micros {
        match Grid::for_map(self.editor.map(), 1) {
            Some(g) => { let mut t = g.snap(t); for _ in 0..(4 * bars) { t = g.step(t, dir) } self.clamp(t) }
            None => self.clamp(Micros(t.0 + (dir * bars * 2000) as i64 * 1000)),
        }
    }
    /// Tolerancia temporal equivalente a unos píxeles de pantalla al hacer clic en una nota.
    fn tol(&self) -> Micros { Micros::from_secs(30.0 / self.zoom as f64) }
    fn say(&mut self, msg: impl Into<String>) { self.status = (msg.into(), 0.0) }
}

#[derive(Component)] struct EditEntity;
#[derive(Component)] struct Ghost;
#[derive(Component)] struct HoverMark;
#[derive(Component)] struct HudTime;
#[derive(Component)] struct HudPos;
#[derive(Component)] struct HudInfo;
#[derive(Component)] struct HudStatus;
#[derive(Component)] struct ModalRoot;

struct NoteSlot { root: Entity, outer: Entity, body: Entity }

#[derive(Clone)]
struct Mats {
    outer: Handle<ColorMaterial>, outer_alt: Handle<ColorMaterial>, manual: Handle<ColorMaterial>,
    hold: Handle<ColorMaterial>, hold_manual: Handle<ColorMaterial>,
    bar: Handle<ColorMaterial>, beat: Handle<ColorMaterial>, sub: Handle<ColorMaterial>,
}

#[derive(Resource)]
struct Pool { notes: Vec<NoteSlot>, grid: Vec<Entity>, ghost: Entity, hover: Entity, mats: Mats }

pub fn plugin(app: &mut App) {
    app.add_systems(OnEnter(Screen::Editor), enter)
        .add_systems(OnExit(Screen::Editor), exit)
        .add_systems(Update, (load_poll, input, playback, draw, hud, modal_ui).chain()
            .run_if(in_state(Screen::Editor)).run_if(resource_exists::<EditSession>).run_if(resource_exists::<Pool>));
}

fn spawn_loader(path: PathBuf, rate: u32) -> Loader {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let r = nocturne_core::decode::decode_file(&path, &AtomicBool::new(false)).map(|a| prepare(&a, rate)).map_err(|e| e.to_string());
        let _ = tx.send(r);
    });
    Mutex::new(rx)
}

fn enter(
    mut commands: Commands, session: Option<ResMut<EditSession>>, mut next: ResMut<NextState<Screen>>, mut jobs: ResMut<Jobs>,
    mut audio: ResMut<Audio>, settings: Res<Settings>, fonts: Res<Fonts>,
    mut meshes: ResMut<Assets<Mesh>>, mut mats: ResMut<Assets<ColorMaterial>>,
) {
    let Some(mut s) = session else {
        jobs.notice("Elige una canción en la biblioteca y pulsa E para editar su mapa", NoticeKind::Info);
        next.set(Screen::Library);
        return;
    };
    audio.set_volume(settings.music_volume, settings.sfx_volume);
    // Audio: reutiliza el ya decodificado si sigue valiendo para este dispositivo.
    let rate = audio.sample_rate();
    let cached = match &s.audio { AudioState::Ready { buf, rate: r } if *r == rate => Some(buf.clone()), _ => None };
    match cached {
        Some(buf) => audio.install(buf, s.cur, false),
        None if s.audio_path.exists() => { audio.unload(); s.audio = AudioState::Loading(spawn_loader(s.audio_path.clone(), rate)) }
        None => { audio.unload(); s.audio = AudioState::Missing(format!("No se encuentra el audio original ({})", paths::redact(&s.audio_path))) }
    }
    s.playing = false;
    s.drag = None;
    s.modal = false;

    let rect = meshes.add(Rectangle::new(1.0, 1.0));
    let circle = meshes.add(Circle::new(1.0));
    let m = Mats {
        outer: mats.add(TEXT), outer_alt: mats.add(Color::srgb_u8(0xC9, 0xCD, 0xD3)), manual: mats.add(OK),
        hold: mats.add(alpha(TEXT_DIM, 0.42)), hold_manual: mats.add(alpha(OK, 0.35)),
        bar: mats.add(alpha(TEXT_DIM, 0.75)), beat: mats.add(alpha(TEXT_DIM, 0.38)), sub: mats.add(alpha(TEXT_DIM, 0.16)),
    };
    let inner = mats.add(SURFACE);
    commands.spawn((DespawnOnExit(Screen::Editor), EditEntity, Mesh2d(rect.clone()), MeshMaterial2d(mats.add(alpha(SURFACE, 0.94))),
        Transform::from_xyz(0.0, 0.0, 1.0).with_scale(Vec3::new(FIELD_W, DESIGN_H + 200.0, 1.0))));
    let edge = mats.add(BORDER);
    for i in [0.0f32, 4.0] {
        commands.spawn((DespawnOnExit(Screen::Editor), Mesh2d(rect.clone()), MeshMaterial2d(edge.clone()),
            Transform::from_xyz((i - 2.0) * LANE_W, 0.0, 2.0).with_scale(Vec3::new(3.0, DESIGN_H + 200.0, 1.0))));
    }
    for i in 1..4 {
        commands.spawn((DespawnOnExit(Screen::Editor), Mesh2d(rect.clone()), MeshMaterial2d(edge.clone()),
            Transform::from_xyz((i as f32 - 2.0) * LANE_W, 0.0, 2.0).with_scale(Vec3::new(1.0, DESIGN_H + 200.0, 1.0))));
    }
    // Línea del cursor.
    commands.spawn((DespawnOnExit(Screen::Editor), Mesh2d(rect.clone()), MeshMaterial2d(mats.add(alpha(WHITE, 0.9))),
        Transform::from_xyz(0.0, hit_y(), 6.0).with_scale(Vec3::new(FIELD_W + 40.0, 2.0, 1.0))));
    // Etiquetas de las teclas bajo el cursor.
    for (i, k) in settings.keys().iter().enumerate() {
        commands.spawn((DespawnOnExit(Screen::Editor), Text2d::new(key_label(*k)),
            TextFont { font: fonts.medium.clone().into(), font_size: FontSize::Px(26.0), ..default() },
            TextColor(TEXT_DIM), Transform::from_xyz(lane_x(i as u8), hit_y() - 60.0, 5.0)));
    }
    let mut notes = Vec::with_capacity(NOTE_POOL);
    for _ in 0..NOTE_POOL {
        let root = commands.spawn((DespawnOnExit(Screen::Editor), Transform::from_xyz(0.0, 0.0, 4.0), Visibility::Hidden)).id();
        let body = commands.spawn((Mesh2d(rect.clone()), MeshMaterial2d(m.hold.clone()), Transform::from_xyz(0.0, 0.0, -0.2), Visibility::Hidden, ChildOf(root))).id();
        let outer = commands.spawn((Mesh2d(circle.clone()), MeshMaterial2d(m.outer.clone()), Transform::from_xyz(0.0, 0.0, 0.2).with_scale(Vec3::new(NOTE_R, NOTE_R, 1.0)), ChildOf(root))).id();
        commands.spawn((Mesh2d(circle.clone()), MeshMaterial2d(inner.clone()), Transform::from_xyz(0.0, 0.0, 0.3).with_scale(Vec3::new(NOTE_R * 0.66, NOTE_R * 0.66, 1.0)), ChildOf(root)));
        notes.push(NoteSlot { root, outer, body });
    }
    let grid = (0..GRID_POOL).map(|_| commands.spawn((DespawnOnExit(Screen::Editor), Mesh2d(rect.clone()), MeshMaterial2d(m.beat.clone()),
        Transform::from_xyz(0.0, 0.0, 2.2), Visibility::Hidden)).id()).collect();
    let ghost = commands.spawn((DespawnOnExit(Screen::Editor), Ghost, Mesh2d(rect.clone()), MeshMaterial2d(mats.add(alpha(OK, 0.3))),
        Transform::from_xyz(0.0, 0.0, 4.5), Visibility::Hidden)).id();
    let hover = commands.spawn((DespawnOnExit(Screen::Editor), HoverMark, Mesh2d(circle), MeshMaterial2d(mats.add(alpha(WHITE, 0.22))),
        Transform::from_xyz(0.0, 0.0, 5.0).with_scale(Vec3::new(NOTE_R, NOTE_R, 1.0)), Visibility::Hidden)).id();
    commands.insert_resource(Pool { notes, grid, ghost, hover, mats: m });
    spawn_hud(&mut commands, &fonts, &s);
}

fn exit(mut audio: ResMut<Audio>, mut commands: Commands) {
    audio.pause();
    commands.remove_resource::<Pool>();
}

fn spawn_hud(commands: &mut Commands, fonts: &Fonts, s: &EditSession) {
    commands.spawn((DespawnOnExit(Screen::Editor), ui::full_screen_root())).with_children(|p| {
        p.spawn(Node { position_type: PositionType::Absolute, left: px(64), top: px(48), flex_direction: FlexDirection::Column, row_gap: px(6), ..default() }).with_children(|c| {
            c.spawn(ui::spaced(fonts, "EDITOR", 16.0, TEXT_DIM, Weight::Medium, 6.0));
            c.spawn(ui::text(fonts, ui::ellipsize(&s.song.title, 28), 30.0, TEXT, Weight::Medium));
            c.spawn((HudInfo, ui::text(fonts, "", 18.0, TEXT_DIM, Weight::Regular)));
        });
        p.spawn(Node { position_type: PositionType::Absolute, right: px(64), top: px(44), flex_direction: FlexDirection::Column, align_items: AlignItems::FlexEnd, ..default() }).with_children(|c| {
            c.spawn((HudTime, ui::text(fonts, "00:00.000", 54.0, TEXT, Weight::Light)));
            c.spawn((HudPos, ui::text(fonts, "", 20.0, TEXT_DIM, Weight::Regular)));
        });
        // Atajos
        p.spawn(Node { position_type: PositionType::Absolute, left: px(64), top: px(250), flex_direction: FlexDirection::Column, row_gap: px(5), width: px(430), ..default() }).with_children(|c| {
            c.spawn(ui::text(fonts, "ATAJOS", 15.0, TEXT_DIM, Weight::Medium));
            for (k, d) in [
                ("Clic", "poner o quitar nota"), ("Arrastrar", "nota larga"), ("Clic derecho", "borrar"),
                ("Teclas de carril", "nota en el cursor"), ("Mayús + tecla", "inicio y fin de larga"),
                ("↑ ↓ · rueda", "mover el cursor"), ("Ctrl + ↑ ↓", "un compás"), ("Re Pág · Av Pág", "cuatro compases"),
                ("Espacio", "reproducir / pausa"), ("Tab", "subdivisión"), ("G", "ajuste libre"),
                ("- · =", "zoom"), (", · .", "desfase de la canción"), ("Supr", "borrar en el cursor"),
                ("Ctrl + Z · Y", "deshacer · rehacer"), ("Ctrl + S", "guardar"), ("T", "probar desde el cursor"), ("Esc", "salir"),
            ] {
                c.spawn(Node { column_gap: px(14), ..default() }).with_children(|r| {
                    r.spawn((Node { width: px(170), flex_shrink: 0.0, ..default() }, children![ui::text(fonts, k, 15.0, TEXT, Weight::Medium)]));
                    r.spawn(ui::text(fonts, d, 15.0, TEXT_DIM, Weight::Regular));
                });
            }
        });
        p.spawn(Node { position_type: PositionType::Absolute, right: px(64), bottom: px(60), width: px(560), justify_content: JustifyContent::FlexEnd, ..default() })
            .with_children(|c| { c.spawn((HudStatus, ui::text(fonts, "", 20.0, TEXT, Weight::Regular))); });
    });
}

fn load_poll(mut s: ResMut<EditSession>, mut audio: ResMut<Audio>) {
    let AudioState::Loading(rx) = &s.audio else { return };
    let Ok(r) = rx.lock().unwrap().try_recv() else { return };
    let rate = audio.sample_rate();
    match r {
        Ok(buf) => { audio.install(buf.clone(), s.cur, false); s.audio = AudioState::Ready { buf, rate } }
        Err(e) => { s.say(format!("No se pudo cargar el audio: {e}")); s.audio = AudioState::Missing(e) }
    }
}

fn leave(commands: &mut Commands, next: &mut NextState<Screen>) {
    commands.remove_resource::<EditSession>();
    next.set(Screen::Library);
}

fn save(s: &mut EditSession, jobs: &mut Jobs) -> bool {
    match import::save_map(&paths::maps_dir(), s.editor.map()) {
        Ok(()) => { s.editor.mark_saved(); jobs.generation += 1; s.say("Mapa guardado"); true }
        Err(e) => { let msg = format!("No se pudo guardar: {e}"); jobs.notice(msg.clone(), NoticeKind::Error); s.say(msg); false }
    }
}

fn set_cursor(s: &mut EditSession, audio: &mut Audio, settings: &Settings, t: Micros) {
    s.cur = s.clamp(t);
    if s.playing { audio.seek(s.cur + s.offset(settings)) }
}

fn toggle_play(s: &mut EditSession, audio: &mut Audio, settings: &Settings) {
    if s.playing { audio.pause(); s.playing = false; return }
    if !audio.is_loaded() { s.say(match &s.audio { AudioState::Loading(_) => "El audio aún se está cargando".to_string(), AudioState::Missing(e) => e.clone(), _ => "Sin audio".into() }); return }
    audio.seek(s.cur + s.offset(settings));
    audio.play();
    s.playing = true;
}

/// Acción de un carril sobre el cursor: alterna una nota; con Mayús marca inicio y fin de una larga.
fn lane_action(s: &mut EditSession, lane: u8, shift: bool) {
    let t = s.snap(s.cur);
    if shift {
        match s.pending {
            Some((l, t0)) if l == lane => {
                s.pending = None;
                match s.editor.add_hold(t0, t, lane) { Ok(_) => s.say("Nota larga añadida"), Err(e) => s.say(e.to_string()) }
            }
            _ => { s.pending = Some((lane, t)); s.say("Mueve el cursor y pulsa de nuevo Mayús + tecla para cerrar la nota larga") }
        }
        return;
    }
    match s.editor.note_at(lane, t, Micros::from_ms(20.0)) {
        Some(id) => { let _ = s.editor.remove(id); }
        None => if let Err(e) = s.editor.add_tap(t, lane) { s.say(e.to_string()) },
    }
}

#[allow(clippy::too_many_arguments)]
fn input(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut wheel: MessageReader<MouseWheel>,
    time: Res<Time>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera2d>>,
    settings: Res<Settings>,
    mut s: ResMut<EditSession>,
    mut audio: ResMut<Audio>,
    mut jobs: ResMut<Jobs>,
    mut clicks: ResMut<Clicks>,
    mut commands: Commands,
    mut next: ResMut<NextState<Screen>>,
    mut rep: Local<KeyRepeat>,
) {
    let dt = time.delta_secs();
    if s.status.1 < 100.0 { s.status.1 += dt }
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let pressed = |k: KeyCode| keys.just_pressed(k);

    // Diálogo de salida
    if s.modal {
        wheel.clear();
        if pressed(KeyCode::Enter) || pressed(KeyCode::KeyG) || clicks.take(ID_SAVE_EXIT) {
            if save(&mut s, &mut jobs) { leave(&mut commands, &mut next) }
            s.modal = false;
        } else if pressed(KeyCode::KeyN) || clicks.take(ID_DISCARD) { leave(&mut commands, &mut next) }
        else if pressed(KeyCode::Escape) || clicks.take(ID_KEEP) { s.modal = false }
        return;
    }

    let kb = settings.keys();
    let lane_key = |k: KeyCode| kb.contains(&k);

    // --- Ratón ---
    let world = window.cursor_position().and_then(|p| camera.0.viewport_to_world_2d(camera.1, p).ok());
    s.hover = world.and_then(|w| lane_at(w.x).map(|l| (l, s.snap(y_to_time(w.y, s.cur, s.zoom)))));
    let scroll: f32 = wheel.read().map(|w| w.y.signum() * (w.y != 0.0) as i32 as f32).sum();
    if scroll != 0.0 {
        if ctrl {
            s.zoom = (s.zoom * 1.15f32.powf(scroll)).clamp(ZOOM_MIN, ZOOM_MAX);
        } else {
            let mut t = s.cur;
            for _ in 0..(scroll.abs() as i32) { t = s.step(t, scroll.signum() as i32) }
            set_cursor(&mut s, &mut audio, &settings, t);
        }
    }
    if mouse.just_pressed(MouseButton::Left) {
        if let (Some(w), Some((lane, t))) = (world, s.hover) { s.drag = Some(Drag { lane, t0: t, y0: w.y }) }
    }
    if mouse.just_released(MouseButton::Left) {
        if let Some(d) = s.drag.take() {
            let y1 = world.map_or(d.y0, |w| w.y);
            let t1 = s.snap(y_to_time(y1, s.cur, s.zoom));
            if (y1 - d.y0).abs() >= DRAG_MIN_PX {
                match s.editor.add_hold(d.t0, t1, d.lane) { Ok(_) => {}, Err(e) => s.say(e.to_string()) }
            } else {
                let tol = s.tol();
                match s.editor.note_at(d.lane, d.t0, tol) {
                    Some(id) => { let _ = s.editor.remove(id); }
                    None => if let Err(e) = s.editor.add_tap(d.t0, d.lane) { s.say(e.to_string()) },
                }
            }
        }
    }
    if mouse.just_pressed(MouseButton::Right) {
        if let (Some(w), Some((lane, _))) = (world, s.hover) {
            let t = y_to_time(w.y, s.cur, s.zoom);
            let tol = s.tol();
            if let Some(id) = s.editor.note_at(lane, t, tol) { let _ = s.editor.remove(id); }
        }
    }

    // --- Teclado ---
    if pressed(KeyCode::Escape) {
        if s.pending.take().is_some() { s.say("Nota larga cancelada") }
        else if s.editor.is_dirty() { s.modal = true }
        else { leave(&mut commands, &mut next) }
        return;
    }
    for (lane, k) in kb.iter().enumerate() {
        if pressed(*k) && !ctrl { lane_action(&mut s, lane as u8, shift) }
    }
    if ctrl && pressed(KeyCode::KeyS) { save(&mut s, &mut jobs); }
    if ctrl && pressed(KeyCode::KeyZ) && !shift { if !s.editor.undo() { s.say("Nada que deshacer") } }
    if (ctrl && pressed(KeyCode::KeyY)) || (ctrl && shift && pressed(KeyCode::KeyZ)) { if !s.editor.redo() { s.say("Nada que rehacer") } }
    let free_key = |k: KeyCode| !lane_key(k) && !ctrl;
    if pressed(KeyCode::Space) && !lane_key(KeyCode::Space) { toggle_play(&mut s, &mut audio, &settings) }
    if pressed(KeyCode::Tab) {
        if s.grid().is_some() || s.free { s.div = (s.div + if shift { DIVISIONS.len() - 1 } else { 1 }) % DIVISIONS.len(); s.free = false }
        let d = division_label(DIVISIONS[s.div]);
        s.say(format!("Ajuste a {d}"));
    }
    if pressed(KeyCode::KeyG) && free_key(KeyCode::KeyG) {
        s.free = !s.free;
        let msg = if s.free { "Ajuste libre (10 ms)" } else { "Ajuste a la cuadrícula" };
        s.say(msg);
        if !s.free && s.grid().is_none() { s.free = true; s.say("Este mapa no tiene BPM: solo ajuste libre") }
    }
    if pressed(KeyCode::Minus) || pressed(KeyCode::NumpadSubtract) { s.zoom = (s.zoom / 1.2).clamp(ZOOM_MIN, ZOOM_MAX) }
    if pressed(KeyCode::Equal) || pressed(KeyCode::NumpadAdd) { s.zoom = (s.zoom * 1.2).clamp(ZOOM_MIN, ZOOM_MAX) }
    if pressed(KeyCode::Comma) || pressed(KeyCode::Period) {
        let d = if pressed(KeyCode::Comma) { -1.0 } else { 1.0 } * if shift { 10.0 } else { 1.0 };
        let v = s.editor.map().song_offset_ms + d;
        s.editor.set_song_offset(v);
        let v = s.editor.map().song_offset_ms;
        s.say(format!("Desfase de la canción: {v:+.0} ms"));
    }
    if pressed(KeyCode::Delete) {
        let t = s.snap(s.cur);
        let ids: Vec<u32> = (0..4u8).filter_map(|l| s.editor.note_at(l, t, Micros::from_ms(20.0))).collect();
        for id in ids { let _ = s.editor.remove(id); }
    }
    if pressed(KeyCode::KeyT) && free_key(KeyCode::KeyT) { start_test(&mut s, &mut audio, &mut commands, &mut next); }

    // Movimiento del cursor (con repetición al mantener).
    let moved = rep.poll(&keys, dt, &[KeyCode::ArrowUp, KeyCode::ArrowDown, KeyCode::PageUp, KeyCode::PageDown]);
    let target = match moved {
        Some(KeyCode::ArrowUp) => Some(if ctrl { s.bar(s.cur, 1, 1) } else { s.step(s.cur, 1) }),
        Some(KeyCode::ArrowDown) => Some(if ctrl { s.bar(s.cur, -1, 1) } else { s.step(s.cur, -1) }),
        Some(KeyCode::PageUp) => Some(s.bar(s.cur, 1, 4)),
        Some(KeyCode::PageDown) => Some(s.bar(s.cur, -1, 4)),
        _ => None,
    };
    if let Some(t) = target { set_cursor(&mut s, &mut audio, &settings, t) }
    if pressed(KeyCode::Home) { set_cursor(&mut s, &mut audio, &settings, Micros::ZERO) }
    if pressed(KeyCode::End) { let d = s.duration(); set_cursor(&mut s, &mut audio, &settings, d) }
}

/// Prueba el mapa editado desde el cursor, en modo práctica (no guarda puntuación).
fn start_test(s: &mut EditSession, audio: &mut Audio, commands: &mut Commands, next: &mut NextState<Screen>) {
    let start = s.snap(s.cur);
    let mut map = s.editor.map().clone();
    map.notes.retain(|n| n.time >= start);
    if map.notes.is_empty() { s.say("No hay notas desde el cursor para probar"); return }
    if !s.audio_path.exists() { s.say("No se encuentra el audio original: no se puede probar"); return }
    if s.playing { audio.pause(); s.playing = false }
    let preloaded = match &s.audio { AudioState::Ready { buf, rate } if *rate == audio.sample_rate() => Some(buf.clone()), _ => None };
    commands.insert_resource(PlayRequest { practice: true, start, preloaded, ..PlayRequest::new(map, Some(s.audio_path.clone())) });
    commands.insert_resource(ReturnTo(Screen::Editor));
    next.set(Screen::Playing);
}

/// Mientras suena, el cursor sigue al audio y cada nota que cruza la línea hace «tic».
fn playback(mut s: ResMut<EditSession>, mut audio: ResMut<Audio>, settings: Res<Settings>) {
    if !s.playing { return }
    let prev = s.cur;
    let t = Micros(audio.clock().0 - s.offset(&settings).0);
    s.cur = s.clamp(t);
    if audio.is_finished() || s.cur >= s.duration() { audio.pause(); s.playing = false; return }
    if s.cur > prev && s.cur - prev < Micros::from_ms(120.0) {
        let notes = s.editor.notes();
        let from = notes.partition_point(|n| n.time <= prev);
        let hits = notes[from..].iter().take_while(|n| n.time <= s.cur).count();
        for _ in 0..hits.min(3) { audio.hit_sound() }
    }
}

fn mat_if_changed(q: &mut Query<&mut MeshMaterial2d<ColorMaterial>>, e: Entity, h: &Handle<ColorMaterial>) {
    if let Ok(mut m) = q.get_mut(e) { if m.0 != *h { m.0 = h.clone() } }
}

#[allow(clippy::too_many_arguments)]
fn draw(
    s: Res<EditSession>,
    pool: Res<Pool>,
    mut tf: Query<&mut Transform>,
    mut vis: Query<&mut Visibility>,
    mut mat: Query<&mut MeshMaterial2d<ColorMaterial>>,
) {
    let (cur, zoom) = (s.cur, s.zoom);
    let (t0, t1) = (y_to_time(-DESIGN_H / 2.0 - 120.0, cur, zoom), y_to_time(DESIGN_H / 2.0 + 120.0, cur, zoom));
    let set_vis = |vis: &mut Query<&mut Visibility>, e: Entity, v: Visibility| { if let Ok(mut x) = vis.get_mut(e) { if *x != v { *x = v } } };

    // Notas
    let mut used = 0;
    for n in s.editor.notes().iter().filter(|n| n.end() >= t0 && n.time <= t1).take(NOTE_POOL) {
        let slot = &pool.notes[used];
        used += 1;
        if let Ok(mut t) = tf.get_mut(slot.root) { t.translation = Vec3::new(lane_x(n.lane), time_to_y(n.time, cur, zoom), 4.0) }
        set_vis(&mut vis, slot.root, Visibility::Visible);
        let outer = if n.manual { &pool.mats.manual } else if n.lane == 0 || n.lane == 3 { &pool.mats.outer } else { &pool.mats.outer_alt };
        mat_if_changed(&mut mat, slot.outer, outer);
        match n.kind {
            NoteKind::Hold { end } => {
                let len = (end - n.time).as_secs() as f32 * zoom;
                if let Ok(mut t) = tf.get_mut(slot.body) { t.translation.y = len / 2.0; t.scale = Vec3::new(NOTE_R * 0.9, len, 1.0) }
                mat_if_changed(&mut mat, slot.body, if n.manual { &pool.mats.hold_manual } else { &pool.mats.hold });
                set_vis(&mut vis, slot.body, Visibility::Inherited);
            }
            NoteKind::Tap => set_vis(&mut vis, slot.body, Visibility::Hidden),
        }
    }
    for slot in &pool.notes[used..] { set_vis(&mut vis, slot.root, Visibility::Hidden) }

    // Cuadrícula
    let mut lines: Vec<(Micros, LineKind)> = match s.grid() {
        Some(g) => g.lines(t0.max(Micros::ZERO), t1).into_iter().map(|l| (l.time, l.kind)).collect(),
        None => {
            let first = (t0.0.max(0) / 500_000) * 500_000;
            (0..).map(|k| Micros(first + k * 500_000)).take_while(|t| *t <= t1).map(|t| (t, if t.0 % 2_000_000 == 0 { LineKind::Bar } else { LineKind::Beat })).collect()
        }
    };
    if lines.len() > GRID_POOL { lines.retain(|l| l.1 != LineKind::Sub) }
    if lines.len() > GRID_POOL { lines.retain(|l| l.1 == LineKind::Bar) }
    for (i, e) in pool.grid.iter().enumerate() {
        match lines.get(i) {
            Some((t, kind)) => {
                let (th, m) = match kind { LineKind::Bar => (3.0, &pool.mats.bar), LineKind::Beat => (2.0, &pool.mats.beat), LineKind::Sub => (1.0, &pool.mats.sub) };
                if let Ok(mut x) = tf.get_mut(*e) { x.translation = Vec3::new(0.0, time_to_y(*t, cur, zoom), 2.2); x.scale = Vec3::new(FIELD_W, th, 1.0) }
                mat_if_changed(&mut mat, *e, m);
                set_vis(&mut vis, *e, Visibility::Visible);
            }
            None => set_vis(&mut vis, *e, Visibility::Hidden),
        }
    }

    // Nota larga en curso (arrastre o Mayús + tecla)
    let ghost = match (&s.drag, s.pending) {
        (Some(d), _) => Some((d.lane, d.t0, s.hover.map_or(d.t0, |h| h.1))),
        (None, Some((l, t0))) => Some((l, t0, s.snap(cur))),
        _ => None,
    };
    match ghost {
        Some((lane, a, b)) => {
            let (ya, yb) = (time_to_y(a, cur, zoom), time_to_y(b, cur, zoom));
            if let Ok(mut t) = tf.get_mut(pool.ghost) { t.translation = Vec3::new(lane_x(lane), (ya + yb) / 2.0, 4.5); t.scale = Vec3::new(NOTE_R * 0.9, (yb - ya).abs().max(2.0), 1.0) }
            set_vis(&mut vis, pool.ghost, Visibility::Visible);
        }
        None => set_vis(&mut vis, pool.ghost, Visibility::Hidden),
    }
    // Vista previa del clic
    match s.hover {
        Some((lane, t)) if !s.modal && ghost.is_none() => {
            if let Ok(mut x) = tf.get_mut(pool.hover) { x.translation = Vec3::new(lane_x(lane), time_to_y(t, cur, zoom), 5.0) }
            set_vis(&mut vis, pool.hover, Visibility::Visible);
        }
        _ => set_vis(&mut vis, pool.hover, Visibility::Hidden),
    }
}

fn hud(
    s: Res<EditSession>,
    mut time_q: Query<&mut Text, (With<HudTime>, Without<HudInfo>, Without<HudStatus>, Without<HudPos>)>,
    mut pos_q: Query<&mut Text, (With<HudPos>, Without<HudInfo>, Without<HudStatus>, Without<HudTime>)>,
    mut info_q: Query<&mut Text, (With<HudInfo>, Without<HudTime>, Without<HudStatus>, Without<HudPos>)>,
    mut status_q: Query<(&mut Text, &mut TextColor), (With<HudStatus>, Without<HudTime>, Without<HudInfo>, Without<HudPos>)>,
) {
    let m = s.editor.map();
    let pos = s.grid().or_else(|| Grid::for_map(m, 1)).and_then(|g| bar_beat(s.cur, &g)).map_or(String::new(), |(b, p)| format!("compás {b} · pulso {p}"));
    let time = fmt_time(s.cur);
    for mut t in &mut time_q { if **t != time { **t = time.clone() } }
    for mut t in &mut pos_q { if **t != pos { **t = pos.clone() } }
    let holds = m.notes.iter().filter(|n| n.is_hold()).count();
    let snap = if s.free || s.grid().is_none() { "libre".to_string() } else { division_label(DIVISIONS[s.div]) };
    let info = format!("{} · {} BPM · {} notas{} · ajuste {} · zoom {:.0}{}{}{}",
        m.difficulty.label(), m.music.bpm.map_or("?".into(), |b| format!("{b:.0}")), m.notes.len(),
        match holds { 0 => String::new(), 1 => " (1 larga)".to_string(), n => format!(" ({n} largas)") }, snap, s.zoom,
        if m.song_offset_ms != 0.0 { format!(" · desfase {:+.0} ms", m.song_offset_ms) } else { String::new() },
        if s.editor.is_dirty() { " · sin guardar" } else { "" },
        if matches!(s.audio, AudioState::Loading(_)) { " · cargando audio…" } else if matches!(s.audio, AudioState::Missing(_)) { " · sin audio" } else { "" });
    for mut t in &mut info_q { if **t != info { **t = info.clone() } }
    for (mut t, mut c) in &mut status_q {
        let (msg, age) = &s.status;
        let a = (1.0 - (*age - 3.0) / 1.0).clamp(0.0, 1.0);
        if **t != *msg { **t = msg.clone() }
        c.0 = alpha(TEXT, a);
    }
}

/// Diálogo «¿Guardar los cambios?» al salir con ediciones sin guardar.
fn modal_ui(mut commands: Commands, s: Res<EditSession>, fonts: Res<Fonts>, roots: Query<Entity, With<ModalRoot>>) {
    if s.modal && roots.is_empty() {
        commands.spawn((DespawnOnExit(Screen::Editor), ModalRoot, ui::modal_backdrop(),
            Node { justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..ui::full_screen_root() }))
            .with_children(|m| {
                m.spawn((Node { width: px(760), flex_direction: FlexDirection::Column, row_gap: px(16), padding: UiRect::all(px(40)), border: UiRect::all(px(1)), ..default() },
                    BackgroundColor(SURFACE), BorderColor::all(BORDER))).with_children(|p| {
                    p.spawn(ui::text(&fonts, "¿Guardar los cambios del mapa?", 34.0, TEXT, Weight::Medium));
                    p.spawn(ui::text(&fonts, "Has editado este mapa y no lo has guardado. Si sales ahora se perderán esas ediciones.", 20.0, TEXT_DIM, Weight::Regular));
                    p.spawn(ui::gap(8.0));
                    p.spawn(Node { column_gap: px(14), ..default() }).with_children(|r| {
                        r.spawn(ui::pill(&fonts, ID_SAVE_EXIT, "Guardar y salir · Intro"));
                        r.spawn(ui::pill(&fonts, ID_DISCARD, "Descartar · N"));
                        r.spawn(ui::pill(&fonts, ID_KEEP, "Seguir editando · Esc"));
                    });
                });
            });
    } else if !s.modal {
        for r in &roots { commands.entity(r).despawn() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nocturne_core::beatmap::{Beatmap, MusicInfo, FORMAT_VERSION};

    #[test]
    fn time_and_position_roundtrip() {
        let cur = Micros::from_secs(12.0);
        for zoom in [120.0f32, 520.0, 2400.0] {
            for t in [10.0, 12.0, 12.5, 15.0] {
                let t = Micros::from_secs(t);
                let back = y_to_time(time_to_y(t, cur, zoom), cur, zoom);
                assert!((back - t).abs() < Micros::from_ms(0.5), "zoom {zoom}");
            }
        }
        assert_eq!(time_to_y(cur, cur, 500.0), hit_y(), "el cursor está en la línea de impacto");
        assert!(time_to_y(cur + Micros::from_secs(1.0), cur, 500.0) > hit_y(), "el futuro queda arriba");
    }

    #[test]
    fn lanes_from_world_x() {
        assert_eq!(lane_at(-LANE_W * 2.0 + 1.0), Some(0));
        assert_eq!(lane_at(0.0 - 1.0), Some(1));
        assert_eq!(lane_at(1.0), Some(2));
        assert_eq!(lane_at(LANE_W * 2.0 - 1.0), Some(3));
        assert_eq!(lane_at(LANE_W * 2.0 + 1.0), None);
        assert_eq!(lane_at(-LANE_W * 2.0 - 1.0), None);
        for l in 0..4u8 { assert_eq!(lane_at(lane_x(l)), Some(l)); }
    }

    #[test]
    fn time_formatting_and_bars() {
        assert_eq!(fmt_time(Micros::from_secs(83.0415)), "01:23.041");
        assert_eq!(fmt_time(Micros(-5)), "00:00.000");
        let g = Grid { origin: Micros::from_secs(1.0), beat_us: 500_000.0, divisions: 1 };
        assert_eq!(bar_beat(Micros::from_secs(0.5), &g), None);
        assert_eq!(bar_beat(Micros::from_secs(1.0), &g), Some((1, 1)));
        assert_eq!(bar_beat(Micros::from_secs(2.6), &g), Some((1, 4)));
        assert_eq!(bar_beat(Micros::from_secs(3.0), &g), Some((2, 1)));
    }

    fn session() -> EditSession {
        let map = Beatmap {
            format_version: FORMAT_VERSION, generator_version: "t".into(), title: "T".into(), artist: String::new(), audio_fingerprint: "ff".into(),
            duration: Micros::from_secs(30.0), difficulty: Difficulty::Normal, parameters: String::new(), notes: vec![],
            music: MusicInfo { bpm: Some(120.0), bpm_confidence: 1.0, bpm_candidates: vec![], first_beat: Some(Micros::from_secs(1.0)) },
            song_offset_ms: 0.0, edited_manually: false, repairs: vec![],
        };
        EditSession {
            editor: Editor::new(map).unwrap(),
            song: Song { id: 1, fingerprint: "ff".into(), path: "/x.wav".into(), title: "T".into(), artist: String::new(), duration_ms: 30_000, bpm: Some(120.0),
                status: crate::db::Status::Ready, error: None, added: 0, last_played: None },
            audio_path: PathBuf::from("/x.wav"), audio: AudioState::Missing("test".into()), cur: Micros::ZERO, playing: false, zoom: 500.0,
            div: 1, free: false, pending: None, drag: None, hover: None, modal: false, status: (String::new(), 9.0),
        }
    }

    #[test]
    fn cursor_snaps_and_steps_on_the_grid() {
        let s = session(); // 120 BPM, origen 1 s, corcheas (250 ms)
        assert_eq!(s.snap(Micros::from_secs(1.3)), Micros::from_secs(1.25));
        assert_eq!(s.step(Micros::from_secs(1.25), 1), Micros::from_secs(1.5));
        assert_eq!(s.step(Micros::ZERO, -1), Micros::ZERO, "no sale por la izquierda");
        assert_eq!(s.step(Micros::from_secs(30.0), 1), Micros::from_secs(30.0), "ni por la derecha");
        assert_eq!(s.bar(Micros::from_secs(1.0), 1, 1), Micros::from_secs(3.0));
        assert_eq!(s.bar(Micros::from_secs(3.0), -1, 1), Micros::from_secs(1.0));
        let mut free = session();
        free.free = true;
        assert_eq!(free.snap(Micros::from_ms(1234.0)), Micros::from_ms(1230.0));
        assert_eq!(free.step(Micros::from_ms(1230.0), 1), Micros::from_ms(1240.0));
    }

    #[test]
    fn keyboard_lane_actions_toggle_and_build_holds() {
        let mut s = session();
        s.cur = Micros::from_secs(2.0);
        lane_action(&mut s, 1, false);
        assert_eq!(s.editor.notes().len(), 1);
        lane_action(&mut s, 1, false);
        assert!(s.editor.notes().is_empty(), "pulsar otra vez en el mismo sitio la quita");
        lane_action(&mut s, 2, true);
        assert_eq!(s.pending, Some((2, Micros::from_secs(2.0))));
        s.cur = Micros::from_secs(3.0);
        lane_action(&mut s, 2, true);
        assert_eq!(s.pending, None);
        let n = &s.editor.notes()[0];
        assert!(n.is_hold() && n.time == Micros::from_secs(2.0) && n.end() == Micros::from_secs(3.0) && n.manual);
        // pulsar el carril en mitad de una larga la borra (no se pueden solapar notas en un carril)
        s.cur = Micros::from_secs(2.5);
        lane_action(&mut s, 2, false);
        assert!(s.editor.notes().is_empty());
        s.editor.map().validate().unwrap();
    }

    #[test]
    fn invalid_edits_report_a_message() {
        let mut s = session();
        s.cur = Micros::from_secs(2.0);
        lane_action(&mut s, 0, false);
        s.cur = Micros::from_secs(2.0) + Micros::from_ms(10.0);
        s.free = true;
        lane_action(&mut s, 0, true);
        s.cur = Micros::from_secs(2.0) + Micros::from_ms(60.0);
        lane_action(&mut s, 0, true);
        assert!(s.status.0.contains("corta") || s.status.0.contains("solapa"), "{}", s.status.0);
        s.editor.map().validate().unwrap();
    }
}
