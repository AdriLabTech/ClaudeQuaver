//! Biblioteca: lista de canciones, estado de los análisis, dificultades y acceso a jugar, editar,
//! reanalizar y eliminar. Incluye el explorador de archivos con el que se importa música (también
//! se puede arrastrar un archivo o una carpeta sobre la ventana).
//!
//! La pantalla se redibuja entera cuando cambia algo (`Library::dirty`): son pocas decenas de
//! nodos, y así el estado visible nunca se desincroniza del modelo.

use crate::app::{AppDb, ReturnTo, Screen};
use crate::db::{Db, ScoreRow, Song, Status};
use crate::game::PlayRequest;
use crate::import::{self, MapSummary};
use crate::jobs::{Jobs, NoticeKind};
use crate::mapedit;
use crate::paths;
use crate::settings::Settings;
use crate::theme::*;
use crate::ui::{self, Clicks, HoverTint, KeyRepeat};
use bevy::ecs::hierarchy::ChildSpawnerCommands;
use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;
use bevy::window::FileDragAndDrop;
use nocturne_core::beatmap::Difficulty;
use nocturne_core::scoring::grade_for;
use std::path::{Path, PathBuf};

const VISIBLE_ROWS: usize = 9;
const ROW_H: f32 = 84.0;
const BROWSER_ROWS: usize = 12;

const ROW_BASE: u32 = 1000;
const DIFF_BASE: u32 = 2000;
const BR_BASE: u32 = 3000;
const ACT_IMPORT: u32 = 1;
const ACT_PLAY: u32 = 2;
const ACT_EDIT: u32 = 3;
const ACT_REANALYZE: u32 = 4;
const ACT_DELETE: u32 = 5;
const ACT_BACK: u32 = 6;
const ACT_CANCEL: u32 = 7;
const ACT_YES: u32 = 8;
const ACT_NO: u32 = 9;
const ACT_BR_ALL: u32 = 10;
const ACT_BR_UP: u32 = 11;
const ACT_BR_CLOSE: u32 = 12;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Mode { #[default] List, Browser, Confirm(Confirm) }

#[derive(Clone, Copy, PartialEq, Eq)]
enum Confirm { Delete, Reanalyze }

#[derive(Clone, Default)]
struct DiffInfo { summary: Option<MapSummary>, best: Option<ScoreRow> }

#[derive(Clone)]
struct Entry { name: String, path: PathBuf, is_dir: bool, size: u64 }

/// Explorador de carpetas que solo muestra carpetas y archivos de audio.
#[derive(Default)]
struct Browser { dir: PathBuf, entries: Vec<Entry>, sel: usize, scroll: usize, error: Option<String> }

impl Browser {
    fn open(dir: PathBuf) -> Browser {
        let mut b = Browser { dir, ..Default::default() };
        b.refresh();
        b
    }

    fn refresh(&mut self) {
        self.entries.clear();
        self.sel = 0;
        self.scroll = 0;
        self.error = None;
        if let Some(parent) = self.dir.parent() {
            self.entries.push(Entry { name: "..".into(), path: parent.to_path_buf(), is_dir: true, size: 0 });
        }
        match std::fs::read_dir(&self.dir) {
            Ok(rd) => {
                let (mut dirs, mut files) = (vec![], vec![]);
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().to_string();
                    if name.starts_with('.') { continue }
                    let path = e.path();
                    let Ok(md) = std::fs::metadata(&path) else { continue };
                    if md.is_dir() { dirs.push(Entry { name, path, is_dir: true, size: 0 }) }
                    else if import::is_audio_candidate(&path) { files.push(Entry { name, path, is_dir: false, size: md.len() }) }
                }
                dirs.sort_by_key(|e| e.name.to_lowercase());
                files.sort_by_key(|e| e.name.to_lowercase());
                self.entries.extend(dirs);
                self.entries.extend(files);
            }
            Err(e) => self.error = Some(format!("No se puede leer la carpeta: {e}")),
        }
    }

    fn audio_count(&self) -> usize { self.entries.iter().filter(|e| !e.is_dir).count() }

    fn go(&mut self, dir: PathBuf) {
        let came_from = self.dir.clone();
        self.dir = dir;
        self.refresh();
        // Al subir, se deja marcada la carpeta de la que venimos.
        if let Some(i) = self.entries.iter().position(|e| e.path == came_from) { self.sel = i; self.follow() }
    }

    fn up(&mut self) { if let Some(p) = self.dir.parent().map(Path::to_path_buf) { self.go(p) } }

    fn follow(&mut self) {
        if self.sel < self.scroll { self.scroll = self.sel }
        if self.sel >= self.scroll + BROWSER_ROWS { self.scroll = self.sel + 1 - BROWSER_ROWS }
    }

    fn step(&mut self, d: isize) {
        if self.entries.is_empty() { return }
        self.sel = (self.sel as isize + d).clamp(0, self.entries.len() as isize - 1) as usize;
        self.follow();
    }
}

#[derive(Resource)]
pub struct Library {
    songs: Vec<Song>,
    selected: usize,
    scroll: usize,
    diff: usize,
    info: Vec<DiffInfo>,
    sig: u64,
    dirty: bool,
    mode: Mode,
    browser: Browser,
    last_dir: Option<PathBuf>,
}

impl Default for Library {
    fn default() -> Self {
        Library { songs: vec![], selected: 0, scroll: 0, diff: 1, info: vec![DiffInfo::default(); Difficulty::ALL.len()], sig: u64::MAX, dirty: true,
            mode: Mode::List, browser: Browser::default(), last_dir: None }
    }
}

impl Library {
    fn current(&self) -> Option<&Song> { self.songs.get(self.selected) }

    fn follow(&mut self) {
        if self.selected < self.scroll { self.scroll = self.selected }
        if self.selected >= self.scroll + VISIBLE_ROWS { self.scroll = self.selected + 1 - VISIBLE_ROWS }
        self.scroll = self.scroll.min(self.songs.len().saturating_sub(VISIBLE_ROWS));
    }

    fn load_info(&mut self, db: &Db) {
        self.info = vec![DiffInfo::default(); Difficulty::ALL.len()];
        let Some(s) = self.songs.get(self.selected) else { return };
        if s.status != Status::Ready { return }
        for (i, d) in Difficulty::ALL.iter().enumerate() {
            self.info[i].summary = import::load_map(&paths::maps_dir(), &s.fingerprint, *d).ok().map(|m| import::summarize(&m));
            self.info[i].best = db.best(&s.fingerprint, d.label()).ok().flatten();
        }
    }

    /// Relee la lista conservando la canción seleccionada.
    fn reload(&mut self, db: &Db) {
        let keep = self.songs.get(self.selected).map(|s| s.id);
        self.songs = db.songs().unwrap_or_default();
        self.selected = keep.and_then(|id| self.songs.iter().position(|s| s.id == id))
            .unwrap_or_else(|| self.selected.min(self.songs.len().saturating_sub(1)));
        self.follow();
        self.load_info(db);
        self.dirty = true;
    }

    fn select(&mut self, i: usize, db: &Db) {
        if self.songs.is_empty() { return }
        let i = i.min(self.songs.len() - 1);
        if i != self.selected { self.selected = i; self.load_info(db) }
        self.follow();
        self.dirty = true;
    }
}

#[derive(Component)] struct LibRoot;

pub fn plugin(app: &mut App) {
    app.init_resource::<Library>()
        .add_systems(OnEnter(Screen::Library), enter)
        .add_systems(Update, (input, refresh, render).chain().run_if(in_state(Screen::Library)))
        .add_systems(Update, drop_files);
}

fn enter(mut lib: ResMut<Library>, db: Res<AppDb>) {
    lib.mode = Mode::List;
    if let Ok(db) = db.0.lock() { lib.reload(&db) }
}

/// Firma del estado de los trabajos: si cambia, hay que releer y redibujar.
fn jobs_signature(jobs: &Jobs) -> u64 {
    let p: u64 = jobs.active.iter().map(|j| (j.progress * 100.0) as u64 + 1000).sum();
    (jobs.generation as u64) << 32 ^ p ^ ((jobs.queued() as u64) << 20)
}

fn refresh(mut lib: ResMut<Library>, jobs: Res<Jobs>, db: Res<AppDb>) {
    let sig = jobs_signature(&jobs);
    if sig != lib.sig {
        lib.sig = sig;
        if let Ok(db) = db.0.lock() { lib.reload(&db) }
    }
}

/// Añade a la cola de análisis un archivo o todos los audios de una carpeta. Devuelve cuántos.
pub fn queue_path(jobs: &mut Jobs, settings: &Settings, p: &Path) -> usize {
    let files = if p.is_dir() { import::audio_files_in(p) }
        else if import::is_audio_candidate(p) { vec![p.to_path_buf()] }
        else { jobs.notice(format!("«{}»: formato no reconocido (se admiten {})", p.file_name().map_or(String::new(), |n| n.to_string_lossy().into()), import::AUDIO_EXTS.join(", ")), NoticeKind::Error); return 0 };
    if files.is_empty() { jobs.notice("No hay archivos de audio en esa carpeta", NoticeKind::Info); return 0 }
    let n = files.len();
    for f in files { jobs.start(f, None, settings) }
    if n > 1 { jobs.notice(format!("{n} canciones añadidas a la cola de análisis"), NoticeKind::Info) }
    n
}

fn drop_files(mut ev: MessageReader<FileDragAndDrop>, mut jobs: ResMut<Jobs>, settings: Res<Settings>) {
    for e in ev.read() {
        if let FileDragAndDrop::DroppedFile { path_buf, .. } = e { queue_path(&mut jobs, &settings, path_buf); }
    }
}

fn start_dir(last: &Option<PathBuf>) -> PathBuf {
    if let Some(d) = last.as_ref().filter(|d| d.is_dir()) { return d.clone() }
    let home = paths::home_dir();
    ["Music", "Música", "Musica"].iter().map(|m| home.join(m)).find(|d| d.is_dir()).unwrap_or(home)
}

enum Act {
    Move(isize),
    Page(isize),
    Jump(bool),
    SelectRow(usize),
    Diff(isize),
    SelectDiff(usize),
    Play,
    Import,
    Edit,
    Reanalyze,
    Delete,
    Cancel,
    Back,
    Yes,
    No,
    BrMove(isize),
    BrSelect(usize),
    BrOpen,
    BrUp,
    BrAll,
    BrJump(&'static str),
    BrClose,
}

#[allow(clippy::too_many_arguments)]
fn input(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut wheel: MessageReader<MouseWheel>,
    mut clicks: ResMut<Clicks>,
    mut lib: ResMut<Library>,
    db: Res<AppDb>,
    mut jobs: ResMut<Jobs>,
    settings: Res<Settings>,
    mut commands: Commands,
    mut next: ResMut<NextState<Screen>>,
    mut vertical: Local<KeyRepeat>,
) {
    let dt = time.delta_secs();
    let mut acts: Vec<Act> = vec![];
    let scroll: isize = wheel.read().map(|w| if w.y > 0.0 { -1 } else if w.y < 0.0 { 1 } else { 0 }).sum();
    let pressed = |k: KeyCode| keys.just_pressed(k);
    let confirm = pressed(KeyCode::Enter) || pressed(KeyCode::NumpadEnter);
    match lib.mode {
        Mode::List => {
            match vertical.poll(&keys, dt, &[KeyCode::ArrowDown, KeyCode::ArrowUp, KeyCode::PageDown, KeyCode::PageUp]) {
                Some(KeyCode::ArrowDown) => acts.push(Act::Move(1)),
                Some(KeyCode::ArrowUp) => acts.push(Act::Move(-1)),
                Some(KeyCode::PageDown) => acts.push(Act::Page(1)),
                Some(KeyCode::PageUp) => acts.push(Act::Page(-1)),
                _ => {}
            }
            if scroll != 0 { acts.push(Act::Move(scroll)) }
            if pressed(KeyCode::Home) { acts.push(Act::Jump(false)) }
            if pressed(KeyCode::End) { acts.push(Act::Jump(true)) }
            if pressed(KeyCode::ArrowLeft) { acts.push(Act::Diff(-1)) }
            if pressed(KeyCode::ArrowRight) { acts.push(Act::Diff(1)) }
            if confirm || pressed(KeyCode::Space) { acts.push(Act::Play) }
            if pressed(KeyCode::KeyI) { acts.push(Act::Import) }
            if pressed(KeyCode::KeyE) { acts.push(Act::Edit) }
            if pressed(KeyCode::KeyR) { acts.push(Act::Reanalyze) }
            if pressed(KeyCode::KeyC) { acts.push(Act::Cancel) }
            if pressed(KeyCode::Delete) || pressed(KeyCode::Backspace) { acts.push(Act::Delete) }
            if pressed(KeyCode::Escape) { acts.push(Act::Back) }
            for (id, a) in [(ACT_IMPORT, Act::Import), (ACT_PLAY, Act::Play), (ACT_EDIT, Act::Edit), (ACT_REANALYZE, Act::Reanalyze),
                            (ACT_DELETE, Act::Delete), (ACT_CANCEL, Act::Cancel), (ACT_BACK, Act::Back)] {
                if clicks.take(id) { acts.push(a) }
            }
            while let Some(i) = clicks.take_range(ROW_BASE, VISIBLE_ROWS as u32) { acts.push(Act::SelectRow(i as usize)) }
            while let Some(i) = clicks.take_range(DIFF_BASE, Difficulty::ALL.len() as u32) { acts.push(Act::SelectDiff(i as usize)) }
        }
        Mode::Confirm(_) => {
            if confirm || pressed(KeyCode::KeyY) { acts.push(Act::Yes) }
            if pressed(KeyCode::Escape) || pressed(KeyCode::KeyN) { acts.push(Act::No) }
            if clicks.take(ACT_YES) { acts.push(Act::Yes) }
            if clicks.take(ACT_NO) { acts.push(Act::No) }
        }
        Mode::Browser => {
            match vertical.poll(&keys, dt, &[KeyCode::ArrowDown, KeyCode::ArrowUp, KeyCode::PageDown, KeyCode::PageUp]) {
                Some(KeyCode::ArrowDown) => acts.push(Act::BrMove(1)),
                Some(KeyCode::ArrowUp) => acts.push(Act::BrMove(-1)),
                Some(KeyCode::PageDown) => acts.push(Act::BrMove(BROWSER_ROWS as isize)),
                Some(KeyCode::PageUp) => acts.push(Act::BrMove(-(BROWSER_ROWS as isize))),
                _ => {}
            }
            if scroll != 0 { acts.push(Act::BrMove(scroll * 3)) }
            if confirm { acts.push(Act::BrOpen) }
            if pressed(KeyCode::Backspace) { acts.push(Act::BrUp) }
            if pressed(KeyCode::KeyA) { acts.push(Act::BrAll) }
            if pressed(KeyCode::KeyH) { acts.push(Act::BrJump("home")) }
            if pressed(KeyCode::KeyM) { acts.push(Act::BrJump("music")) }
            if pressed(KeyCode::Escape) { acts.push(Act::BrClose) }
            if clicks.take(ACT_BR_ALL) { acts.push(Act::BrAll) }
            if clicks.take(ACT_BR_UP) { acts.push(Act::BrUp) }
            if clicks.take(ACT_BR_CLOSE) { acts.push(Act::BrClose) }
            while let Some(i) = clicks.take_range(BR_BASE, BROWSER_ROWS as u32) { acts.push(Act::BrSelect(i as usize)) }
        }
    }
    if acts.is_empty() { return }
    let Ok(db) = db.0.lock() else { return };

    for act in acts {
        match act {
            Act::Move(d) => { let i = (lib.selected as isize + d).clamp(0, lib.songs.len().max(1) as isize - 1) as usize; lib.select(i, &db) }
            Act::Page(d) => { let i = (lib.selected as isize + d * VISIBLE_ROWS as isize).clamp(0, lib.songs.len().max(1) as isize - 1) as usize; lib.select(i, &db) }
            Act::Jump(end) => { let i = if end { lib.songs.len().saturating_sub(1) } else { 0 }; lib.select(i, &db) }
            Act::SelectRow(vi) => {
                let i = lib.scroll + vi;
                if i == lib.selected { play(&mut lib, &mut jobs, &mut commands, &mut next) } else { lib.select(i, &db) }
            }
            Act::Diff(d) => { lib.diff = (lib.diff as isize + d).clamp(0, Difficulty::ALL.len() as isize - 1) as usize; lib.dirty = true }
            Act::SelectDiff(d) => {
                if d == lib.diff { play(&mut lib, &mut jobs, &mut commands, &mut next) } else { lib.diff = d; lib.dirty = true }
            }
            Act::Play => play(&mut lib, &mut jobs, &mut commands, &mut next),
            Act::Import => {
                let dir = start_dir(&lib.last_dir);
                lib.browser = Browser::open(dir);
                lib.mode = Mode::Browser;
                lib.dirty = true;
            }
            Act::Edit => {
                let Some(song) = lib.current().cloned() else { continue };
                if song.status != Status::Ready { jobs.notice("La canción aún no está lista para editar", NoticeKind::Info); continue }
                match mapedit::open_session(&song, Difficulty::ALL[lib.diff]) {
                    Ok(s) => { commands.insert_resource(s); next.set(Screen::Editor) }
                    Err(e) => jobs.notice(e, NoticeKind::Error),
                }
            }
            Act::Reanalyze => {
                let Some(song) = lib.current().cloned() else { continue };
                if jobs.busy_with(song.id) { jobs.notice("Esa canción ya se está analizando", NoticeKind::Info); continue }
                if lib.info.iter().any(|i| i.summary.as_ref().is_some_and(|s| s.edited)) { lib.mode = Mode::Confirm(Confirm::Reanalyze); lib.dirty = true }
                else { reanalyze(&song, &db, &mut jobs, &settings); lib.reload(&db) }
            }
            Act::Delete => {
                if lib.current().is_some() { lib.mode = Mode::Confirm(Confirm::Delete); lib.dirty = true }
            }
            Act::Cancel => {
                let Some(id) = lib.current().map(|s| s.id) else { continue };
                if !jobs.cancel(id) { jobs.notice("No hay ningún análisis en curso para esa canción", NoticeKind::Info) }
                else if !jobs.active.iter().any(|j| j.song_id == Some(id)) { let _ = db.set_status(id, Status::Cancelled, None, None); lib.reload(&db) }
            }
            Act::Back => next.set(Screen::Home),
            Act::Yes => {
                let Mode::Confirm(c) = lib.mode else { continue };
                lib.mode = Mode::List;
                let Some(song) = lib.current().cloned() else { continue };
                match c {
                    Confirm::Delete => {
                        jobs.cancel(song.id);
                        import::delete_maps(&paths::maps_dir(), &song.fingerprint);
                        let _ = db.delete_song(song.id);
                        jobs.notice(format!("«{}» eliminada de la biblioteca", song.title), NoticeKind::Info);
                    }
                    Confirm::Reanalyze => reanalyze(&song, &db, &mut jobs, &settings),
                }
                lib.reload(&db);
            }
            Act::No => { lib.mode = Mode::List; lib.dirty = true }
            Act::BrMove(d) => { lib.browser.step(d); lib.dirty = true }
            Act::BrSelect(vi) => {
                let i = lib.browser.scroll + vi;
                if i < lib.browser.entries.len() {
                    if i == lib.browser.sel { browser_open(&mut lib, &mut jobs, &settings) } else { lib.browser.sel = i; lib.dirty = true }
                }
            }
            Act::BrOpen => browser_open(&mut lib, &mut jobs, &settings),
            Act::BrUp => { lib.browser.up(); lib.dirty = true }
            Act::BrAll => {
                let dir = lib.browser.dir.clone();
                if lib.browser.audio_count() == 0 { jobs.notice("No hay archivos de audio en esta carpeta", NoticeKind::Info) }
                else { queue_path(&mut jobs, &settings, &dir); lib.last_dir = Some(dir); lib.mode = Mode::List; lib.dirty = true }
            }
            Act::BrJump(w) => {
                let home = paths::home_dir();
                let target = if w == "music" { ["Music", "Música", "Musica"].iter().map(|m| home.join(m)).find(|d| d.is_dir()) } else { Some(home) };
                if let Some(t) = target { lib.browser.go(t); lib.dirty = true }
            }
            Act::BrClose => { lib.mode = Mode::List; lib.dirty = true }
        }
    }
}

fn browser_open(lib: &mut Library, jobs: &mut Jobs, settings: &Settings) {
    let Some(e) = lib.browser.entries.get(lib.browser.sel).cloned() else { return };
    if e.is_dir {
        if e.name == ".." { lib.browser.up() } else { lib.browser.go(e.path) }
    } else {
        lib.last_dir = e.path.parent().map(Path::to_path_buf);
        queue_path(jobs, settings, &e.path);
        lib.mode = Mode::List;
    }
    lib.dirty = true;
}

fn reanalyze(song: &Song, db: &Db, jobs: &mut Jobs, settings: &Settings) {
    let path = PathBuf::from(&song.path);
    if !path.exists() {
        jobs.notice(format!("No se encuentra el audio original ({}). Impórtalo de nuevo desde su nueva ubicación.", paths::redact(&path)), NoticeKind::Error);
        return;
    }
    let _ = db.set_status(song.id, Status::Analyzing, None, None);
    jobs.start(path, Some(song.id), settings);
}

fn play(lib: &mut Library, jobs: &mut Jobs, commands: &mut Commands, next: &mut NextState<Screen>) {
    let Some(song) = lib.current().cloned() else { return };
    if song.status != Status::Ready { jobs.notice("La canción aún no está lista", NoticeKind::Info); return }
    let d = Difficulty::ALL[lib.diff];
    match import::load_map(&paths::maps_dir(), &song.fingerprint, d) {
        Err(e) => jobs.notice(format!("No se pudo abrir el mapa {}: {e}", d.label()), NoticeKind::Error),
        Ok(map) if map.notes.is_empty() => jobs.notice("Este mapa no tiene notas: prueba otra dificultad o sube la sensibilidad y reanaliza", NoticeKind::Error),
        Ok(map) => {
            let path = PathBuf::from(&song.path);
            if !path.exists() {
                jobs.notice(format!("No se encuentra el audio original ({}). Impórtalo de nuevo desde su nueva ubicación.", paths::redact(&path)), NoticeKind::Error);
            } else {
                commands.insert_resource(PlayRequest::new(map, Some(path)));
                commands.insert_resource(ReturnTo(Screen::Library));
                next.set(Screen::Playing);
            }
        }
    }
}

// --- Dibujo -------------------------------------------------------------------------------------

fn status_of(song: &Song, jobs: &Jobs) -> (String, Color) {
    if let Some((p, stage)) = jobs.progress_of(song.id) {
        return (if stage == "en cola" { "En cola".into() } else { format!("Analizando · {stage} · {:.0} %", p * 100.0) }, TEXT);
    }
    match song.status {
        Status::Ready => ("Listo".into(), TEXT_DIM),
        Status::Analyzing => ("Analizando…".into(), TEXT),
        Status::Failed => ("Error".into(), DANGER),
        Status::Cancelled => ("Cancelado".into(), TEXT_DIM),
    }
}

fn song_subtitle(song: &Song) -> String {
    if song.status == Status::Failed || (song.status == Status::Ready && song.error.is_some()) {
        if let Some(e) = &song.error { return ui::ellipsize(e, 70) }
    }
    let mut parts = vec![];
    if !song.artist.is_empty() { parts.push(song.artist.clone()) }
    parts.push(ui::fmt_duration(song.duration_ms));
    if let Some(b) = song.bpm { parts.push(format!("{b:.0} BPM")) }
    parts.join(" · ")
}

fn fmt_size(b: u64) -> String {
    if b >= 1 << 20 { format!("{:.1} MB", b as f64 / (1 << 20) as f64) } else { format!("{} KB", b.div_ceil(1024)) }
}

fn render(mut commands: Commands, mut lib: ResMut<Library>, fonts: Res<Fonts>, jobs: Res<Jobs>, roots: Query<Entity, With<LibRoot>>) {
    if !lib.dirty { return }
    lib.dirty = false;
    for r in &roots { commands.entity(r).despawn() }
    let lib = &*lib;
    commands.spawn((DespawnOnExit(Screen::Library), LibRoot,
        Node { flex_direction: FlexDirection::Column, padding: UiRect::new(px(96), px(96), px(64), px(40)), row_gap: px(28), ..ui::full_screen_root() }))
        .with_children(|root| {
            // Cabecera
            root.spawn(Node { justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::FlexEnd, width: percent(100), ..default() }).with_children(|h| {
                h.spawn(ui::spaced(&fonts, "BIBLIOTECA", 38.0, TEXT, Weight::Light, 10.0));
                let n = lib.songs.len();
                h.spawn(ui::text(&fonts, match n { 0 => "sin canciones".to_string(), 1 => "1 canción".to_string(), n => format!("{n} canciones") }, 20.0, TEXT_DIM, Weight::Regular));
            });
            if lib.songs.is_empty() { empty_state(root, &fonts) } else { body(root, lib, &fonts, &jobs) }
            footer(root, lib, &fonts, &jobs);
            match lib.mode {
                Mode::Browser => browser_modal(root, lib, &fonts),
                Mode::Confirm(c) => confirm_modal(root, lib, c, &fonts),
                Mode::List => {}
            }
        });
}

fn empty_state(root: &mut ChildSpawnerCommands, fonts: &Fonts) {
    root.spawn(Node { flex_grow: 1.0, flex_direction: FlexDirection::Column, justify_content: JustifyContent::Center, row_gap: px(14), padding: UiRect::left(px(8)), ..default() }).with_children(|c| {
        c.spawn(ui::text(fonts, "Tu biblioteca está vacía", 52.0, TEXT, Weight::Light));
        c.spawn(ui::text(fonts, "Importa una canción y Nocturne generará seis mapas de ritmo, de Easy a Extreme, a partir de ella.", 24.0, TEXT_DIM, Weight::Regular));
        c.spawn(ui::text(fonts, "Pulsa I para elegir un archivo, o arrastra uno (o una carpeta) a esta ventana.", 24.0, TEXT_DIM, Weight::Regular));
        c.spawn(ui::text(fonts, "Formatos: MP3, WAV, FLAC, OGG, M4A/AAC, y la pista de audio de MP4/MOV.", 18.0, alpha(TEXT_DIM, 0.7), Weight::Regular));
    });
}


fn body(root: &mut ChildSpawnerCommands, lib: &Library, fonts: &Fonts, jobs: &Jobs) {
    root.spawn(Node { flex_grow: 1.0, column_gap: px(72), width: percent(100), ..default() }).with_children(|body| {
        // Lista
        body.spawn(Node { flex_direction: FlexDirection::Column, width: px(900), row_gap: px(4), ..default() }).with_children(|list| {
            for (vi, song) in lib.songs.iter().enumerate().skip(lib.scroll).take(VISIBLE_ROWS) {
                let sel = vi == lib.selected;
                let (status, status_col) = status_of(song, jobs);
                let bg = if sel { SURFACE_2 } else { Color::NONE };
                list.spawn((ui::click(ROW_BASE + (vi - lib.scroll) as u32),
                    Node { width: percent(100), height: px(ROW_H), padding: UiRect::axes(px(24), px(0)), align_items: AlignItems::Center,
                           justify_content: JustifyContent::SpaceBetween, border: UiRect::left(px(3)), ..default() },
                    BackgroundColor(bg), BorderColor::all(if sel { WHITE } else { Color::NONE }),
                    HoverTint { normal: bg, hover: if sel { SURFACE_2 } else { SURFACE } }))
                    .with_children(|r| {
                        r.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(4), ..default() }).with_children(|c| {
                            c.spawn(ui::text(fonts, ui::ellipsize(&song.title, 40), 28.0, if sel { WHITE } else { TEXT }, Weight::Medium));
                            c.spawn(ui::text(fonts, song_subtitle(song), 18.0, if song.status == Status::Failed { DANGER } else { TEXT_DIM }, Weight::Regular));
                        });
                        r.spawn(ui::text(fonts, status, 18.0, status_col, Weight::Regular));
                    });
            }
            if lib.songs.len() > VISIBLE_ROWS {
                list.spawn(Node { margin: UiRect::top(px(8)), padding: UiRect::left(px(24)), ..default() })
                    .with_children(|c| { c.spawn(ui::text(fonts, format!("{} de {}", lib.selected + 1, lib.songs.len()), 16.0, alpha(TEXT_DIM, 0.8), Weight::Regular)); });
            }
        });
        // Detalle
        if let Some(song) = lib.current() { detail(body, lib, song, fonts, jobs) }
    });
}

fn detail(body: &mut ChildSpawnerCommands, lib: &Library, song: &Song, fonts: &Fonts, jobs: &Jobs) {
    body.spawn(Node { flex_direction: FlexDirection::Column, flex_grow: 1.0, row_gap: px(10), padding: UiRect::top(px(8)), ..default() }).with_children(|d| {
        d.spawn(ui::text(fonts, ui::ellipsize(&song.title, 30), 46.0, TEXT, Weight::Medium));
        d.spawn(ui::text(fonts, song_subtitle(song), 20.0, TEXT_DIM, Weight::Regular));
        let t0 = crate::db::now();
        let history = match song.last_played {
            Some(p) => format!("añadida {} · jugada {}", crate::stats::ago(t0, song.added), crate::stats::ago(t0, p)),
            None => format!("añadida {} · aún sin jugar", crate::stats::ago(t0, song.added)),
        };
        d.spawn(ui::text(fonts, history, 16.0, alpha(TEXT_DIM, 0.8), Weight::Regular));
        d.spawn(ui::gap(14.0));
        match (song.status, jobs.progress_of(song.id)) {
            (_, Some((p, stage))) => {
                d.spawn(ui::text(fonts, if stage == "en cola" { "En cola de análisis".to_string() } else { format!("Analizando · {stage}") }, 24.0, TEXT, Weight::Regular));
                d.spawn((Node { width: px(560), height: px(4), ..default() }, BackgroundColor(BORDER)))
                    .with_children(|b| { b.spawn((Node { width: percent(p * 100.0), height: percent(100), ..default() }, BackgroundColor(TEXT))); });
                d.spawn(ui::text(fonts, "C cancela el análisis", 16.0, TEXT_DIM, Weight::Regular));
            }
            (Status::Ready, _) => {
                d.spawn(ui::text(fonts, "DIFICULTAD", 16.0, TEXT_DIM, Weight::Medium));
                for (i, diff) in Difficulty::ALL.iter().enumerate() {
                    let sel = i == lib.diff;
                    let info = &lib.info[i];
                    let bg = if sel { SURFACE_2 } else { Color::NONE };
                    d.spawn((ui::click(DIFF_BASE + i as u32),
                        Node { width: px(780), padding: UiRect::axes(px(22), px(9)), align_items: AlignItems::Center, justify_content: JustifyContent::SpaceBetween,
                               border: UiRect::left(px(3)), ..default() },
                        BackgroundColor(bg), BorderColor::all(if sel { WHITE } else { Color::NONE }),
                        HoverTint { normal: bg, hover: if sel { SURFACE_2 } else { SURFACE } }))
                        .with_children(|r| {
                            r.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(2), ..default() }).with_children(|c| {
                                c.spawn(ui::text(fonts, diff.label(), 30.0, if sel { WHITE } else { TEXT_DIM }, Weight::Regular));
                                let line = match &info.summary {
                                    Some(s) => format!("nivel {:.1} · {} notas · {:.1} n/s{}{}", s.rating, s.notes, s.nps,
                                        if s.holds > 0 { format!(" · {} largas", s.holds) } else { String::new() }, if s.edited { " · editado" } else { "" }),
                                    None => "no generado: pulsa R para crear este nivel".to_string(),
                                };
                                c.spawn(ui::text(fonts, line, 17.0, TEXT_DIM, Weight::Regular));
                            });
                            match &info.best {
                                Some(b) => {
                                    r.spawn(Node { flex_direction: FlexDirection::Column, align_items: AlignItems::FlexEnd, ..default() }).with_children(|c| {
                                        c.spawn(ui::text(fonts, grade_for(b.accuracy), 34.0, WHITE, Weight::Light));
                                        c.spawn(ui::text(fonts, format!("{:07} · {:.2} %", b.score, b.accuracy), 16.0, TEXT_DIM, Weight::Regular));
                                    });
                                }
                                None => { r.spawn(ui::text(fonts, "sin jugar", 16.0, alpha(TEXT_DIM, 0.7), Weight::Regular)); }
                            }
                        });
                }
            }
            (Status::Failed, _) => {
                d.spawn(ui::text(fonts, "El análisis falló", 26.0, DANGER, Weight::Regular));
                d.spawn(ui::text(fonts, song.error.clone().unwrap_or_default(), 18.0, TEXT_DIM, Weight::Regular));
                d.spawn(ui::text(fonts, "R lo vuelve a intentar", 16.0, TEXT_DIM, Weight::Regular));
            }
            (Status::Cancelled, _) => {
                d.spawn(ui::text(fonts, "Análisis cancelado", 26.0, TEXT, Weight::Regular));
                d.spawn(ui::text(fonts, "R lo reanuda desde cero", 16.0, TEXT_DIM, Weight::Regular));
            }
            (Status::Analyzing, _) => { d.spawn(ui::text(fonts, "Analizando…", 24.0, TEXT, Weight::Regular)); }
        }
    });
}

fn footer(root: &mut ChildSpawnerCommands, lib: &Library, fonts: &Fonts, jobs: &Jobs) {
    root.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(14), width: percent(100), ..default() }).with_children(|f| {
        f.spawn((ui::text(fonts, jobs.summary().unwrap_or_default(), 18.0, TEXT, Weight::Regular), Node { height: px(24), ..default() }));
        f.spawn(ui::divider(percent(100)));
        f.spawn(Node { column_gap: px(14), align_items: AlignItems::Center, ..default() }).with_children(|b| {
            b.spawn(ui::pill(fonts, ACT_IMPORT, "Importar · I"));
            if !lib.songs.is_empty() {
                b.spawn(ui::pill(fonts, ACT_PLAY, "Jugar · Intro"));
                b.spawn(ui::pill(fonts, ACT_EDIT, "Editar mapa · E"));
                b.spawn(ui::pill(fonts, ACT_REANALYZE, "Reanalizar · R"));
                b.spawn(ui::pill(fonts, ACT_DELETE, "Eliminar · Supr"));
            }
            b.spawn(Node { flex_grow: 1.0, ..default() });
            b.spawn(ui::text(fonts, "↑↓ canción   ←→ dificultad", 16.0, alpha(TEXT_DIM, 0.8), Weight::Regular));
            b.spawn(ui::pill(fonts, ACT_BACK, "Volver · Esc"));
        });
    });
}

fn browser_modal(root: &mut ChildSpawnerCommands, lib: &Library, fonts: &Fonts) {
    let b = &lib.browser;
    root.spawn((ui::modal_backdrop(), Node { justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..ui::full_screen_root() })).with_children(|m| {
        m.spawn((Node { width: px(1100), flex_direction: FlexDirection::Column, row_gap: px(10), padding: UiRect::all(px(36)), border: UiRect::all(px(1)), ..default() },
            BackgroundColor(SURFACE), BorderColor::all(BORDER))).with_children(|p| {
            p.spawn(ui::spaced(fonts, "IMPORTAR MÚSICA", 26.0, TEXT, Weight::Light, 8.0));
            let shown = b.dir.to_string_lossy().to_string();
            let tail: String = if shown.chars().count() > 78 { format!("…{}", shown.chars().rev().take(77).collect::<Vec<_>>().into_iter().rev().collect::<String>()) } else { shown };
            p.spawn(ui::text(fonts, tail, 18.0, TEXT_DIM, Weight::Regular));
            p.spawn(ui::gap(6.0));
            if let Some(e) = &b.error { p.spawn(ui::text(fonts, e.clone(), 18.0, DANGER, Weight::Regular)); }
            for (vi, e) in b.entries.iter().enumerate().skip(b.scroll).take(BROWSER_ROWS) {
                let sel = vi == b.sel;
                let bg = if sel { SURFACE_2 } else { Color::NONE };
                let label = if e.name == ".." { "↑  carpeta superior".to_string() } else if e.is_dir { format!("{}/", ui::ellipsize(&e.name, 70)) } else { ui::ellipsize(&e.name, 70) };
                p.spawn((ui::click(BR_BASE + (vi - b.scroll) as u32),
                    Node { width: percent(100), height: px(44), padding: UiRect::axes(px(16), px(0)), align_items: AlignItems::Center, justify_content: JustifyContent::SpaceBetween,
                           border: UiRect::left(px(3)), ..default() },
                    BackgroundColor(bg), BorderColor::all(if sel { WHITE } else { Color::NONE }),
                    HoverTint { normal: bg, hover: if sel { SURFACE_2 } else { BG } }))
                    .with_children(|r| {
                        r.spawn(ui::text(fonts, label, 22.0, if sel { WHITE } else if e.is_dir { TEXT } else { TEXT_DIM }, Weight::Regular));
                        if !e.is_dir { r.spawn(ui::text(fonts, fmt_size(e.size), 16.0, TEXT_DIM, Weight::Regular)); }
                    });
            }
            if b.entries.len() <= 1 && b.error.is_none() { p.spawn(ui::text(fonts, "Aquí no hay carpetas ni archivos de audio.", 18.0, TEXT_DIM, Weight::Regular)); }
            p.spawn(ui::gap(8.0));
            p.spawn(ui::divider(percent(100)));
            p.spawn(Node { column_gap: px(12), align_items: AlignItems::Center, ..default() }).with_children(|r| {
                r.spawn(ui::pill(fonts, ACT_BR_UP, "Subir · Retroceso"));
                if b.audio_count() > 0 { r.spawn(ui::pill(fonts, ACT_BR_ALL, &format!("Importar toda la carpeta ({}) · A", b.audio_count()))); }
                r.spawn(Node { flex_grow: 1.0, ..default() });
                r.spawn(ui::pill(fonts, ACT_BR_CLOSE, "Cerrar · Esc"));
            });
            p.spawn(ui::text(fonts, "↑↓ mover   Intro abrir o importar   H carpeta personal   M música", 16.0, alpha(TEXT_DIM, 0.8), Weight::Regular));
        });
    });
}

fn confirm_modal(root: &mut ChildSpawnerCommands, lib: &Library, c: Confirm, fonts: &Fonts) {
    let title = lib.current().map(|s| s.title.clone()).unwrap_or_default();
    let (head, msg, yes) = match c {
        Confirm::Delete => (format!("¿Eliminar «{}»?", ui::ellipsize(&title, 36)),
            "Se borrarán sus mapas y su lugar en la biblioteca. El archivo de audio original no se toca; tus puntuaciones se conservan en las estadísticas.", "Eliminar · Intro"),
        Confirm::Reanalyze => (format!("¿Reanalizar «{}»?", ui::ellipsize(&title, 36)),
            "Algún mapa de esta canción tiene ediciones manuales. Al reanalizar se generan los seis mapas de nuevo y esas ediciones se perderán.", "Reanalizar · Intro"),
    };
    root.spawn((ui::modal_backdrop(), Node { justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..ui::full_screen_root() })).with_children(|m| {
        m.spawn((Node { width: px(780), flex_direction: FlexDirection::Column, row_gap: px(16), padding: UiRect::all(px(40)), border: UiRect::all(px(1)), ..default() },
            BackgroundColor(SURFACE), BorderColor::all(BORDER))).with_children(|p| {
            p.spawn(ui::text(fonts, head, 34.0, TEXT, Weight::Medium));
            p.spawn(ui::text(fonts, msg, 20.0, TEXT_DIM, Weight::Regular));
            p.spawn(ui::gap(8.0));
            p.spawn(Node { column_gap: px(14), ..default() }).with_children(|r| {
                r.spawn(ui::pill(fonts, ACT_YES, yes));
                r.spawn(ui::pill(fonts, ACT_NO, "Cancelar · Esc"));
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nocturne-lib-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn browser_lists_dirs_then_audio_and_hides_the_rest() {
        let d = tmp("br");
        std::fs::create_dir(d.join("Zeta")).unwrap();
        std::fs::create_dir(d.join("alfa")).unwrap();
        std::fs::create_dir(d.join(".oculta")).unwrap();
        for f in ["b.mp3", "A.flac", "notas.txt", ".secreto.mp3"] { std::fs::write(d.join(f), vec![0u8; 2048]).unwrap(); }
        let b = Browser::open(d.clone());
        let names: Vec<&str> = b.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["..", "alfa", "Zeta", "A.flac", "b.mp3"]);
        assert_eq!(b.audio_count(), 2);
        assert_eq!(b.entries[3].size, 2048);
    }

    #[test]
    fn browser_navigation_remembers_where_it_came_from() {
        let d = tmp("nav");
        std::fs::create_dir(d.join("uno")).unwrap();
        std::fs::create_dir(d.join("dos")).unwrap();
        let mut b = Browser::open(d.clone());
        let uno = b.entries.iter().position(|e| e.name == "uno").unwrap();
        b.sel = uno;
        b.go(d.join("uno"));
        assert_eq!(b.dir, d.join("uno"));
        b.up();
        assert_eq!(b.dir, d);
        assert_eq!(b.entries[b.sel].name, "uno", "al subir queda marcada la carpeta de origen");
        b.step(-100);
        assert_eq!(b.sel, 0);
        b.step(100);
        assert_eq!(b.sel, b.entries.len() - 1);
    }

    #[test]
    fn browser_scroll_follows_selection() {
        let d = tmp("scroll");
        for i in 0..30 { std::fs::write(d.join(format!("t{i:02}.wav")), b"x").unwrap(); }
        let mut b = Browser::open(d);
        for _ in 0..20 { b.step(1) }
        assert!(b.sel >= b.scroll && b.sel < b.scroll + BROWSER_ROWS);
        for _ in 0..20 { b.step(-1) }
        assert_eq!((b.sel, b.scroll), (0, 0));
    }

    #[test]
    fn unreadable_directory_reports_error_instead_of_panicking() {
        let b = Browser::open(PathBuf::from("/no/existe/de/verdad"));
        assert!(b.error.is_some());
    }

    #[test]
    fn library_selection_and_scrolling_stay_in_range() {
        let db = Db::open_memory().unwrap();
        for i in 0..20 { db.add_song(&format!("fp{i:02}"), "/x.mp3", &format!("Canción {i}"), "", 1000).unwrap(); }
        let mut lib = Library::default();
        lib.reload(&db);
        assert_eq!(lib.songs.len(), 20);
        lib.select(100, &db);
        assert_eq!(lib.selected, 19);
        assert_eq!(lib.scroll, 20 - VISIBLE_ROWS);
        lib.select(0, &db);
        assert_eq!(lib.scroll, 0);
        // eliminar la seleccionada deja una selección válida
        let id = lib.current().unwrap().id;
        db.delete_song(id).unwrap();
        lib.reload(&db);
        assert!(lib.selected < lib.songs.len());
        let empty = Db::open_memory().unwrap();
        let mut lib = Library::default();
        lib.reload(&empty);
        lib.select(5, &empty);
        assert!(lib.current().is_none());
    }

    #[test]
    fn size_and_status_formatting() {
        assert_eq!(fmt_size(1), "1 KB");
        assert_eq!(fmt_size(5 * 1024 * 1024 + 512 * 1024), "5.5 MB");
        let song = Song { id: 1, fingerprint: "f".into(), path: "/p".into(), title: "T".into(), artist: String::new(), duration_ms: 222_000,
            bpm: Some(127.6), status: Status::Ready, error: None, added: 0, last_played: None };
        assert_eq!(song_subtitle(&song), "3:42 · 128 BPM");
        let failed = Song { status: Status::Failed, error: Some("audio corrupto".into()), ..song.clone() };
        assert_eq!(song_subtitle(&failed), "audio corrupto");
        assert_eq!(status_of(&failed, &Jobs::default()).0, "Error");
    }
}
