//! Estructura rítmica sobre onsets + beats: subdivisión, patrones por compás y segmentación.
//! Distingue evento detectado → posición en la cuadrícula → patrón reconocido.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Subdivision { Beat, Eighth, Sixteenth, Triplet, OffGrid }

/// Posición de un evento en la cuadrícula de beats.
#[derive(Debug, Clone, Copy)]
pub struct Grid { pub beat_pos: f64, pub sub: Subdivision, pub err_ms: f32 }

/// Fracciones de beat reconocidas y su etiqueta.
const FRACS: [(f64, Subdivision); 7] = [
    (0.0, Subdivision::Beat), (0.25, Subdivision::Sixteenth), (1.0 / 3.0, Subdivision::Triplet), (0.5, Subdivision::Eighth),
    (2.0 / 3.0, Subdivision::Triplet), (0.75, Subdivision::Sixteenth), (1.0, Subdivision::Beat)];

/// Asigna cada instante (en muestras) a la cuadrícula definida por `beats` (muestras, crecientes).
/// Fuera del rango rastreado (más de un beat antes/después) devuelve `None`: no se fuerza un pulso.
pub fn assign_grid(samples: &[u64], beats: &[u64], sr: u32) -> Vec<Option<Grid>> {
    samples.iter().map(|&s| {
        if beats.len() < 2 { return None }
        let k = match beats.binary_search(&s) { Ok(i) => i, Err(0) => 0, Err(i) => i - 1 };
        let k = k.min(beats.len() - 2);
        let (b0, b1) = (beats[k] as f64, beats[k + 1] as f64);
        let iv = b1 - b0;
        let x = (s as f64 - b0) / iv; // puede ser <0 antes del primero o >1 tras el último
        if x < -1.0 || x > 2.0 { return None }
        let whole = x.floor();
        let frac = x - whole;
        let (q, sub) = FRACS.iter().min_by(|a, b| (a.0 - frac).abs().partial_cmp(&(b.0 - frac).abs()).unwrap()).copied().unwrap();
        let err_ms = ((frac - q).abs() * iv / sr as f64 * 1000.0) as f32;
        let sub = if err_ms > 35.0 || (frac - q).abs() > 0.12 { Subdivision::OffGrid } else { sub };
        Some(Grid { beat_pos: k as f64 + whole + q, sub, err_ms })
    }).collect()
}

#[derive(Debug, Clone, Copy)]
pub struct BarSlot { pub bar: u32, /// 0..48 (4 beats × 12 posiciones: sirve a semicorcheas y tresillos).
    pub slot: u8 }

/// Compás de 4 beats con `phase` como primer tiempo (heurística: el bombo marca el 1).
pub fn bar_slot(beat_pos: f64, phase: usize) -> Option<BarSlot> {
    let p = beat_pos - phase as f64;
    if p < 0.0 { return None }
    let bar = (p / 4.0).floor();
    let slot = (((p - bar * 4.0) * 12.0).round() as i64).rem_euclid(48) as u8;
    Some(BarSlot { bar: bar as u32, slot })
}

/// Agrupa compases con firmas parecidas (Jaccard ≥ 0.7) y les da un id. Devuelve id por compás.
pub fn cluster_bars(signatures: &[u64]) -> Vec<Option<u32>> {
    let mut protos: Vec<u64> = vec![];
    signatures.iter().map(|&sig| {
        if sig == 0 { return None }
        let found = protos.iter().position(|&p| {
            let (i, u) = ((p & sig).count_ones() as f64, (p | sig).count_ones() as f64);
            i / u >= 0.7
        });
        Some(found.unwrap_or_else(|| { protos.push(sig); protos.len() - 1 }) as u32)
    }).collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment { pub start_s: f64, pub end_s: f64, /// 0 baja … 2 alta intensidad.
    pub level: u8, pub silent: bool }

/// Segmentación por densidad de onsets ponderada y energía en ventanas de 2 s.
/// `onsets`: (segundos, fuerza) de los eventos aceptados.
pub fn segment(duration_s: f64, rms: &[f32], rms_hop_s: f64, onsets: &[(f64, f32)], silence_rms: f32) -> Vec<Segment> {
    const W: f64 = 2.0;
    let n = ((duration_s / W).ceil() as usize).max(1);
    let mut dens = vec![0f32; n];
    for (t, s) in onsets { if let Some(d) = dens.get_mut((t / W) as usize) { *d += s } }
    let silent: Vec<bool> = (0..n).map(|i| {
        let (a, b) = ((i as f64 * W / rms_hop_s) as usize, (((i + 1) as f64 * W / rms_hop_s) as usize).min(rms.len()));
        a >= b || rms[a..b].iter().sum::<f32>() / (b - a) as f32 <= silence_rms
    }).collect();
    let smooth: Vec<f32> = (0..n).map(|i| {
        let (a, b) = (i.saturating_sub(1), (i + 2).min(n));
        dens[a..b].iter().sum::<f32>() / (b - a) as f32
    }).collect();
    let mut live: Vec<f32> = (0..n).filter(|i| !silent[*i]).map(|i| smooth[i]).collect();
    live.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let q = |f: f64| live.get(((live.len() as f64 * f) as usize).min(live.len().saturating_sub(1))).copied().unwrap_or(0.0);
    let (t1, t2) = (q(1.0 / 3.0), q(2.0 / 3.0));
    let level = |i: usize| if smooth[i] <= t1 { 0u8 } else if smooth[i] <= t2 { 1 } else { 2 };
    let mut segs: Vec<Segment> = vec![];
    for i in 0..n {
        let (s, e) = (i as f64 * W, ((i + 1) as f64 * W).min(duration_s));
        let (lv, sil) = (if silent[i] { 0 } else { level(i) }, silent[i]);
        match segs.last_mut() {
            Some(l) if l.level == lv && l.silent == sil => l.end_s = e,
            _ => segs.push(Segment { start_s: s, end_s: e, level: lv, silent: sil }),
        }
    }
    // Absorber segmentos de menos de 6 s (salvo silencios) en el anterior.
    let mut out: Vec<Segment> = vec![];
    for s in segs {
        match out.last_mut() {
            Some(l) if !s.silent && !l.silent && s.end_s - s.start_s < 6.0 => l.end_s = s.end_s,
            _ => out.push(s),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grid_labels() {
        let sr = 1000; // 1 muestra = 1 ms
        let beats: Vec<u64> = (0..8).map(|i| i * 500).collect();
        let got = assign_grid(&[500, 750, 625, 667, 1210], &beats, sr);
        assert_eq!(got[0].unwrap().sub, Subdivision::Beat);
        assert_eq!(got[1].unwrap().sub, Subdivision::Eighth);
        assert_eq!(got[2].unwrap().sub, Subdivision::Sixteenth);
        assert_eq!(got[3].unwrap().sub, Subdivision::Triplet);
        assert_eq!(got[4].unwrap().sub, Subdivision::OffGrid);
        assert!((got[1].unwrap().beat_pos - 1.5).abs() < 1e-9);
    }
    #[test]
    fn no_grid_far_from_beats() {
        let beats = vec![10_000u64, 10_500, 11_000];
        assert!(assign_grid(&[0], &beats, 1000)[0].is_none());
    }
    #[test]
    fn repeated_bars_share_pattern() {
        let a = 0b1001_0010_0100u64;
        let ids = cluster_bars(&[a, a, 0, a | 1 << 40, 0b1111u64 << 30]);
        assert_eq!(ids[0], ids[1]);
        assert_eq!(ids[2], None);
        assert_eq!(ids[3], ids[0]);
        assert_ne!(ids[4], ids[0]);
    }
    #[test]
    fn segmentation_flags_silence_and_levels() {
        let rms: Vec<f32> = (0..2000).map(|i| if (800..1200).contains(&i) { 0.0 } else { 0.1 }).collect();
        let onsets: Vec<(f64, f32)> = (0..40).map(|i| (i as f64 * 0.5, 1.0)).chain((100..300).map(|i| (i as f64 * 0.1, 1.0))).collect();
        let seg = segment(40.0, &rms, 0.02, &onsets, 0.01);
        assert!(seg.iter().any(|s| s.silent));
        assert!(seg.iter().map(|s| s.level).collect::<std::collections::HashSet<_>>().len() >= 2);
        assert!((seg.last().unwrap().end_s - 40.0).abs() < 1e-9);
    }
}
