//! Estimación de tempo (autocorrelación de la envolvente de onsets) y seguimiento
//! de beats por programación dinámica (Ellis, 2007). Se mantienen hipótesis múltiples.

use serde::{Deserialize, Serialize};

pub const BPM_MIN: f64 = 60.0;
pub const BPM_MAX: f64 = 200.0;
const PRIOR_BPM: f64 = 120.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TempoEstimate {
    pub bpm: f64,
    /// Heurística 0..1 (prominencia del pico de periodicidad); no es una probabilidad calibrada.
    pub confidence: f32,
    /// Hipótesis alternativas (BPM, puntuación normalizada 0..1) incluida la ganadora.
    pub candidates: Vec<(f64, f32)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TempoSection { pub start_s: f64, pub end_s: f64, pub bpm: f64, pub confidence: f32 }

/// Quita la media móvil (≈1 s) y rectifica: resalta ataques, descarta nivel de fondo.
fn detrend(env: &[f32], fps: f64) -> Vec<f32> {
    let h = (0.5 * fps) as usize;
    let n = env.len();
    let pre: Vec<f64> = std::iter::once(0.0).chain(env.iter().scan(0.0f64, |a, x| { *a += *x as f64; Some(*a) })).collect();
    (0..n).map(|t| {
        let (a, b) = (t.saturating_sub(h), (t + h + 1).min(n));
        (env[t] - ((pre[b] - pre[a]) / (b - a) as f64) as f32).max(0.0)
    }).collect()
}

pub fn estimate(env: &[f32], fps: f64) -> Option<TempoEstimate> {
    // Mismo suelo absoluto que el detector: el ruido numérico no es ritmo.
    let gated: Vec<f32> = env.iter().map(|v| (v - super::onset::ABS_FLOOR).max(0.0)).collect();
    let x = detrend(&gated, fps);
    let n = x.len();
    let lag_min = (fps * 60.0 / BPM_MAX).floor() as usize;
    let lag_max = (fps * 60.0 / BPM_MIN).ceil() as usize * 2; // hasta 2 compases para armónicos
    if n < lag_max * 2 || x.iter().all(|v| *v == 0.0) { return None }
    let mut acf = vec![0f64; lag_max + 2];
    for lag in 1..acf.len() {
        let mut s = 0f64;
        for t in lag..n { s += (x[t] * x[t - lag]) as f64 }
        acf[lag] = s / (n - lag) as f64;
    }
    let zero = acf[1].max(1e-12);
    // Puntuación por lag: acf + armónicos (múltiplos), ponderada por el prior log-gaussiano.
    let score = |lag: f64| -> f64 {
        let at = |l: f64| -> f64 {
            let i = l.floor() as usize;
            if i + 1 >= acf.len() { return 0.0 }
            let f = l - i as f64;
            acf[i] * (1.0 - f) + acf[i + 1] * f
        };
        let bpm = fps * 60.0 / lag;
        let prior = (-0.5 * ((bpm / PRIOR_BPM).log2() / 1.0).powi(2)).exp();
        (at(lag) + 0.5 * at(lag * 2.0)) * prior
    };
    // Muestreo fino de lags y selección de máximos locales.
    let (lo, hi) = (lag_min.max(2) as f64, (fps * 60.0 / BPM_MIN).ceil());
    let step = 0.25;
    let pts: Vec<(f64, f64)> = (0..).map(|i| lo + i as f64 * step).take_while(|l| *l <= hi).map(|l| (l, score(l))).collect();
    let mut peaks: Vec<(f64, f64)> = (1..pts.len() - 1).filter(|&i| pts[i].1 > pts[i - 1].1 && pts[i].1 >= pts[i + 1].1)
        .map(|i| (fps * 60.0 / pts[i].0, pts[i].1)).collect();
    peaks.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    let mut chosen: Vec<(f64, f64)> = vec![];
    for p in peaks { if chosen.iter().all(|c| (c.0 / p.0 - 1.0).abs() > 0.04) { chosen.push(p) } if chosen.len() == 5 { break } }
    let best = *chosen.first()?;
    if best.1 <= 0.0 { return None }
    let mean = pts.iter().map(|p| p.1).sum::<f64>() / pts.len() as f64;
    let confidence = (((best.1 / mean.max(1e-12)) - 1.0) / 8.0).clamp(0.0, 1.0) as f32;
    let _ = zero;
    Some(TempoEstimate { bpm: best.0, confidence,
        candidates: chosen.iter().map(|c| (c.0, (c.1 / best.1) as f32)).collect() })
}

/// Tempo por ventanas de 12 s (paso 6 s), plegado a la octava del tempo global y fusionado
/// en secciones con tempo estable (±4 %). Los cambios de tempo reales quedan como secciones.
pub fn sections(env: &[f32], fps: f64, global: &TempoEstimate) -> Vec<TempoSection> {
    let win = (12.0 * fps) as usize;
    let hop = (6.0 * fps) as usize;
    let total_s = env.len() as f64 / fps;
    if env.len() < win { return vec![TempoSection { start_s: 0.0, end_s: total_s, bpm: global.bpm, confidence: global.confidence }] }
    let mut ws: Vec<(f64, f64, f64, f32)> = vec![];
    let mut s = 0;
    let mut prev = global.bpm;
    while s + win <= env.len() {
        let (bpm, conf) = match estimate(&env[s..s + win], fps) {
            Some(e) if e.confidence > 0.15 => {
                // Continuidad: si alguna octava del tempo de la ventana coincide con el
                // tempo anterior (±4 %), se elige esa; si no, manda la mejor hipótesis propia.
                let cont = e.candidates.iter().filter(|c| c.1 > 0.5).flat_map(|c| [c.0, c.0 * 2.0, c.0 / 2.0])
                    .find(|b| (b / prev - 1.0).abs() < 0.04);
                (cont.unwrap_or(e.bpm), e.confidence)
            }
            _ => (prev, 0.0), // sin información (silencio, final): hereda la sección previa
        };
        prev = bpm;
        ws.push((s as f64 / fps, (s + win) as f64 / fps, bpm, conf));
        s += hop;
    }
    // Fusionar ventanas consecutivas con tempo similar; una ventana aislada discrepante no abre sección.
    let mut out: Vec<TempoSection> = vec![];
    for (i, w) in ws.iter().enumerate() {
        let isolated = i > 0 && i + 1 < ws.len() && (ws[i - 1].2 / w.2 - 1.0).abs() > 0.04 && (ws[i + 1].2 / ws[i - 1].2 - 1.0).abs() <= 0.04;
        let bpm = if isolated { ws[i - 1].2 } else { w.2 };
        match out.last_mut() {
            Some(l) if (l.bpm / bpm - 1.0).abs() <= 0.04 => { l.end_s = w.1; l.confidence = l.confidence.max(w.3) }
            _ => out.push(TempoSection { start_s: w.0, end_s: w.1, bpm, confidence: w.3 }),
        }
    }
    if let Some(first) = out.first_mut() { first.start_s = 0.0 }
    if let Some(last) = out.last_mut() { last.end_s = total_s }
    // Alinear límites: cada sección empieza donde termina la anterior.
    for i in 1..out.len() { let b = (out[i - 1].end_s + out[i].start_s) / 2.0; out[i - 1].end_s = b; out[i].start_s = b }
    out
}

/// Seguimiento de beats (Ellis). `bpm` fija el periodo; devuelve índices de fotograma.
pub fn track_beats(env: &[f32], fps: f64, bpm: f64, tightness: f64) -> Vec<usize> {
    let n = env.len();
    let period = fps * 60.0 / bpm;
    if n < period as usize * 2 { return vec![] }
    let std = (env.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / n as f64).sqrt().max(1e-9);
    let o: Vec<f64> = env.iter().map(|v| *v as f64 / std).collect();
    let (lo, hi) = ((period / 2.0).round() as usize, (period * 2.0).round() as usize);
    let mut c = o.clone();
    let mut back = vec![usize::MAX; n];
    for t in 0..n {
        if t < lo { continue }
        let (a, b) = (t.saturating_sub(hi), t - lo);
        let mut best = (f64::NEG_INFINITY, usize::MAX);
        for tau in a..=b {
            let pen = tightness * ((((t - tau) as f64) / period).ln()).powi(2);
            let v = c[tau] - pen;
            if v > best.0 { best = (v, tau) }
        }
        if best.1 != usize::MAX && best.0 > 0.0 { c[t] = o[t] + best.0; back[t] = best.1 }
    }
    let tail = (n.saturating_sub((period * 1.5) as usize)).max(0);
    let mut cur = (tail..n).max_by(|a, b| c[*a].partial_cmp(&c[*b]).unwrap()).unwrap();
    let mut beats = vec![cur];
    while back[cur] != usize::MAX { cur = back[cur]; beats.push(cur) }
    beats.reverse();
    beats
}
