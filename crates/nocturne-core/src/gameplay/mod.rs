//! Motor de juicio determinista. Solo conoce instantes (`Micros`) de la línea
//! temporal de la canción; el reloj, el offset y la pausa viven fuera.

use crate::beatmap::{Beatmap, Note, NoteKind};
use crate::scoring::{HitWindows, Judgement, ScoreState};
use crate::Micros;

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Juicio de una nota (cabeza de hold incluida).
    Judged { note: u32, lane: u8, judgement: Judgement, error_ms: Option<f64> },
    /// Juicio del final de una nota larga.
    HoldEnd { note: u32, lane: u8, judgement: Judgement },
    /// Pulsación que no corresponde a ninguna nota (no puntúa).
    GhostTap { lane: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State { Pending, Holding, Done }

#[derive(Debug)]
pub struct Judge {
    notes: Vec<Note>,
    state: Vec<State>,
    /// Por carril: índices en `notes` ordenados por tiempo.
    lanes: [Vec<usize>; 4],
    cursor: [usize; 4],
    windows: HitWindows,
    pub score: ScoreState,
}

impl Judge {
    pub fn new(map: &Beatmap, windows: HitWindows) -> Self {
        let mut notes = map.notes.clone();
        notes.sort_by_key(|n| (n.time, n.lane));
        let mut lanes: [Vec<usize>; 4] = Default::default();
        for (i, n) in notes.iter().enumerate() { lanes[n.lane as usize].push(i) }
        let total = map.total_judgements();
        let state = vec![State::Pending; notes.len()];
        Self { notes, state, lanes, cursor: [0; 4], windows, score: ScoreState::new(total) }
    }

    pub fn notes(&self) -> &[Note] { &self.notes }
    pub fn is_finished(&self) -> bool { self.state.iter().all(|s| *s == State::Done) }
    pub fn is_pending(&self, idx: usize) -> bool { self.state[idx] == State::Pending }
    pub fn is_holding(&self, idx: usize) -> bool { self.state[idx] == State::Holding }

    fn release_window(&self) -> Micros { Micros::from_ms(self.windows.good_ms) }

    fn head(&mut self, lane: usize) -> Option<usize> {
        while let Some(&i) = self.lanes[lane].get(self.cursor[lane]) {
            if self.state[i] == State::Done { self.cursor[lane] += 1 } else { return Some(i) }
        }
        None
    }

    /// Procesa una pulsación en el instante `t` (ya compensado).
    pub fn press(&mut self, lane: u8, t: Micros) -> Vec<Event> {
        let l = lane as usize;
        let mut out = vec![];
        let Some(i) = self.head(l) else { return vec![Event::GhostTap { lane }] };
        if self.state[i] != State::Pending { return vec![Event::GhostTap { lane }] }
        let signed = (t - self.notes[i].time).as_ms();
        match self.windows.classify(signed.abs()) {
            None => out.push(Event::GhostTap { lane }),
            Some(j) => {
                let n = &self.notes[i];
                self.score.register(j, Some(signed));
                out.push(Event::Judged { note: n.id, lane, judgement: j, error_ms: Some(signed) });
                self.state[i] = if n.is_hold() { State::Holding } else { State::Done };
            }
        }
        out
    }

    /// Procesa la liberación de una tecla en `t`.
    pub fn release(&mut self, lane: u8, t: Micros) -> Vec<Event> {
        let l = lane as usize;
        let Some(i) = self.head(l) else { return vec![] };
        if self.state[i] != State::Holding { return vec![] }
        let end = self.notes[i].end();
        let early = (end - t).as_ms();
        let j = if early > self.windows.good_ms {
            Judgement::Miss // soltó demasiado pronto
        } else {
            // Tolerancia de liberación doble: soltar es menos preciso que pulsar.
            self.windows.classify((t - end).as_ms().abs() / 2.0).unwrap_or(Judgement::Good)
        };
        self.finish_hold(i, j)
    }

    fn finish_hold(&mut self, i: usize, j: Judgement) -> Vec<Event> {
        let n = &self.notes[i];
        self.score.register(j, None);
        self.state[i] = State::Done;
        vec![Event::HoldEnd { note: n.id, lane: n.lane, judgement: j }]
    }

    /// Avanza el tiempo: genera Misses y cierra holds mantenidos hasta el final.
    pub fn advance(&mut self, t: Micros) -> Vec<Event> {
        let mut out = vec![];
        let miss_after = self.windows.miss_after();
        let rel = self.release_window();
        for l in 0..4 {
            while let Some(i) = self.head(l) {
                let n = self.notes[i].clone();
                match self.state[i] {
                    State::Pending if t > n.time + miss_after => {
                        self.score.register(Judgement::Miss, None);
                        out.push(Event::Judged { note: n.id, lane: n.lane, judgement: Judgement::Miss, error_ms: None });
                        if let NoteKind::Hold { .. } = n.kind {
                            self.score.register(Judgement::Miss, None);
                            out.push(Event::HoldEnd { note: n.id, lane: n.lane, judgement: Judgement::Miss });
                        }
                        self.state[i] = State::Done;
                    }
                    State::Holding if t > n.end() + rel => out.extend(self.finish_hold(i, Judgement::Perfect)),
                    _ => break,
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests;
