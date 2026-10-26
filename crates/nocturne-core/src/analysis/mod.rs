//! Pipeline de análisis musical. Ver `docs/AUDIO_ANALYSIS.md`.
//!
//! señal → STFT/bandas → flujo espectral → candidatos de onset → tempo (+secciones)
//! → beats (DP) → cuadrícula/subdivisión → patrones por compás → segmentación.

pub mod onset;
pub mod rhythm;
pub mod spectral;
pub mod synth;
pub mod tempo;

use crate::{decode::DecodedAudio, Error, Micros, Result};
use onset::{Band, Rejection};
use rhythm::{Segment, Subdivision};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use tempo::{TempoEstimate, TempoSection};

pub const ANALYZER_VERSION: &str = "1";

/// Parámetros del analizador (perfiles internos; el usuario ve solo calidad/sensibilidad).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisParams {
    /// 0 (conservador) … 1 (detecta ataques débiles).
    pub onset_sensitivity: f32,
    pub min_gap_ms: f64,
    pub silence_db: f32,
    /// Rigidez del seguimiento de beats (Ellis): más alto = más regular.
    pub beat_tightness: f64,
}

impl Default for AnalysisParams {
    fn default() -> Self { Self { onset_sensitivity: 0.5, min_gap_ms: 45.0, silence_db: -60.0, beat_tightness: 100.0 } }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Beat { pub time: Micros, pub downbeat: bool }

/// Un evento detectado con todo lo necesario para explicar por qué se acepta o se rechaza.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub time: Micros,
    pub strength: f32,
    pub band: Band,
    pub level_db: f32,
    pub refined: bool,
    pub beat_pos: Option<f64>,
    pub subdivision: Subdivision,
    pub grid_err_ms: f32,
    pub bar: Option<u32>,
    pub pattern: Option<u32>,
    /// Intensidad de la sección (0..2) donde cae.
    pub section_level: u8,
    pub rejected: Option<Rejection>,
    /// Pulso fuerte del compás (primer tiempo estimado).
    pub downbeat: bool,
    /// Duración durante la que la energía se mantiene ≥ −9 dB del pico (0 en golpes secos).
    pub sustain_ms: u32,
}

impl Event {
    pub fn accepted(&self) -> bool { self.rejected.is_none() }
    pub fn explain(&self) -> String {
        let why = match self.rejected {
            None => "aceptado".to_string(),
            Some(Rejection::BelowThreshold) => "rechazado: por debajo del umbral adaptativo".into(),
            Some(Rejection::TooClose) => "rechazado: a menos de la distancia mínima de un evento más fuerte".into(),
            Some(Rejection::Silent) => "rechazado: nivel bajo el umbral de silencio".into(),
        };
        format!("{:.3}s fuerza {:.2} banda {:?} {:.0} dB subdiv {:?} (±{:.0} ms) patrón {:?} → {why}",
            self.time.as_secs(), self.strength, self.band, self.level_db, self.subdivision, self.grid_err_ms, self.pattern)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AnalysisStats {
    pub candidates: usize,
    pub accepted: usize,
    pub rejected_threshold: usize,
    pub rejected_close: usize,
    pub rejected_silent: usize,
    pub beats: usize,
    pub patterns: usize,
    pub elapsed_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Analysis {
    pub sample_rate: u32,
    pub duration: Micros,
    pub tempo: Option<TempoEstimate>,
    pub tempo_sections: Vec<TempoSection>,
    pub beats: Vec<Beat>,
    pub events: Vec<Event>,
    pub segments: Vec<Segment>,
    pub stats: AnalysisStats,
    pub warnings: Vec<String>,
}

fn next_accepted(c: &[onset::Candidate], i: usize) -> Option<u64> {
    c[i + 1..].iter().find(|x| x.rejected.is_none()).map(|x| x.sample)
}

/// Milisegundos que la energía se mantiene ≥ −9 dB (×0,35) del nivel *posterior al transitorio*
/// (≈ 60 ms tras el ataque), para que un golpe fuerte no oculte una nota que sigue sonando.
fn sustain(f: &spectral::Features, sample: usize, next: Option<u64>) -> u32 {
    let skip = ((0.06 * f.fps()) as usize).max(2);
    let t0 = sample / f.hop;
    let (a, b) = (t0 + skip, (t0 + skip + 4).min(f.rms.len()));
    if a >= b { return 0 }
    let level = f.rms[a..b].iter().cloned().fold(0.0, f32::max);
    if level <= 0.0 { return 0 }
    let cap = (4.0 * f.fps()) as usize;
    let stop = next.map_or(usize::MAX, |n| n as usize / f.hop);
    let mut t = a;
    while t < f.rms.len() && t - t0 < cap && t < stop && f.rms[t] >= 0.35 * level { t += 1 }
    ((t - t0) as f64 / f.fps() * 1000.0) as u32
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() { return None }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Some(v[v.len() / 2])
}

pub fn analyze(audio: &DecodedAudio, params: &AnalysisParams, cancel: &AtomicBool,
               progress: &mut dyn FnMut(f32, &'static str)) -> Result<Analysis> {
    let started = std::time::Instant::now();
    let check = || if cancel.load(Ordering::Relaxed) { Err(Error::Cancelled) } else { Ok(()) };
    let sr = audio.sample_rate;
    let mono = audio.to_mono();
    let duration = audio.duration();
    if duration < Micros::from_secs(2.0) { return Err(Error::TooShort(duration.0 / 1000)) }
    if mono.iter().any(|s| !s.is_finite()) { return Err(Error::NonFinite) }
    let mut warnings = vec![];

    progress(0.05, "espectro");
    let f = spectral::compute(&mono, sr, cancel).ok_or(Error::Cancelled)?;
    check()?;
    progress(0.45, "onsets");
    let on = onset::detect(&mono, &f, params);
    let fps = f.fps();

    progress(0.6, "tempo");
    let tempo_est = tempo::estimate(&on.envelope, fps);
    let mut tempo_sections = vec![];
    let mut beats: Vec<Beat> = vec![];
    let mut phase = 0usize;
    if let Some(t) = &tempo_est {
        if t.confidence < 0.25 { warnings.push(format!("tempo poco fiable (confianza {:.2}); el mapa se apoya en los onsets", t.confidence)) }
        tempo_sections = tempo::sections(&on.envelope, fps, t);
        check()?;
        progress(0.7, "beats");
        // Beats por sección, en fotogramas de la envolvente completa.
        let mut frames: Vec<usize> = vec![];
        for s in &tempo_sections {
            let (a, b) = ((s.start_s * fps) as usize, ((s.end_s * fps) as usize).min(on.envelope.len()));
            if b <= a + 16 { continue }
            let period = fps * 60.0 / s.bpm;
            for fr in tempo::track_beats(&on.envelope[a..b], fps, s.bpm, params.beat_tightness) {
                let fr = fr + a;
                if frames.last().map_or(true, |l| (fr - l) as f64 > 0.6 * period) { frames.push(fr) }
            }
        }
        // Calibración: sesgo entre el centro de fotograma y el ataque real (mediana).
        let accepted: Vec<u64> = on.candidates.iter().filter(|c| c.rejected.is_none()).map(|c| c.sample).collect();
        let centers: Vec<i64> = frames.iter().map(|fr| f.center_sample(*fr) as i64).collect();
        let tol = (0.06 * sr as f64) as i64;
        let offsets: Vec<f64> = centers.iter().filter_map(|c| accepted.iter().map(|a| *a as i64 - c)
            .filter(|d| d.abs() <= tol).min_by_key(|d| d.abs()).map(|d| d as f64)).collect();
        let bias = if offsets.len() >= 4 { median(offsets).unwrap() } else { 0.0 };
        let snap = (0.03 * sr as f64) as i64;
        let samples: Vec<u64> = centers.iter().map(|c| {
            let est = *c as f64 + bias;
            let near = accepted.iter().map(|a| *a as i64).filter(|a| (a - est as i64).abs() <= snap).min_by_key(|a| (a - est as i64).abs());
            near.unwrap_or(est.max(0.0) as i64) as u64
        }).collect();
        // No forzar pulsos en intros/colas sin eventos: recortar extremos sin onset cercano.
        let near = |s: u64| accepted.iter().any(|a| a.abs_diff(s) <= (0.05 * sr as f64) as u64);
        let (mut lo, mut hi) = (0usize, samples.len());
        while lo < hi && !near(samples[lo]) { lo += 1 }
        while hi > lo && !near(samples[hi - 1]) { hi -= 1 }
        let samples: Vec<u64> = samples[lo..hi].to_vec();
        // Primer tiempo: fase (0..3) con más energía grave en los beats.
        if samples.len() >= 8 {
            let mut acc = [0f64; 4];
            let mut cnt = [0usize; 4];
            for (i, s) in samples.iter().enumerate() {
                let fr = (*s as usize).saturating_sub(f.n_fft / 2) / f.hop;
                let lo = on.low[fr.saturating_sub(1)..(fr + 3).min(on.low.len())].iter().sum::<f32>() as f64;
                acc[i % 4] += lo; cnt[i % 4] += 1;
            }
            phase = (0..4).max_by(|a, b| (acc[*a] / cnt[*a].max(1) as f64).partial_cmp(&(acc[*b] / cnt[*b].max(1) as f64)).unwrap()).unwrap();
        }
        beats = samples.iter().enumerate().map(|(i, s)| Beat { time: Micros::from_samples(*s, sr), downbeat: i >= phase && (i - phase) % 4 == 0 }).collect();
    } else {
        warnings.push("no se detectó una pulsación estable; se generará un mapa más escaso basado en onsets".into());
    }
    check()?;

    progress(0.85, "patrones");
    let beat_samples: Vec<u64> = beats.iter().map(|b| b.time.to_samples(sr) as u64).collect();
    let cand_samples: Vec<u64> = on.candidates.iter().map(|c| c.sample).collect();
    let grid = rhythm::assign_grid(&cand_samples, &beat_samples, sr);
    let slots: Vec<Option<rhythm::BarSlot>> = on.candidates.iter().zip(&grid).map(|(c, g)| {
        if c.rejected.is_some() { return None }
        g.as_ref().and_then(|g| if g.sub == Subdivision::OffGrid { None } else { rhythm::bar_slot(g.beat_pos, phase) })
    }).collect();
    let n_bars = slots.iter().flatten().map(|s| s.bar as usize + 1).max().unwrap_or(0);
    let mut sigs = vec![0u64; n_bars];
    for s in slots.iter().flatten() { sigs[s.bar as usize] |= 1u64 << s.slot }
    let bar_ids = rhythm::cluster_bars(&sigs);

    let acc_times: Vec<(f64, f32)> = on.candidates.iter().filter(|c| c.rejected.is_none())
        .map(|c| (c.sample as f64 / sr as f64, c.strength)).collect();
    let segments = rhythm::segment(duration.as_secs(), &f.rms, f.hop as f64 / sr as f64, &acc_times,
        10f32.powf((params.silence_db + 10.0) / 20.0));

    let events: Vec<Event> = on.candidates.iter().enumerate().map(|(i, c)| {
        let t = c.sample as f64 / sr as f64;
        let lvl = segments.iter().find(|s| t >= s.start_s && t < s.end_s).map_or(1, |s| s.level);
        let g = grid[i];
        let downbeat = c.rejected.is_none() && g.map_or(false, |g| g.sub == Subdivision::Beat && {
            let k = g.beat_pos.round() as usize;
            beats.get(k).map_or(false, |b| b.downbeat)
        });
        let sustain_ms = if c.rejected.is_some() { 0 } else { sustain(&f, c.sample as usize, next_accepted(&on.candidates, i)) };
        Event {
            time: Micros::from_samples(c.sample, sr), strength: c.strength, band: c.band, level_db: c.level_db, refined: c.refined,
            beat_pos: g.map(|g| g.beat_pos), subdivision: g.map_or(Subdivision::OffGrid, |g| g.sub), grid_err_ms: g.map_or(0.0, |g| g.err_ms),
            bar: slots[i].map(|s| s.bar), pattern: slots[i].and_then(|s| bar_ids[s.bar as usize]),
            section_level: lvl, rejected: c.rejected, downbeat, sustain_ms,
        }
    }).collect();

    let mut stats = AnalysisStats { candidates: events.len(), beats: beats.len(),
        patterns: bar_ids.iter().flatten().max().map_or(0, |m| *m as usize + 1), ..Default::default() };
    for e in &events { match e.rejected {
        None => stats.accepted += 1,
        Some(Rejection::BelowThreshold) => stats.rejected_threshold += 1,
        Some(Rejection::TooClose) => stats.rejected_close += 1,
        Some(Rejection::Silent) => stats.rejected_silent += 1,
    } }
    if stats.accepted == 0 { warnings.push("no se detectaron eventos rítmicos".into()) }
    stats.elapsed_ms = started.elapsed().as_millis();
    progress(1.0, "listo");
    Ok(Analysis { sample_rate: sr, duration, tempo: tempo_est, tempo_sections, beats, events, segments, stats, warnings })
}
