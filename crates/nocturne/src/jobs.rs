//! Importaciones/análisis en segundo plano. Cada trabajo tiene su hilo, su conexión SQLite (WAL)
//! y una bandera de cancelación; la interfaz solo lee mensajes y nunca se bloquea.
//! Hay como mucho `MAX_PARALLEL` análisis a la vez (cada uno retiene la pista decodificada en
//! memoria); el resto espera en cola.

use crate::db::{Db, Status};
use crate::import::{analyze_and_generate, probe};
use crate::paths;
use crate::settings::Settings;
use bevy::prelude::*;
use nocturne_core::Error;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Instant;

pub const MAX_PARALLEL: usize = 2;

pub enum Msg {
    Progress { key: u64, p: f32, stage: &'static str },
    Started { key: u64, song_id: i64, title: String },
    Done { key: u64, result: Result<String, String>, cancelled: bool },
    Duplicate { title: String },
    Relinked { title: String },
}

pub struct Job { pub key: u64, pub song_id: Option<i64>, pub path: PathBuf, pub title: String, pub cancel: Arc<AtomicBool>, pub progress: f32, pub stage: &'static str }

struct Queued { path: PathBuf, existing: Option<i64>, settings: Settings }

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind { Info, Ok, Error }

#[derive(Resource)]
pub struct Jobs {
    pub active: Vec<Job>,
    queue: VecDeque<Queued>,
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
        Jobs { active: vec![], queue: VecDeque::new(), tx, rx: Mutex::new(rx), next_key: 1, generation: 0, notices: vec![] }
    }
}

fn stem(path: &Path) -> String { path.file_stem().and_then(|s| s.to_str()).unwrap_or("canción").to_string() }

impl Jobs {
    pub fn summary(&self) -> Option<String> {
        let j = self.active.first()?;
        let more = self.active.len() - 1 + self.queue.len();
        Some(format!("Analizando «{}» · {} · {:.0} %{}", j.title, j.stage, j.progress * 100.0,
            if more > 0 { format!("  (+{more} más)") } else { String::new() }))
    }
    pub fn queued(&self) -> usize { self.queue.len() }
    pub fn busy_with(&self, song_id: i64) -> bool {
        self.active.iter().any(|j| j.song_id == Some(song_id)) || self.queue.iter().any(|q| q.existing == Some(song_id))
    }
    /// Progreso (0..1) y etapa del trabajo de una canción, si lo hay.
    pub fn progress_of(&self, song_id: i64) -> Option<(f32, &'static str)> {
        if let Some(j) = self.active.iter().find(|j| j.song_id == Some(song_id)) { return Some((j.progress, j.stage)) }
        self.queue.iter().any(|q| q.existing == Some(song_id)).then_some((0.0, "en cola"))
    }
    /// Cancela el trabajo de una canción (activo o en cola). Devuelve `true` si había algo.
    pub fn cancel(&mut self, song_id: i64) -> bool {
        let mut any = false;
        for j in self.active.iter().filter(|j| j.song_id == Some(song_id)) { j.cancel.store(true, Ordering::Relaxed); any = true }
        let before = self.queue.len();
        self.queue.retain(|q| q.existing != Some(song_id));
        any || self.queue.len() != before
    }
    pub fn notice(&mut self, text: impl Into<String>, kind: NoticeKind) { self.notices.push((text.into(), kind, Instant::now())) }

    /// Importa un archivo nuevo, o reanaliza una canción existente (`existing`).
    pub fn start(&mut self, path: PathBuf, existing: Option<i64>, settings: &Settings) {
        // Ruta absoluta: la biblioteca la guarda y debe seguir valiendo al cambiar de directorio de trabajo.
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        let dup = self.active.iter().any(|j| j.path == path) || self.queue.iter().any(|q| q.path == path);
        if dup { return }
        self.queue.push_back(Queued { path, existing, settings: settings.clone() });
        self.pump();
    }

    /// Lanza trabajos de la cola mientras haya hueco.
    fn pump(&mut self) {
        while self.active.len() < MAX_PARALLEL {
            let Some(q) = self.queue.pop_front() else { break };
            let key = self.next_key;
            self.next_key += 1;
            let cancel = Arc::new(AtomicBool::new(false));
            let title = stem(&q.path);
            self.active.push(Job { key, song_id: q.existing, path: q.path.clone(), title, cancel: cancel.clone(), progress: 0.0, stage: "leyendo" });
            let tx = self.tx.clone();
            let Queued { path, existing, settings } = q;
            std::thread::Builder::new().name("nocturne-import".into()).spawn(move || run(key, path, existing, settings, cancel, tx)).ok();
        }
    }
}

fn run(key: u64, path: PathBuf, existing: Option<i64>, settings: Settings, cancel: Arc<AtomicBool>, tx: mpsc::Sender<Msg>) {
    let open_db = || Db::open(&paths::db_path()).0;
    let done = |result: Result<String, String>, cancelled: bool| { let _ = tx.send(Msg::Done { key, result, cancelled }); };
    let _ = tx.send(Msg::Progress { key, p: 0.02, stage: "decodificando" });
    let probed = match probe(&path, &cancel) {
        Ok(p) => p,
        Err(Error::Cancelled) => {
            if let Some(id) = existing { let _ = open_db().set_status(id, Status::Cancelled, None, None); }
            return done(Err("cancelado".into()), true)
        }
        Err(e) => {
            if let Some(id) = existing { let _ = open_db().set_status(id, Status::Failed, Some(&e.to_string()), None); }
            return done(Err(e.to_string()), false);
        }
    };
    let db = open_db();
    let duration_ms = probed.audio.duration().0 / 1000;
    let song_id = match existing {
        Some(id) => id,
        None => match db.song_by_fingerprint(&probed.fingerprint) {
            // Mismo audio ya importado: si el archivo original desapareció, se enlaza la nueva ubicación.
            Ok(Some(s)) if s.status == Status::Ready => {
                if !Path::new(&s.path).exists() {
                    let _ = db.update_path(s.id, &path.to_string_lossy());
                    let _ = tx.send(Msg::Relinked { title: s.title });
                } else {
                    let _ = tx.send(Msg::Duplicate { title: s.title });
                }
                return done(Ok("duplicado".into()), false)
            }
            Ok(Some(s)) => { let _ = db.update_path(s.id, &path.to_string_lossy()); let _ = db.set_status(s.id, Status::Analyzing, None, None); s.id }
            _ => match db.add_song(&probed.fingerprint, &path.to_string_lossy(), &probed.title, "", duration_ms) {
                Ok(id) => id,
                Err(e) => return done(Err(format!("base de datos: {e}")), false),
            },
        },
    };
    let _ = db.set_status(song_id, Status::Analyzing, None, None);
    let _ = tx.send(Msg::Started { key, song_id, title: probed.title.clone() });
    let mut last = Instant::now();
    let mut progress = |p: f32, stage: &'static str| {
        if last.elapsed().as_millis() > 80 || p >= 1.0 { last = Instant::now(); let _ = tx.send(Msg::Progress { key, p: 0.1 + p * 0.9, stage }); }
    };
    match analyze_and_generate(&probed, "", &settings, &paths::maps_dir(), false, &cancel, &mut progress) {
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
            Msg::Relinked { title } => { jobs.generation += 1; jobs.notice(format!("«{title}»: archivo de audio enlazado a su nueva ubicación"), NoticeKind::Ok) }
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
    jobs.pump();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_limits_parallelism_and_dedups_paths() {
        let mut j = Jobs::default();
        let s = Settings::default();
        // Rutas inexistentes: los hilos fallan enseguida, pero la cola se evalúa de forma síncrona.
        for i in 0..5 { j.start(PathBuf::from(format!("/no/existe/{i}.wav")), None, &s); }
        assert_eq!(j.active.len(), MAX_PARALLEL);
        assert_eq!(j.queued(), 5 - MAX_PARALLEL);
        j.start(PathBuf::from("/no/existe/4.wav"), None, &s);
        assert_eq!(j.queued(), 5 - MAX_PARALLEL, "la misma ruta no se encola dos veces");
        assert!(j.summary().unwrap().contains("(+4 más)"));
    }

    #[test]
    fn queued_reanalysis_can_be_cancelled() {
        let mut j = Jobs::default();
        let s = Settings::default();
        for i in 0..MAX_PARALLEL { j.start(PathBuf::from(format!("/no/existe/a{i}.wav")), None, &s); }
        j.start(PathBuf::from("/no/existe/z.wav"), Some(42), &s);
        assert!(j.busy_with(42));
        assert_eq!(j.progress_of(42), Some((0.0, "en cola")));
        assert!(j.cancel(42));
        assert!(!j.busy_with(42));
        assert!(!j.cancel(42));
    }
}
