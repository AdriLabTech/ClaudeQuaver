//! Generador determinista de mapas 4K a partir de un `Analysis`.
//!
//! Misma versión + mismos parámetros + misma entrada ⇒ mismo mapa (sin aleatoriedad:
//! el desempate usa un hash de la huella del audio y el instante).
//!
//! Las dificultades comparten una misma *importancia* por evento y solo cambian los
//! umbrales/reglas, de modo que cada nivel inferior es una simplificación reconocible
//! del superior (los conjuntos de eventos están anidados salvo por la poda de densidad).
//!
//! De Easy a Hard solo se usan los golpes que el analizador acepta. Los niveles superiores
//! añaden, por este orden:
//!
//! 1. **Eventos débiles sobre la cuadrícula**: golpes por debajo del umbral adaptativo (hi-hats,
//!    shakers, arpegios suaves) que caen exactamente sobre una subdivisión del tempo. Que encajen en
//!    la cuadrícula es la prueba de que son reales y no ruido.
//! 2. **Golpes fuera de cuadrícula** (swing, tempo mal estimado) que el analizador sí aceptó.
//! 3. **Acordes**: saltos de dos notas y, en lo más alto, manos de tres.
//! 4. **Relleno de cuadrícula** (Master y Extreme): corcheas o semicorcheas que puentean los huecos
//!    entre notas ya elegidas, solo donde la canción está activa. Es la única regla que no parte de
//!    un golpe medido: es lo que convierte un patrón de bombos en un *stream*.

use super::*;
use crate::analysis::{onset::{Band, Rejection}, rhythm::Subdivision, Analysis, Event};
use std::collections::BTreeMap;

pub const GENERATOR_VERSION: &str = "2";

#[derive(Debug, Clone)]
pub struct GenOptions {
    pub holds: bool,
    /// Multiplica el límite de densidad (0.5..3). Ajuste avanzado.
    pub density_scale: f32,
}
impl Default for GenOptions { fn default() -> Self { Self { holds: true, density_scale: 1.0 } } }

#[derive(Debug, Clone)]
pub struct Meta { pub title: String, pub artist: String, pub fingerprint: String }

/// Cuándo se acompaña una nota con otra simultánea.
struct Chords {
    /// Importancia mínima del evento.
    min: f32,
    subs: &'static [Subdivision],
    downbeat_only: bool,
    /// Separación mínima entre los dos carriles (1 = pueden ser contiguos).
    min_dist: i8,
}

/// Relleno de cuadrícula entre notas ya elegidas.
struct Fill {
    /// Casillas por pulso (2 = corcheas, 4 = semicorcheas).
    step: u32,
    /// Solo se puentean huecos de hasta tantos pulsos.
    max_gap_beats: f64,
    /// Solo donde ya hay al menos tantas notas por segundo (±1,5 s): no se inventan streams en intros.
    busy_nps: f32,
    /// Cada cuántas notas de relleno se añade un salto (`None` = nunca): 2 es un *jumpstream*.
    chord_every: Option<usize>,
}

struct Profile {
    min_importance: f32,
    min_gap_ms: f64,
    /// Eventos por segundo máximos en cualquier ventana de 1 s (los acordes añaden hasta un 30 %).
    max_nps: f32,
    allow: &'static [Subdivision],
    /// Golpes débiles sobre la cuadrícula: (fuerza mínima, subdivisiones admitidas); `None` = no se usan.
    weak: Option<(f32, &'static [Subdivision])>,
    chords: Option<Chords>,
    /// Importancia mínima para una mano de tres notas en un pulso fuerte.
    triple_min: Option<f32>,
    hold_min_ms: u32,
    max_hold_ratio: f32,
    fill: Option<Fill>,
}

use Subdivision::*;
const GRID: &[Subdivision] = &[Beat, Eighth, Sixteenth, Triplet];
const ANY: &[Subdivision] = &[Beat, Eighth, Sixteenth, Triplet, OffGrid];

fn profile(d: Difficulty) -> Profile {
    match d {
        Difficulty::Easy => Profile { min_importance: 0.55, min_gap_ms: 330.0, max_nps: 2.6, allow: &[Beat], weak: None,
            chords: None, triple_min: None, hold_min_ms: u32::MAX, max_hold_ratio: 0.0, fill: None },
        Difficulty::Normal => Profile { min_importance: 0.42, min_gap_ms: 210.0, max_nps: 4.6, allow: &[Beat, Eighth], weak: None,
            chords: None, triple_min: None, hold_min_ms: 600, max_hold_ratio: 0.08, fill: None },
        Difficulty::Hard => Profile { min_importance: 0.28, min_gap_ms: 120.0, max_nps: 7.5, allow: GRID, weak: Some((0.10, &[Beat, Eighth])),
            chords: Some(Chords { min: 0.80, subs: &[Beat], downbeat_only: true, min_dist: 2 }), triple_min: None,
            hold_min_ms: 450, max_hold_ratio: 0.12, fill: None },
        Difficulty::Expert => Profile { min_importance: 0.18, min_gap_ms: 80.0, max_nps: 11.0, allow: GRID, weak: Some((0.10, GRID)),
            chords: Some(Chords { min: 0.62, subs: &[Beat, Eighth], downbeat_only: false, min_dist: 2 }), triple_min: None,
            hold_min_ms: 400, max_hold_ratio: 0.15, fill: None },
        Difficulty::Master => Profile { min_importance: 0.12, min_gap_ms: 62.0, max_nps: 15.0, allow: ANY, weak: Some((0.08, GRID)),
            chords: Some(Chords { min: 0.45, subs: &[Beat, Eighth, Sixteenth], downbeat_only: false, min_dist: 2 }), triple_min: Some(0.90),
            hold_min_ms: 350, max_hold_ratio: 0.18,
            fill: Some(Fill { step: 4, max_gap_beats: 1.01, busy_nps: 2.5, chord_every: None }) },
        Difficulty::Extreme => Profile { min_importance: 0.06, min_gap_ms: 48.0, max_nps: 20.0, allow: ANY, weak: Some((0.05, GRID)),
            chords: Some(Chords { min: 0.30, subs: GRID, downbeat_only: false, min_dist: 1 }), triple_min: Some(0.75),
            hold_min_ms: 300, max_hold_ratio: 0.22,
            fill: Some(Fill { step: 4, max_gap_beats: 2.01, busy_nps: 1.5, chord_every: Some(2) }) },
    }
}

/// Importancia 0..1 de un evento: ataque fuerte, posición métrica, banda y regularidad.
pub fn importance(e: &Event) -> f32 {
    let metric = match e.subdivision {
        Subdivision::Beat => if e.downbeat { 1.0 } else { 0.85 },
        Subdivision::Eighth => 0.6, Subdivision::Triplet => 0.45, Subdivision::Sixteenth => 0.4, Subdivision::OffGrid => 0.2,
    };
    let band = match e.band { Band::Low => 1.0, Band::Mid => 0.8, Band::High => 0.5 };
    let pattern = if e.pattern.is_some() { 1.0 } else { 0.5 };
    0.45 * e.strength + 0.25 * metric + 0.15 * band + 0.15 * pattern
}

fn hash(seed: u64, x: u64) -> f32 {
    let mut h = seed ^ x.wrapping_mul(0x9E3779B97F4A7C15);
    h ^= h >> 30; h = h.wrapping_mul(0xBF58476D1CE4E5B9); h ^= h >> 27; h = h.wrapping_mul(0x94D049BB133111EB); h ^= h >> 31;
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// ¿Añadir una nota en `t` deja **cada** ventana de 1 s `[s, s+1 s)` con como mucho `limit` notas?
/// `sets` son listas ordenadas de instantes ya ocupados. Basta mirar las ventanas que empiezan en una
/// nota de `[t−1 s, t]` o en `t`: cualquier otra ventana que contenga a `t` tiene menos notas que alguna de esas.
fn windows_ok(sets: &[&[Micros]], t: Micros, limit: f32) -> bool {
    let one = Micros::from_secs(1.0);
    let mut around: Vec<Micros> = vec![];
    for set in sets {
        let (lo, hi) = (set.partition_point(|x| *x < t - one), set.partition_point(|x| *x < t + one));
        around.extend_from_slice(&set[lo..hi]);
    }
    around.sort_unstable();
    let starts = around.iter().copied().filter(|s| *s <= t).chain(std::iter::once(t));
    starts.into_iter().all(|s| {
        let n = around.iter().filter(|x| **x >= s && **x < s + one).count() + usize::from(t >= s && t < s + one);
        n as f32 <= limit
    })
}

/// Instantes ya elegidos (ordenados): comprueba hueco mínimo y densidad antes de admitir otro.
struct Selector { times: Vec<Micros>, gap: Micros, cap: f32 }

impl Selector {
    fn can_add(&self, t: Micros) -> bool {
        let pos = self.times.partition_point(|x| *x < t);
        if pos > 0 && t - self.times[pos - 1] < self.gap { return false }
        if pos < self.times.len() && self.times[pos] - t < self.gap { return false }
        windows_ok(&[&self.times], t, self.cap)
    }
    fn add(&mut self, t: Micros) {
        let pos = self.times.partition_point(|x| *x < t);
        self.times.insert(pos, t);
    }
}

struct Pick { ev: Event, fill: bool }

pub fn generate(an: &Analysis, diff: Difficulty, meta: &Meta, opts: &GenOptions) -> Beatmap {
    let prof = profile(diff);
    let seed = u64::from_str_radix(&meta.fingerprint, 16).unwrap_or(0x5eed);
    let mut repairs = vec![];
    let mut filled = 0usize;
    let cap = (prof.max_nps * opts.density_scale.clamp(0.5, 3.0)).max(1.0);
    let mut sel = Selector { times: vec![], gap: Micros::from_ms(prof.min_gap_ms), cap };
    let mut picks: Vec<Pick> = vec![];

    // 1. Selección por importancia (determinista: orden por importancia desc, luego tiempo). Primero
    //    los golpes aceptados y después los débiles, para que estos nunca desplacen a aquellos.
    let by_importance = |idx: &mut Vec<usize>| idx.sort_by(|a, b| importance(&an.events[*b]).partial_cmp(&importance(&an.events[*a])).unwrap()
        .then(an.events[*a].time.cmp(&an.events[*b].time)));
    let mut accepted: Vec<usize> = (0..an.events.len()).filter(|i| an.events[*i].accepted()).collect();
    by_importance(&mut accepted);
    for i in accepted {
        let e = &an.events[i];
        let level_adj = match e.section_level { 0 => 0.08, 2 => -0.05, _ => 0.0 };
        if importance(e) < prof.min_importance + level_adj { continue }
        if !prof.allow.contains(&e.subdivision) { continue }
        if !sel.can_add(e.time) { continue }
        sel.add(e.time);
        picks.push(Pick { ev: e.clone(), fill: false });
    }
    if let Some((min_strength, weak_subs)) = prof.weak {
        let mut weak: Vec<usize> = (0..an.events.len())
            .filter(|i| an.events[*i].rejected == Some(Rejection::BelowThreshold)
                && weak_subs.contains(&an.events[*i].subdivision) && an.events[*i].strength >= min_strength).collect();
        by_importance(&mut weak);
        for i in weak {
            let e = &an.events[i];
            if importance(e) < prof.min_importance || !prof.allow.contains(&e.subdivision) || !sel.can_add(e.time) { continue }
            sel.add(e.time);
            let mut ev = e.clone();
            ev.rejected = None; // ya es una nota del mapa
            picks.push(Pick { ev, fill: false });
        }
    }

    // 2. Relleno de cuadrícula entre notas ya elegidas. Solo si la cuadrícula explica el audio:
    //    con un tempo dudoso o muchos golpes fuera de cuadrícula, rellenar pondría notas a destiempo.
    let usable: Vec<&Event> = an.events.iter().filter(|e| e.accepted() || e.rejected == Some(Rejection::BelowThreshold)).collect();
    let on_grid = usable.iter().filter(|e| e.subdivision != Subdivision::OffGrid).count();
    let grid_ok = an.tempo.as_ref().is_some_and(|t| t.confidence >= 0.4) && !usable.is_empty() && on_grid as f32 >= 0.8 * usable.len() as f32;
    if prof.fill.is_some() && !grid_ok { repairs.push("relleno de cuadrícula desactivado: el tempo no explica el audio".into()) }
    if let (Some(fill), true) = (&prof.fill, grid_ok) {
        let before = picks.len();
        for k in 0..an.beats.len().saturating_sub(1) {
            let (b0, b1) = (an.beats[k].time, an.beats[k + 1].time);
            let beat_len = (b1 - b0).0 as f64;
            if beat_len <= 0.0 { continue }
            for j in 0..fill.step {
                let t = Micros(b0.0 + (beat_len * j as f64 / fill.step as f64).round() as i64);
                let pos = sel.times.partition_point(|x| *x < t);
                if pos == 0 || pos >= sel.times.len() { continue }
                let (p, q) = (sel.times[pos - 1], sel.times[pos]);
                if (q - p).0 as f64 > fill.max_gap_beats * beat_len { continue }
                // Solo donde la canción ya está activa (y no en silencio).
                let (a, b) = (t - Micros::from_ms(1500.0), t + Micros::from_ms(1500.0));
                let around = sel.times.partition_point(|x| *x <= b) - sel.times.partition_point(|x| *x < a);
                if (around as f32) < fill.busy_nps * 3.0 { continue }
                if an.segments.iter().any(|s| s.silent && t.as_secs() >= s.start_s && t.as_secs() < s.end_s) { continue }
                if !sel.can_add(t) { continue }
                let near = an.events.iter().rev().find(|e| e.time <= t && e.accepted()).or_else(|| an.events.iter().find(|e| e.accepted()));
                let Some(near) = near else { continue };
                sel.add(t);
                let ev = Event {
                    time: t, strength: near.strength * 0.4, band: Band::Mid, level_db: near.level_db, refined: false,
                    beat_pos: Some(k as f64 + j as f64 / fill.step as f64),
                    subdivision: if fill.step >= 4 && j % 2 == 1 { Subdivision::Sixteenth } else if j == 0 { Subdivision::Beat } else { Subdivision::Eighth },
                    grid_err_ms: 0.0, bar: None, pattern: None, section_level: near.section_level, rejected: None,
                    downbeat: false, sustain_ms: 0,
                };
                picks.push(Pick { ev, fill: true });
            }
        }
        filled = picks.len() - before;
    }
    picks.sort_by_key(|p| p.ev.time);

    // 3. Notas largas: solo con sustain medido, sin invadir el siguiente evento elegido.
    let mut holds: BTreeMap<usize, Micros> = BTreeMap::new();
    if opts.holds && prof.hold_min_ms != u32::MAX {
        let max_holds = (picks.len() as f32 * prof.max_hold_ratio) as usize;
        for (k, p) in picks.iter().enumerate() {
            if holds.len() >= max_holds { break }
            let e = &p.ev;
            if p.fill || e.sustain_ms < prof.hold_min_ms || e.strength < 0.4 || e.band == Band::High { continue }
            let next = picks.get(k + 1).map(|n| n.ev.time).unwrap_or(an.duration);
            let end = (e.time + Micros::from_ms(e.sustain_ms as f64 - 60.0)).min(next - Micros::from_ms(90.0)).min(an.duration - Micros::from_ms(20.0));
            if end - e.time >= Micros::from_ms(prof.hold_min_ms as f64 * 0.8) { holds.insert(k, end); }
        }
    }

    // 4. Carriles y acordes.
    let mut notes: Vec<Note> = vec![];
    let mut lane_free = [Micros(i64::MIN); 4]; // instante hasta el que cada carril está ocupado
    let mut last_lane: Option<u8> = None;
    let mut prev_lane: Option<u8> = None;
    // Todos los carriles usados en el evento anterior y en el previo a ese (un acorde ocupa varios).
    let mut last_set = [false; 4];
    let mut prev_set = [false; 4];
    let mut last_time = Micros(i64::MIN / 2);
    let mut learned: BTreeMap<(u32, u32), u8> = BTreeMap::new(); // (patrón, posición en compás) → carril
    let mut chord_times: Vec<Micros> = vec![];
    let mut fills_seen = 0usize;
    let mut id = 0u32;
    for (k, p) in picks.iter().enumerate() {
        let e = &p.ev;
        let end = holds.get(&k).copied();
        let dt = (e.time - last_time).as_ms();
        let slot_key = e.pattern.zip(e.beat_pos).map(|(pt, bp)| (pt, ((bp.rem_euclid(4.0)) * 12.0).round() as u32));
        let learned_lane = slot_key.and_then(|key| learned.get(&key)).copied();
        let pick = |lane_ok: &dyn Fn(u8) -> bool| -> Option<u8> {
            // Reutilizar el carril aprendido en el mismo patrón/posición → repetición musical.
            if let Some(l) = learned_lane {
                if lane_ok(l) && !(last_set[l as usize] && dt < 150.0) { return Some(l) }
            }
            (0..4u8).filter(|l| lane_ok(*l)).map(|l| {
                let mut cost = 0.0f32;
                if last_set[l as usize] { cost += if dt < 140.0 { 9.0 } else if dt < 260.0 { 2.5 } else { 0.8 } }
                if prev_set[l as usize] { cost += 0.8 }
                if let Some(ll) = last_lane {
                    if (ll / 2 != l / 2) && dt < 300.0 { cost -= 0.5 } // alternar manos
                    // En pasajes rápidos se prefieren escaleras y trinos: carriles contiguos y misma dirección.
                    if dt < 220.0 && (l as i8 - ll as i8).abs() == 1 { cost -= 0.6 }
                    if let Some(pl) = prev_lane {
                        if dt < 220.0 && l != ll && (l as i8 - ll as i8) == (ll as i8 - pl as i8) { cost -= 0.4 }
                    }
                }
                cost += match (e.band, l) { (Band::Low, 1 | 2) => -0.3, (Band::High, 0 | 3) => -0.3, _ => 0.0 };
                cost += hash(seed, e.time.0 as u64 ^ (l as u64) << 56) * 0.4;
                (l, cost)
            }).min_by(|a, b| a.1.partial_cmp(&b.1).unwrap().then(a.0.cmp(&b.0))).map(|x| x.0)
        };
        let Some(lane) = pick(&|l| lane_free[l as usize] < e.time) else {
            repairs.push(format!("evento a {} ms descartado: sin carril libre", e.time.as_ms() as i64)); continue
        };
        lane_free[lane as usize] = end.unwrap_or(e.time) + Micros::from_ms(60.0);
        let mut used = [false; 4];
        used[lane as usize] = true;
        if let Some(key) = slot_key { learned.entry(key).or_insert(lane); }
        notes.push(Note { id, time: e.time, lane,
            kind: end.map_or(NoteKind::Tap, |end| NoteKind::Hold { end }), confidence: e.strength.max(if p.fill { 0.2 } else { 0.0 }), pattern: e.pattern, manual: false });
        id += 1;

        // Acordes: nunca sobre notas largas. El presupuesto de densidad incluye los acordes recientes.
        if end.is_none() {
            // Acordes: los eventos más los acordes ya puestos no pueden pasar de cap × 1,3 en ninguna ventana de 1 s.
            let budget_ok = || windows_ok(&[&sel.times, &chord_times], e.time, cap * 1.3);
            let wants_chord = if p.fill {
                fills_seen += 1;
                prof.fill.as_ref().and_then(|f| f.chord_every).is_some_and(|n| fills_seen % n == 0)
            } else {
                prof.chords.as_ref().is_some_and(|c| importance(e) >= c.min && c.subs.contains(&e.subdivision) && (!c.downbeat_only || e.downbeat))
            };
            if wants_chord && budget_ok() {
                let min_dist = prof.chords.as_ref().map_or(2, |c| c.min_dist);
                let first = lane;
                if let Some(l2) = pick(&|l| l != first && lane_free[l as usize] < e.time && (l as i8 - first as i8).abs() >= min_dist) {
                    lane_free[l2 as usize] = e.time + Micros::from_ms(60.0);
                    notes.push(Note { id, time: e.time, lane: l2, kind: NoteKind::Tap, confidence: e.strength, pattern: e.pattern, manual: false });
                    id += 1;
                    used[l2 as usize] = true;
                    chord_times.push(e.time);
                    // Mano de tres: solo en pulsos fuertes y muy importantes.
                    let triple = prof.triple_min.is_some_and(|m| !p.fill && importance(e) >= m && e.downbeat);
                    if triple {
                        if let Some(l3) = pick(&|l| l != first && l != l2 && lane_free[l as usize] < e.time) {
                            lane_free[l3 as usize] = e.time + Micros::from_ms(60.0);
                            notes.push(Note { id, time: e.time, lane: l3, kind: NoteKind::Tap, confidence: e.strength, pattern: e.pattern, manual: false });
                            id += 1;
                            used[l3 as usize] = true;
                        }
                    }
                }
            }
        }
        prev_lane = last_lane; last_lane = Some(lane); last_time = e.time;
        prev_set = last_set; last_set = used;
    }

    // 5. Validación y reparación determinista.
    notes.retain(|n| {
        let ok = n.time >= Micros::ZERO && n.end() <= an.duration && n.lane < LANES;
        if !ok { repairs.push(format!("nota {} fuera de rango eliminada", n.id)) }
        ok
    });
    notes.sort_by_key(|n| (n.time, n.lane));

    let (bpm, conf) = an.tempo.as_ref().map_or((None, 0.0), |t| (Some(t.bpm), t.confidence));
    Beatmap {
        format_version: FORMAT_VERSION,
        generator_version: format!("{}+{}", GENERATOR_VERSION, crate::analysis::ANALYZER_VERSION),
        title: meta.title.clone(), artist: meta.artist.clone(), audio_fingerprint: meta.fingerprint.clone(),
        duration: an.duration,
        music: MusicInfo { bpm, bpm_confidence: conf,
            bpm_candidates: an.tempo.as_ref().map(|t| t.candidates.clone()).unwrap_or_default(),
            first_beat: an.beats.first().map(|b| b.time) },
        difficulty: diff,
        parameters: format!("holds={} density_scale={:.2}{}", opts.holds, opts.density_scale,
            if filled > 0 { format!(" fill={filled}") } else { String::new() }),
        notes, song_offset_ms: 0.0, edited_manually: false, repairs,
    }
}

pub fn generate_all(an: &Analysis, meta: &Meta, opts: &GenOptions) -> Vec<Beatmap> {
    Difficulty::ALL.iter().map(|d| generate(an, *d, meta, opts)).collect()
}
