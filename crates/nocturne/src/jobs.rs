//! Importaciones/análisis en segundo plano. Cada trabajo tiene su hilo, su conexión SQLite (WAL)
//! y una bandera de cancelación; la interfaz solo lee mensajes y nunca se bloquea.

use crate::db::{Db, Status};
use crate::import::{analyze_and_generate, probe};
use crate::paths;
use crate::settings::Settings;
use bevy::prelude::*;
use nocturne_core::Error;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Instant;

pub enum Msg {
    Progress { key: u64, p: f32, stage: &'static str },
    Started { key: u64, song_id: i64, title: String },
    Done { key: u64, result: Result<String, String>, cancelled: bool },
    Duplicate { title: String },
}

pub struct Job { pub key: u64, pub song_id: Option<i64>, pub title: String, pub cancel: Arc<AtomicBool>, pub progress: f32, pub stage: &'static str }

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind { Info, Ok, Error }

#[derive(Resource)]
pub struct Jobs {
    pub active: Vec<Job>,
    tx: mpsc::Sender<Msg>,
    rx: Mutex<mpsc::Receiver<Msg>>,
    next_key: u64,
    /// Sube cada vez que cambia la biblioteca (alta, fin de análisis…).
    pub generation: u32,
    pub notices: Vec<(String, NoticeKind, Instant)>,
}

impl Default for Jobs {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Jobs { active: vec![], tx, rx: Mutex::new(rx), next_key: 1, generation: 0, notices: vec![] }
    }
}

impl Jobs {
    pub fn summary(&self) -> Option<String> {
        let j = self.active.first()?;
        Some(format!("Analizando «{}» · {} · {:.0} %{}", j.title, j.stage, j.progress * 100.0,
            if self.active.len() > 1 { format!("  (+{} más)", self.active.len() - 1) } else { String::new() }))
    }
    pub fn busy_with(&self, song_id: i64) -> bool { self.active.iter().any(|j| j.song_id == Some(song_id)) }
    pub fn cancel(&self, song_id: i64) { for j in self.active.iter().filter(|j| j.song_id == Some(song_id)) { j.cancel.store(true, Ordering::Relaxed) } }
    pub fn notice(&mut self, text: impl Into<String>, kind: NoticeKind) { self.notices.push((text.into(), kind, Instant::now())) }

    /// Importa un archivo nuevo, o reanaliza una canción existente (`existing`).
    pub fn start(&mut self, path: PathBuf, existing: Option<i64>, settings: &Settings) {
        let key = self.next_key;
        self.next_key += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        let title = path.file_stem().and_then(|s| s.to_str()).unwrap_or("canción").to_string();
        self.active.push(Job { key, song_id: existing, title: title.clone(), cancel: cancel.clone(), progress: 0.0, stage: "leyendo" });
        let tx = self.tx.clone();
        let settings = settings.clone();
        std::thread::Builder::new().name("nocturne-import".into()).spawn(move || run(key, path, existing, settings, cancel, tx)).ok();
    }
}

fn run(key: u64, path: PathBuf, existing: Option<i64>, settings: Settings, cancel: Arc<AtomicBool>, tx: mpsc::Sender<Msg>) {
    let (db, _) = Db::open(&paths::db_path());
    let done = |result: Result<String, String>, cancelled: bool| { let _ = tx.send(Msg::Done { key, result, cancelled }); };
    let _ = tx.send(Msg::Progress { key, p: 0.02, stage: "decodificando" });
    let probed = match probe(&path, &cancel) {
        Ok(p) => p,
        Err(Error::Cancelled) => return done(Err("cancelado".into()), true),
        Err(e) => {
            if let Some(id) = existing { let _ = db.set_status(id, Status::Failed, Some(&e.to_string()), None); }
            return done(Err(e.to_string()), false);
        }
    };
    let duration_ms = probed.audio.duration().0 / 1000;
    let song_id = match existing {
        Some(id) => id,
        None => match db.song_by_fingerprint(&probed.fingerprint) {
            Ok(Some(s)) if s.status == Status::Ready => { let _ = tx.send(Msg::Duplicate { title: s.title }); return done(Ok("duplicado".into()), false) }
            Ok(Some(s)) => { let _ = db.update_path(s.id, &path.to_string_lossy()); let _ = db.set_status(s.id, Status::Analyzing, None, None); s.id }
            _ => match db.add_song(&probed.fingerprint, &path.to_string_lossy(), &probed.title, "", duration_ms) {
                Ok(id) => id,
                Err(e) => return done(Err(format!("base de datos: {e}")), false),
            },
        },
    };
    let _ = tx.send(Msg::Started { key, song_id, title: probed.title.clone() });
    let mut last = Instant::now();
    let mut progress = |p: f32, stage: &'static str| {
        if last.elapsed().as_millis() > 80 || p >= 1.0 { last = Instant::now(); let _ = tx.send(Msg::Progress { key, p: 0.1 + p * 0.9, stage }); }
    };
    match analyze_and_generate(&probed, "", &settings, &paths::maps_dir(), &cancel, &mut progress) {
        Ok(out) => {
            let _ = db.set_status(song_id, Status::Ready, out.warnings.first().map(|s| s.as_str()).filter(|_| out.notes.iter().all(|n| *n == 0)), out.bpm);
            let msg = format!("«{}» listo · {} BPM · notas {:?} · {} ms", probed.title, out.bpm.map_or("?".into(), |b| format!("{b:.0}")), out.notes, out.elapsed_ms);
            done(Ok(msg), false)
        }
        Err(Error::Cancelled) => { let _ = db.set_status(song_id, Status::Cancelled, None, None); done(Err("cancelado".into()), true) }
        Err(e) => { let _ = db.set_status(song_id, Status::Failed, Some(&e.to_string()), None); done(Err(e.to_string()), false) }
    }
}

/// Aplica los mensajes de los hilos al estado visible.
pub fn poll(mut jobs: ResMut<Jobs>) {
    let msgs: Vec<Msg> = { let rx = jobs.rx.lock().unwrap(); std::iter::from_fn(|| rx.try_recv().ok()).collect() };
    for m in msgs {
        match m {
            Msg::Progress { key, p, stage } => if let Some(j) = jobs.active.iter_mut().find(|j| j.key == key) { j.progress = p; j.stage = stage },
            Msg::Started { key, song_id, title } => {
                if let Some(j) = jobs.active.iter_mut().find(|j| j.key == key) { j.song_id = Some(song_id); j.title = title }
                jobs.generation += 1;
            }
            Msg::Duplicate { title } => jobs.notice(format!("«{title}» ya está en la biblioteca"), NoticeKind::Info),
            Msg::Done { key, result, cancelled } => {
                jobs.active.retain(|j| j.key != key);
                jobs.generation += 1;
                match (result, cancelled) {
                    (_, true) => jobs.notice("Análisis cancelado", NoticeKind::Info),
                    (Ok(m), _) if m == "duplicado" => {}
                    (Ok(m), _) => jobs.notice(m, NoticeKind::Ok),
                    (Err(e), _) => jobs.notice(format!("No se pudo analizar: {e}"), NoticeKind::Error),
                }
            }
        }
    }
}
