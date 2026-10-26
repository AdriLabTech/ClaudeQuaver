//! Detección de onsets: candidatos con evidencia, nunca notas definitivas.
//!
//! Pipeline: flujo espectral (SuperFlux) → máximos locales → umbral adaptativo
//! (media local + fracción de la referencia global) → supresión por distancia mínima
//! → ajuste fino en el dominio del tiempo (sobre la señal, no sobre el fotograma).

use super::spectral::{self, Features};
use super::AnalysisParams;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Band { Low, Mid, High }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rejection {
    /// Máximo local por debajo del umbral adaptativo.
    BelowThreshold,
    /// Suprimido por un evento más fuerte a menos de `min_gap_ms`.
    TooClose,
    /// Bajo el umbral de silencio absoluto.
    Silent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    /// Instante en muestras (ajustado en el dominio del tiempo cuando es fiable).
    pub sample: u64,
    /// 0..1 respecto a la referencia global de la canción (heurística, no probabilidad).
    pub strength: f32,
    pub band: Band,
    /// RMS local en dBFS.
    pub level_db: f32,
    pub refined: bool,
    pub rejected: Option<Rejection>,
}

pub struct OnsetResult {
    /// Envolvente de onsets (flujo total), una muestra por fotograma.
    pub envelope: Vec<f32>,
    pub low: Vec<f32>,
    pub candidates: Vec<Candidate>,
    pub global_ref: f32,
}

fn percentile_top_mean(v: &[f32], frac: f32) -> f32 {
    let mut s: Vec<f32> = v.iter().copied().filter(|x| *x > 0.0).collect();
    if s.is_empty() { return 0.0 }
    s.sort_by(|a, b| b.partial_cmp(a).unwrap());
    let k = ((s.len() as f32 * frac).ceil() as usize).clamp(1, s.len());
    s[..k].iter().sum::<f32>() / k as f32
}

/// Suelo absoluto del flujo (suma de incrementos log en 80 bandas): por debajo es ruido numérico.
pub const ABS_FLOOR: f32 = 0.4;

pub fn detect(mono: &[f32], f: &Features, p: &AnalysisParams) -> OnsetResult {
    let [env, low, mid, high] = spectral::flux(f, 1);
    let n = env.len();
    let fps = f.fps();
    let global_ref = percentile_top_mean(&env, 0.02);
    let k = 0.22 - 0.18 * p.onset_sensitivity.clamp(0.0, 1.0);
    let delta = k * global_ref;

    // Media local (±0,4 s) con sumas prefijas.
    let pre: Vec<f64> = std::iter::once(0.0).chain(env.iter().scan(0.0f64, |a, x| { *a += *x as f64; Some(*a) })).collect();
    let half = (0.4 * fps) as usize;
    let local_mean = |t: usize| -> f32 {
        let (a, b) = (t.saturating_sub(half), (t + half + 1).min(n));
        ((pre[b] - pre[a]) / (b - a) as f64) as f32
    };
    let w = (0.025 * fps).round().max(1.0) as usize; // ventana de máximo local ±25 ms
    let silence_rms = 10f32.powf(p.silence_db / 20.0);

    let mut cands = vec![];
    for t in 1..n.saturating_sub(1) {
        let o = env[t];
        if o < ABS_FLOOR { continue }
        let (a, b) = (t.saturating_sub(w), (t + w + 1).min(n));
        if env[a..b].iter().any(|x| *x > o) || (env[a..t].iter().any(|x| *x == o)) { continue }
        let thr = local_mean(t) * 1.6 + delta;
        let rms_here = f.rms[t.saturating_sub(2)..(t + 6).min(n)].iter().cloned().fold(0.0, f32::max);
        let rejected = if rms_here < silence_rms { Some(Rejection::Silent) }
            else if o < thr { if o < 0.6 * thr { continue } else { Some(Rejection::BelowThreshold) } }
            else { None };
        let (lo, hi) = (t.saturating_sub(2), (t + 3).min(n));
        let sum = |v: &Vec<f32>| v[lo..hi].iter().sum::<f32>();
        let (sl, sm, sh) = (sum(&low), sum(&mid), sum(&high));
        let tot = (sl + sm + sh).max(1e-9);
        let band = if sl / tot > 0.45 { Band::Low } else if sh / tot > 0.5 { Band::High } else { Band::Mid };
        let strength = if global_ref > 0.0 { (o / (global_ref * 1.2)).min(1.0) } else { 0.0 };
        let c0 = f.center_sample(t);
        let (sample, refined) = match snap_time(mono, f.sr, t * f.hop, f.n_fft) {
            Some(s) => (s as u64, true),
            None => (c0 as u64, false),
        };
        cands.push(Candidate { sample, strength, band, level_db: 20.0 * rms_here.max(1e-9).log10(), refined, rejected });
    }

    // Supresión por distancia mínima: el más fuerte gana.
    let gap = (p.min_gap_ms / 1000.0 * f.sr as f64) as u64;
    let mut order: Vec<usize> = (0..cands.len()).filter(|i| cands[*i].rejected.is_none()).collect();
    order.sort_by(|a, b| cands[*b].strength.partial_cmp(&cands[*a].strength).unwrap());
    let mut kept: Vec<u64> = vec![];
    for i in order {
        let s = cands[i].sample;
        if kept.iter().any(|k| k.abs_diff(s) < gap) { cands[i].rejected = Some(Rejection::TooClose) } else { kept.push(s) }
    }
    cands.sort_by_key(|c| c.sample);
    OnsetResult { envelope: env, low, candidates: cands, global_ref }
}

/// Localiza el ataque en el dominio del tiempo dentro de `[start, start+len)`:
/// máximo del cociente de energía (post/pre) del residuo de primera diferencia.
/// Devuelve `None` si no hay un salto de ≥ 6 dB (ataque difuso: se usa el fotograma).
pub fn snap_time(x: &[f32], sr: u32, start: usize, len: usize) -> Option<usize> {
    let a = (0.002 * sr as f64) as usize;
    let b = (0.004 * sr as f64) as usize;
    let lo = start.max(b + 1);
    let hi = (start + len).min(x.len().saturating_sub(a + 1));
    if hi <= lo + 4 { return None }
    let energy = |from: usize, to: usize| -> f64 {
        (from..to).map(|i| { let d = (x[i] - x[i - 1]) as f64; d * d }).sum::<f64>()
    };
    let mut best = (0f64, lo);
    for p in lo..hi {
        let after = energy(p, p + a);
        let before = energy(p - b, p);
        let r = (after + 1e-10) / (before * (a as f64 / b as f64) + 1e-10);
        if r > best.0 { best = (r, p) }
    }
    if best.0 >= 4.0 { Some(best.1) } else { None }
}
