//! Edición manual de mapas: operaciones validadas, deshacer/rehacer y cuadrícula musical.
//!
//! Toda operación deja el mapa en un estado que supera `Beatmap::validate`: si una edición
//! lo rompería (solape en un carril, fuera de la canción, hold demasiado corto), se rechaza
//! con un [`EditError`] y el mapa queda intacto. La interfaz nunca necesita reparar nada.

use crate::beatmap::{Beatmap, Note, NoteKind, LANES};
use crate::Micros;
use thiserror::Error;

/// Separación mínima entre el final de una nota y la siguiente en el mismo carril.
pub const MIN_LANE_GAP: Micros = Micros(30_000);
/// Duración mínima de una nota larga (por debajo es un tap).
pub const MIN_HOLD: Micros = Micros(120_000);
/// Pasos de deshacer que se conservan.
const UNDO_LIMIT: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum EditError {
    #[error("fuera de la canción o del campo")]
    OutOfRange,
    #[error("se solapa con otra nota del mismo carril")]
    Overlap,
    #[error("la nota larga es demasiado corta")]
    TooShort,
    #[error("la nota ya no existe")]
    NotFound,
}

/// Estado restaurable por deshacer/rehacer.
#[derive(Debug, Clone)]
struct Snapshot { notes: Vec<Note>, song_offset_ms: f64, edited: bool, next_id: u32 }

#[derive(Debug)]
pub struct Editor {
    map: Beatmap,
    next_id: u32,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// Sube con cada cambio; `saved_rev` es la revisión que está en disco.
    rev: u64,
    saved_rev: u64,
}

impl Editor {
    /// Abre un mapa para editar. Falla si el mapa ya es inválido.
    pub fn new(mut map: Beatmap) -> crate::Result<Editor> {
        map.validate()?;
        map.notes.sort_by_key(|n| (n.time, n.lane));
        let next_id = map.notes.iter().map(|n| n.id + 1).max().unwrap_or(0);
        Ok(Editor { map, next_id, undo: vec![], redo: vec![], rev: 0, saved_rev: 0 })
    }

    pub fn map(&self) -> &Beatmap { &self.map }
    pub fn notes(&self) -> &[Note] { &self.map.notes }
    pub fn into_map(self) -> Beatmap { self.map }
    pub fn is_dirty(&self) -> bool { self.rev != self.saved_rev }
    pub fn mark_saved(&mut self) { self.saved_rev = self.rev }
    pub fn can_undo(&self) -> bool { !self.undo.is_empty() }
    pub fn can_redo(&self) -> bool { !self.redo.is_empty() }
    pub fn note(&self, id: u32) -> Option<&Note> { self.map.notes.iter().find(|n| n.id == id) }

    /// Nota que ocupa `time` en `lane`: un tap a menos de `tol` o un hold cuyo rango lo contiene.
    pub fn note_at(&self, lane: u8, time: Micros, tol: Micros) -> Option<u32> {
        self.map.notes.iter().filter(|n| n.lane == lane)
            .filter(|n| match n.kind {
                NoteKind::Tap => (n.time - time).abs() <= tol,
                NoteKind::Hold { end } => time >= n.time - tol && time <= end + tol,
            })
            .min_by_key(|n| (n.time - time).abs()).map(|n| n.id)
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot { notes: self.map.notes.clone(), song_offset_ms: self.map.song_offset_ms, edited: self.map.edited_manually, next_id: self.next_id }
    }
    fn restore(&mut self, s: Snapshot) {
        self.map.notes = s.notes;
        self.map.song_offset_ms = s.song_offset_ms;
        self.map.edited_manually = s.edited;
        self.next_id = s.next_id;
    }
    /// Registra el estado previo y marca el mapa como editado a mano.
    fn commit(&mut self, before: Snapshot) {
        self.undo.push(before);
        if self.undo.len() > UNDO_LIMIT { self.undo.remove(0); }
        self.redo.clear();
        self.map.edited_manually = true;
        self.map.notes.sort_by_key(|n| (n.time, n.lane));
        self.rev += 1;
    }

    /// ¿Cabe `[start, end]` en `lane` ignorando la nota `ignore`?
    fn check_free(&self, lane: u8, start: Micros, end: Micros, ignore: Option<u32>) -> Result<(), EditError> {
        if lane >= LANES || start < Micros::ZERO || end > self.map.duration || end < start { return Err(EditError::OutOfRange) }
        for n in self.map.notes.iter().filter(|n| n.lane == lane && Some(n.id) != ignore) {
            if start < n.end() + MIN_LANE_GAP && n.time < end + MIN_LANE_GAP { return Err(EditError::Overlap) }
        }
        Ok(())
    }

    fn insert(&mut self, time: Micros, lane: u8, kind: NoteKind) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.map.notes.push(Note { id, time, lane, kind, confidence: 1.0, pattern: None, manual: true });
        id
    }

    pub fn add_tap(&mut self, time: Micros, lane: u8) -> Result<u32, EditError> {
        self.check_free(lane, time, time, None)?;
        let before = self.snapshot();
        let id = self.insert(time, lane, NoteKind::Tap);
        self.commit(before);
        Ok(id)
    }

    /// Añade una nota larga; `a` y `b` pueden darse en cualquier orden.
    pub fn add_hold(&mut self, a: Micros, b: Micros, lane: u8) -> Result<u32, EditError> {
        let (start, end) = (a.min(b), a.max(b));
        if end - start < MIN_HOLD { return Err(EditError::TooShort) }
        self.check_free(lane, start, end, None)?;
        let before = self.snapshot();
        let id = self.insert(start, lane, NoteKind::Hold { end });
        self.commit(before);
        Ok(id)
    }

    pub fn remove(&mut self, id: u32) -> Result<(), EditError> {
        if self.note(id).is_none() { return Err(EditError::NotFound) }
        let before = self.snapshot();
        self.map.notes.retain(|n| n.id != id);
        self.commit(before);
        Ok(())
    }

    /// Mueve una nota (un hold conserva su duración).
    pub fn move_note(&mut self, id: u32, time: Micros, lane: u8) -> Result<(), EditError> {
        let n = self.note(id).ok_or(EditError::NotFound)?.clone();
        let len = n.end() - n.time;
        self.check_free(lane, time, time + len, Some(id))?;
        let before = self.snapshot();
        let m = self.map.notes.iter_mut().find(|m| m.id == id).unwrap();
        m.time = time;
        m.lane = lane;
        if let NoteKind::Hold { end } = &mut m.kind { *end = time + len }
        m.manual = true;
        self.commit(before);
        Ok(())
    }

    /// Cambia el final de un hold. Si queda más corto que `MIN_HOLD` se convierte en tap.
    pub fn set_hold_end(&mut self, id: u32, end: Micros) -> Result<(), EditError> {
        let n = self.note(id).ok_or(EditError::NotFound)?.clone();
        if end - n.time < MIN_HOLD {
            return self.make_tap(id);
        }
        self.check_free(n.lane, n.time, end, Some(id))?;
        let before = self.snapshot();
        let m = self.map.notes.iter_mut().find(|m| m.id == id).unwrap();
        m.kind = NoteKind::Hold { end };
        m.manual = true;
        self.commit(before);
        Ok(())
    }

    pub fn make_tap(&mut self, id: u32) -> Result<(), EditError> {
        let n = self.note(id).ok_or(EditError::NotFound)?;
        if !n.is_hold() { return Ok(()) }
        let before = self.snapshot();
        let m = self.map.notes.iter_mut().find(|m| m.id == id).unwrap();
        m.kind = NoteKind::Tap;
        m.manual = true;
        self.commit(before);
        Ok(())
    }

    /// Desplaza todas las notas respecto a la música sin tocarlas (`song_offset_ms`).
    pub fn set_song_offset(&mut self, ms: f64) {
        let ms = ms.clamp(-500.0, 500.0);
        if (ms - self.map.song_offset_ms).abs() < 1e-9 { return }
        let before = self.snapshot();
        self.map.song_offset_ms = ms;
        self.commit(before);
    }

    pub fn undo(&mut self) -> bool {
        let Some(s) = self.undo.pop() else { return false };
        let now = self.snapshot();
        self.redo.push(now);
        self.restore(s);
        self.rev += 1;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(s) = self.redo.pop() else { return false };
        let now = self.snapshot();
        self.undo.push(now);
        self.restore(s);
        self.rev += 1;
        true
    }
}

// --- Cuadrícula musical ------------------------------------------------------------------------

/// Subdivisiones por pulso que ofrece el editor (1 = negras, 2 = corcheas, 3 = tresillos, 4 = semicorcheas…).
pub const DIVISIONS: [u32; 6] = [1, 2, 3, 4, 6, 8];

pub fn division_label(d: u32) -> String { format!("1/{}", 4 * d) }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind { Bar, Beat, Sub }

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridLine { pub time: Micros, pub kind: LineKind }

/// Cuadrícula de tempo constante: línea `k` en `origin + k · beat / divisions`. Compases de 4 pulsos.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    pub origin: Micros,
    /// Microsegundos por pulso.
    pub beat_us: f64,
    pub divisions: u32,
}

impl Grid {
    /// `None` si el mapa no tiene BPM fiable: el editor cae entonces a ajuste libre.
    pub fn for_map(map: &Beatmap, divisions: u32) -> Option<Grid> {
        let bpm = map.music.bpm.filter(|b| b.is_finite() && *b >= 30.0 && *b <= 400.0)?;
        Some(Grid { origin: map.music.first_beat.unwrap_or(Micros::ZERO), beat_us: 60_000_000.0 / bpm, divisions: divisions.max(1) })
    }

    fn line_us(&self) -> f64 { self.beat_us / self.divisions as f64 }
    fn index(&self, t: Micros) -> i64 { ((t.0 - self.origin.0) as f64 / self.line_us()).round() as i64 }
    fn at(&self, k: i64) -> Micros { Micros(self.origin.0 + (k as f64 * self.line_us()).round() as i64) }

    /// Línea más cercana a `t`.
    pub fn snap(&self, t: Micros) -> Micros { self.at(self.index(t)) }

    /// Siguiente (`dir > 0`) o anterior (`dir < 0`) línea estrictamente distinta de `t`.
    pub fn step(&self, t: Micros, dir: i32) -> Micros {
        let k = self.index(t);
        let cur = self.at(k);
        let k = if dir > 0 { if cur > t { k } else { k + 1 } } else if cur < t { k } else { k - 1 };
        self.at(k)
    }

    /// Líneas en `[from, to]`, clasificadas por importancia.
    pub fn lines(&self, from: Micros, to: Micros) -> Vec<GridLine> {
        if to < from { return vec![] }
        let (a, b) = (self.index(from) - 1, self.index(to) + 1);
        let d = self.divisions as i64;
        (a..=b).map(|k| {
            let kind = if k.rem_euclid(4 * d) == 0 { LineKind::Bar } else if k.rem_euclid(d) == 0 { LineKind::Beat } else { LineKind::Sub };
            GridLine { time: self.at(k), kind }
        }).filter(|l| l.time >= from && l.time <= to).collect()
    }
}

/// Ajuste libre (sin BPM): redondea a 10 ms.
pub fn snap_free(t: Micros) -> Micros { Micros((t.0 as f64 / 10_000.0).round() as i64 * 10_000) }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::beatmap::{Difficulty, MusicInfo, FORMAT_VERSION};

    fn ms(v: f64) -> Micros { Micros::from_ms(v) }

    fn base() -> Beatmap {
        Beatmap {
            format_version: FORMAT_VERSION, generator_version: "t".into(), title: "t".into(), artist: "".into(), audio_fingerprint: "ab".into(),
            duration: Micros::from_secs(20.0),
            music: MusicInfo { bpm: Some(120.0), bpm_confidence: 1.0, bpm_candidates: vec![], first_beat: Some(ms(500.0)) },
            difficulty: Difficulty::Normal, parameters: String::new(), song_offset_ms: 0.0, edited_manually: false, repairs: vec![],
            notes: vec![Note { id: 7, time: ms(1000.0), lane: 0, kind: NoteKind::Tap, confidence: 0.5, pattern: None, manual: false }],
        }
    }

    #[test]
    fn rejects_invalid_maps_and_continues_ids() {
        let mut m = base();
        m.notes[0].lane = 9;
        assert!(Editor::new(m).is_err());
        let mut e = Editor::new(base()).unwrap();
        assert_eq!(e.add_tap(ms(2000.0), 1).unwrap(), 8, "los ids no se reutilizan");
    }

    #[test]
    fn add_remove_and_flags() {
        let mut e = Editor::new(base()).unwrap();
        assert!(!e.is_dirty() && !e.map().edited_manually);
        let id = e.add_tap(ms(1500.0), 2).unwrap();
        assert!(e.is_dirty() && e.map().edited_manually);
        assert!(e.note(id).unwrap().manual);
        e.map().validate().unwrap();
        e.remove(id).unwrap();
        assert_eq!(e.remove(id), Err(EditError::NotFound));
        assert_eq!(e.notes().len(), 1);
    }

    #[test]
    fn overlap_and_range_are_rejected_without_changes() {
        let mut e = Editor::new(base()).unwrap();
        assert_eq!(e.add_tap(ms(1010.0), 0), Err(EditError::Overlap));
        assert_eq!(e.add_tap(ms(1010.0), 1).is_ok(), true, "otro carril sí");
        assert_eq!(e.add_tap(Micros(-1), 3), Err(EditError::OutOfRange));
        assert_eq!(e.add_tap(Micros::from_secs(21.0), 3), Err(EditError::OutOfRange));
        assert_eq!(e.add_tap(ms(3000.0), 4), Err(EditError::OutOfRange));
        let n = e.notes().len();
        assert_eq!(e.add_hold(ms(900.0), ms(1400.0), 0), Err(EditError::Overlap), "el hold pisaría la nota existente");
        assert_eq!(e.notes().len(), n);
        e.map().validate().unwrap();
    }

    #[test]
    fn holds_need_length_and_accept_reversed_arguments() {
        let mut e = Editor::new(base()).unwrap();
        assert_eq!(e.add_hold(ms(3000.0), ms(3050.0), 2), Err(EditError::TooShort));
        let id = e.add_hold(ms(4000.0), ms(3000.0), 2).unwrap();
        assert_eq!(e.note(id).unwrap().time, ms(3000.0));
        assert_eq!(e.note(id).unwrap().end(), ms(4000.0));
        // acortar por debajo del mínimo lo convierte en tap
        e.set_hold_end(id, ms(3050.0)).unwrap();
        assert!(!e.note(id).unwrap().is_hold());
        e.map().validate().unwrap();
    }

    #[test]
    fn move_keeps_hold_length_and_checks_collisions() {
        let mut e = Editor::new(base()).unwrap();
        let h = e.add_hold(ms(3000.0), ms(4000.0), 1).unwrap();
        e.move_note(h, ms(6000.0), 3).unwrap();
        let n = e.note(h).unwrap();
        assert_eq!((n.time, n.end(), n.lane), (ms(6000.0), ms(7000.0), 3));
        assert_eq!(e.move_note(h, ms(1000.0), 0), Err(EditError::Overlap));
        // moverse sobre sí misma (mismo carril, poco desplazamiento) no cuenta como solape
        e.move_note(h, ms(6100.0), 3).unwrap();
        e.map().validate().unwrap();
    }

    #[test]
    fn undo_redo_restores_everything_including_offset() {
        let mut e = Editor::new(base()).unwrap();
        let original = e.notes().to_vec();
        let id = e.add_tap(ms(2000.0), 1).unwrap();
        e.set_song_offset(25.0);
        e.remove(7).unwrap();
        assert!(e.can_undo() && !e.can_redo());
        assert!(e.undo() && e.undo() && e.undo());
        assert!(!e.undo(), "pila vacía");
        assert_eq!(e.notes(), &original[..]);
        assert_eq!(e.map().song_offset_ms, 0.0);
        assert!(!e.map().edited_manually, "deshacer todo vuelve al estado original");
        assert!(e.redo() && e.redo());
        assert_eq!(e.map().song_offset_ms, 25.0);
        assert!(e.note(id).is_some() && e.note(7).is_some());
        // una edición nueva invalida el rehacer
        e.add_tap(ms(5000.0), 0).unwrap();
        assert!(!e.can_redo());
        e.map().validate().unwrap();
    }

    #[test]
    fn ids_survive_undo_without_reuse() {
        let mut e = Editor::new(base()).unwrap();
        let a = e.add_tap(ms(2000.0), 1).unwrap();
        e.undo();
        let b = e.add_tap(ms(2500.0), 1).unwrap();
        assert_eq!(a, b, "tras deshacer, el id vuelve a estar libre");
        e.remove(b).unwrap();
        let c = e.add_tap(ms(2600.0), 1).unwrap();
        assert!(c > b, "tras borrar, el id no se reutiliza");
    }

    #[test]
    fn dirty_tracks_saved_revision() {
        let mut e = Editor::new(base()).unwrap();
        e.add_tap(ms(2000.0), 1).unwrap();
        assert!(e.is_dirty());
        e.mark_saved();
        assert!(!e.is_dirty());
        e.undo();
        assert!(e.is_dirty(), "deshacer tras guardar vuelve a ensuciar");
    }

    #[test]
    fn note_at_finds_taps_and_holds() {
        let mut e = Editor::new(base()).unwrap();
        let h = e.add_hold(ms(3000.0), ms(4000.0), 2).unwrap();
        assert_eq!(e.note_at(0, ms(1030.0), ms(40.0)), Some(7));
        assert_eq!(e.note_at(0, ms(1100.0), ms(40.0)), None);
        assert_eq!(e.note_at(2, ms(3500.0), ms(10.0)), Some(h), "interior de un hold");
        assert_eq!(e.note_at(1, ms(3500.0), ms(10.0)), None);
    }

    #[test]
    fn undo_limit_is_bounded() {
        let mut e = Editor::new(base()).unwrap();
        for i in 0..(UNDO_LIMIT + 50) { e.set_song_offset(i as f64 % 400.0 + 1.0); }
        assert!(e.undo.len() <= UNDO_LIMIT);
    }

    #[test]
    fn grid_snap_step_and_lines() {
        let m = base();
        let g = Grid::for_map(&m, 2).unwrap(); // 120 BPM → corchea = 250 ms, origen 500 ms
        assert_eq!(g.snap(ms(760.0)), ms(750.0));
        assert_eq!(g.snap(ms(600.0)), ms(500.0));
        assert_eq!(g.step(ms(500.0), 1), ms(750.0));
        assert_eq!(g.step(ms(500.0), -1), ms(250.0));
        assert_eq!(g.step(ms(600.0), 1), ms(750.0), "desde fuera de la cuadrícula salta a la siguiente");
        assert_eq!(g.step(ms(600.0), -1), ms(500.0));
        let lines = g.lines(ms(500.0), ms(3000.0));
        assert_eq!(lines.len(), 11);
        assert_eq!(lines[0], GridLine { time: ms(500.0), kind: LineKind::Bar });
        assert_eq!(lines[1].kind, LineKind::Sub);
        assert_eq!(lines[2].kind, LineKind::Beat);
        assert_eq!(lines[8].kind, LineKind::Bar, "cada 4 pulsos (8 corcheas)");
        assert!(lines.windows(2).all(|w| w[0].time < w[1].time));
    }

    #[test]
    fn triplets_and_missing_bpm() {
        let g = Grid::for_map(&base(), 3).unwrap();
        assert_eq!(g.snap(ms(667.0)), Micros(666_667));
        let mut m = base();
        m.music.bpm = None;
        assert!(Grid::for_map(&m, 1).is_none());
        m.music.bpm = Some(f64::NAN);
        assert!(Grid::for_map(&m, 1).is_none());
        assert_eq!(snap_free(ms(1234.0)), ms(1230.0));
        assert_eq!(division_label(4), "1/16");
    }
}
