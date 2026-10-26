//! Generador determinista de mapas 4K a partir de un `Analysis`.
//!
//! Misma versión + mismos parámetros + misma entrada ⇒ mismo mapa (sin aleatoriedad:
//! el desempate usa un hash de la huella del audio y el instante).
//!
//! Las dificultades comparten una misma *importancia* por evento y solo cambian los
//! umbrales/reglas, de modo que cada nivel inferior es una simplificación reconocible
//! del superior (los conjuntos de eventos están anidados salvo por la poda de densidad).

use super::*;
use crate::analysis::{rhythm::Subdivision, onset::Band, Analysis, Event};
use std::collections::BTreeMap;

pub const GENERATOR_VERSION: &str = "1";

#[derive(Debug, Clone)]
pub struct GenOptions {
    pub holds: bool,
    /// Multiplica el límite de densidad (0.5..2). Ajuste avanzado.
    pub density_scale: f32,
}
impl Default for GenOptions { fn default() -> Self { Self { holds: true, density_scale: 1.0 } } }

#[derive(Debug, Clone)]
pub struct Meta { pub title: String, pub artist: String, pub fingerprint: String }

struct Profile {
    min_importance: f32,
    min_gap_ms: f64,
    /// Notas por segundo máximas en cualquier ventana de 1 s.
    max_nps: f32,
    allow: &'static [Subdivision],
    /// Importancia mínima para acompañar con una segunda nota (acorde); `None` = sin acordes.
    chord_min: Option<f32>,
    hold_min_ms: u32,
    max_hold_ratio: f32,
}

fn profile(d: Difficulty) -> Profile {
    use Subdivision::*;
    match d {
        Difficulty::Easy => Profile { min_importance: 0.55, min_gap_ms: 330.0, max_nps: 2.6, allow: &[Beat],
            chord_min: None, hold_min_ms: u32::MAX, max_hold_ratio: 0.0 },
        Difficulty::Normal => Profile { min_importance: 0.42, min_gap_ms: 210.0, max_nps: 4.6, allow: &[Beat, Eighth],
            chord_min: None, hold_min_ms: 600, max_hold_ratio: 0.08 },
        Difficulty::Hard => Profile { min_importance: 0.30, min_gap_ms: 130.0, max_nps: 7.0, allow: &[Beat, Eighth, Sixteenth, Triplet],
            chord_min: Some(0.85), hold_min_ms: 450, max_hold_ratio: 0.12 },
        Difficulty::Expert => Profile { min_importance: 0.20, min_gap_ms: 85.0, max_nps: 10.5, allow: &[Beat, Eighth, Sixteenth, Triplet],
            chord_min: Some(0.70), hold_min_ms: 400, max_hold_ratio: 0.15 },
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

pub fn generate(an: &Analysis, diff: Difficulty, meta: &Meta, opts: &GenOptions) -> Beatmap {
    let prof = profile(diff);
    let seed = u64::from_str_radix(&meta.fingerprint, 16).unwrap_or(0x5eed);
    let mut repairs = vec![];

    // 1. Selección por importancia (determinista: orden por importancia desc, luego tiempo).
    let mut order: Vec<usize> = (0..an.events.len()).filter(|i| an.events[*i].accepted()).collect();
    order.sort_by(|a, b| importance(&an.events[*b]).partial_cmp(&importance(&an.events[*a])).unwrap()
        .then(an.events[*a].time.cmp(&an.events[*b].time)));
    let gap = Micros::from_ms(prof.min_gap_ms);
    let cap = (prof.max_nps * opts.density_scale.clamp(0.5, 2.0)).max(1.0);
    let mut chosen: Vec<usize> = vec![];
    let mut times: Vec<Micros> = vec![]; // ordenado
    for i in order {
        let e = &an.events[i];
        let level_adj = match e.section_level { 0 => 0.08, 2 => -0.05, _ => 0.0 };
        if importance(e) < prof.min_importance + level_adj { continue }
        if !prof.allow.contains(&e.subdivision) { continue }
        let pos = times.partition_point(|t| *t < e.time);
        if pos > 0 && e.time - times[pos - 1] < gap { continue }
        if pos < times.len() && times[pos] - e.time < gap { continue }
        // Densidad: notas en la ventana de 1 s centrada en el evento.
        let (a, b) = (e.time - Micros::from_ms(500.0), e.time + Micros::from_ms(500.0));
        let n = times.partition_point(|t| *t <= b) - times.partition_point(|t| *t < a);
        if n as f32 + 1.0 > cap { continue }
        times.insert(pos, e.time);
        chosen.push(i);
    }
    chosen.sort_by_key(|i| an.events[*i].time);

    // 2. Notas largas: solo con sustain medido, sin invadir el siguiente evento elegido.
    let mut holds: BTreeMap<usize, Micros> = BTreeMap::new();
    if opts.holds && prof.hold_min_ms != u32::MAX {
        let max_holds = (chosen.len() as f32 * prof.max_hold_ratio) as usize;
        for (k, &i) in chosen.iter().enumerate() {
            if holds.len() >= max_holds { break }
            let e = &an.events[i];
            if e.sustain_ms < prof.hold_min_ms || e.strength < 0.4 || e.band == Band::High { continue }
            let next = chosen.get(k + 1).map(|j| an.events[*j].time).unwrap_or(an.duration);
            let end = (e.time + Micros::from_ms(e.sustain_ms as f64 - 60.0)).min(next - Micros::from_ms(90.0)).min(an.duration - Micros::from_ms(20.0));
            if end - e.time >= Micros::from_ms(prof.hold_min_ms as f64 * 0.8) { holds.insert(i, end); }
        }
    }

    // 3. Carriles.
    let mut notes: Vec<Note> = vec![];
    let mut lane_free = [Micros(i64::MIN); 4]; // instante hasta el que cada carril está ocupado
    let mut last_lane: Option<u8> = None;
    let mut prev_lane: Option<u8> = None;
    let mut last_time = Micros(i64::MIN / 2);
    let mut learned: BTreeMap<(u32, u32), u8> = BTreeMap::new(); // (patrón, posición en compás) → carril
    let mut id = 0u32;
    for &i in &chosen {
        let e = &an.events[i];
        let end = holds.get(&i).copied();
        let dt = (e.time - last_time).as_ms();
        let slot_key = e.pattern.zip(e.beat_pos).map(|(p, bp)| (p, ((bp.rem_euclid(4.0)) * 12.0).round() as u32));
        let learned_lane = slot_key.and_then(|k| learned.get(&k)).copied();
        let pick = |lane_ok: &dyn Fn(u8) -> bool| -> Option<u8> {
            // Reutilizar el carril aprendido en el mismo patrón/posición → repetición musical.
            if let Some(l) = learned_lane {
                if lane_ok(l) && !(Some(l) == last_lane && dt < 150.0) { return Some(l) }
            }
            (0..4u8).filter(|l| lane_ok(*l)).map(|l| {
                let mut cost = 0.0f32;
                if Some(l) == last_lane { cost += if dt < 140.0 { 9.0 } else if dt < 260.0 { 2.5 } else { 0.8 } }
                if Some(l) == prev_lane { cost += 0.8 }
                if let Some(ll) = last_lane { if (ll / 2 != l / 2) && dt < 300.0 { cost -= 0.5 } } // alternar manos
                cost += match (e.band, l) { (Band::Low, 1 | 2) => -0.3, (Band::High, 0 | 3) => -0.3, _ => 0.0 };
                cost += hash(seed, e.time.0 as u64 ^ (l as u64) << 56) * 0.4;
                (l, cost)
            }).min_by(|a, b| a.1.partial_cmp(&b.1).unwrap().then(a.0.cmp(&b.0))).map(|x| x.0)
        };
        let Some(lane) = pick(&|l| lane_free[l as usize] < e.time) else {
            repairs.push(format!("evento a {} ms descartado: sin carril libre", e.time.as_ms() as i64)); continue
        };
        lane_free[lane as usize] = end.unwrap_or(e.time) + Micros::from_ms(60.0);
        if let Some(k) = slot_key { learned.entry(k).or_insert(lane); }
        notes.push(Note { id, time: e.time, lane,
            kind: end.map_or(NoteKind::Tap, |end| NoteKind::Hold { end }), confidence: e.strength, pattern: e.pattern, manual: false });
        id += 1;
        // Acorde justificado: evento muy importante en un pulso (nunca en holds).
        if let (Some(min), None) = (prof.chord_min, end) {
            if importance(e) >= min && e.subdivision == Subdivision::Beat && e.downbeat {
                let first = lane;
                if let Some(l2) = pick(&|l| l != first && lane_free[l as usize] < e.time && (l as i8 - first as i8).abs() >= 2) {
                    lane_free[l2 as usize] = e.time + Micros::from_ms(60.0);
                    notes.push(Note { id, time: e.time, lane: l2, kind: NoteKind::Tap, confidence: e.strength, pattern: e.pattern, manual: false });
                    id += 1;
                }
            }
        }
        prev_lane = last_lane; last_lane = Some(lane); last_time = e.time;
    }

    // 4. Validación y reparación determinista.
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
        parameters: format!("holds={} density_scale={:.2}", opts.holds, opts.density_scale),
        notes, song_offset_ms: 0.0, edited_manually: false, repairs,
    }
}

pub fn generate_all(an: &Analysis, meta: &Meta, opts: &GenOptions) -> Vec<Beatmap> {
    Difficulty::ALL.iter().map(|d| generate(an, *d, meta, opts)).collect()
}
