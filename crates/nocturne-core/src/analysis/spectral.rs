//! STFT por bloques y características por fotograma.
//!
//! * Ventana de Hann de `n_fft` (potencia de 2 ≈ 46 ms) y `hop = n_fft/8` (≈ 5,8 ms a 44,1 kHz).
//! * Solo se conserva un banco de `N_BANDS` bandas triangulares de espaciado logarítmico
//!   (memoria ≈ 4·N_BANDS B por fotograma); el espectro lineal completo no se almacena.
//! * Compresión logarítmica `ln(1 + 100·x)` con x normalizado (seno a fondo de escala ≈ 1).

use rustfft::{num_complex::Complex, FftPlanner};
use std::sync::atomic::{AtomicBool, Ordering};

pub const N_BANDS: usize = 80;
const F_MIN: f32 = 30.0;
const LOW_HZ: f32 = 200.0;
const HIGH_HZ: f32 = 2500.0;

pub struct Features {
    pub sr: u32,
    pub n_fft: usize,
    pub hop: usize,
    /// `bands[t][b]`: magnitud comprimida.
    pub bands: Vec<[f32; N_BANDS]>,
    pub rms: Vec<f32>,
    pub centroid_hz: Vec<f32>,
    /// Frecuencia central de cada banda.
    pub band_hz: [f32; N_BANDS],
    pub low_end: usize,
    pub high_start: usize,
}

impl Features {
    pub fn frames(&self) -> usize { self.bands.len() }
    pub fn fps(&self) -> f64 { self.sr as f64 / self.hop as f64 }
    /// Instante (en muestras) del centro del fotograma `t`.
    pub fn center_sample(&self, t: usize) -> usize { t * self.hop + self.n_fft / 2 }
}

pub fn fft_size_for(sr: u32) -> usize {
    let target = (sr as f64 * 0.0464) as usize;
    target.next_power_of_two().clamp(1024, 4096)
}

fn filterbank(sr: u32, n_fft: usize) -> (Vec<(usize, Vec<f32>)>, [f32; N_BANDS]) {
    let f_max = (sr as f32 / 2.0 * 0.95).min(16000.0);
    let ratio = (f_max / F_MIN).powf(1.0 / (N_BANDS as f32 + 1.0));
    let centers: Vec<f32> = (0..N_BANDS + 2).map(|i| F_MIN * ratio.powi(i as i32)).collect();
    let bin_hz = sr as f32 / n_fft as f32;
    let mut bank = Vec::with_capacity(N_BANDS);
    let mut band_hz = [0f32; N_BANDS];
    for b in 0..N_BANDS {
        let (lo, c, hi) = (centers[b], centers[b + 1], centers[b + 2]);
        band_hz[b] = c;
        let k0 = ((lo / bin_hz).floor() as usize).max(1);
        let k1 = ((hi / bin_hz).ceil() as usize).min(n_fft / 2);
        let mut w: Vec<f32> = (k0..=k1).map(|k| {
            let f = k as f32 * bin_hz;
            if f <= lo || f >= hi { 0.0 } else if f < c { (f - lo) / (c - lo) } else { (hi - f) / (hi - c) }
        }).collect();
        let sum: f32 = w.iter().sum();
        if sum > 0.0 { w.iter_mut().for_each(|x| *x /= sum) } else {
            // Banda más estrecha que un bin: asignar al bin más cercano.
            let k = ((c / bin_hz).round() as usize).clamp(1, n_fft / 2);
            bank.push((k, vec![1.0])); continue;
        }
        bank.push((k0, w));
    }
    (bank, band_hz)
}

/// Calcula las características. Devuelve `None` si se cancela.
pub fn compute(mono: &[f32], sr: u32, cancel: &AtomicBool) -> Option<Features> {
    let n_fft = fft_size_for(sr);
    let hop = n_fft / 8;
    let (bank, band_hz) = filterbank(sr, n_fft);
    let low_end = band_hz.iter().position(|f| *f > LOW_HZ).unwrap_or(N_BANDS);
    let high_start = band_hz.iter().position(|f| *f > HIGH_HZ).unwrap_or(N_BANDS);
    let window: Vec<f32> = (0..n_fft)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n_fft as f32).cos()).collect();
    let norm = 4.0 / n_fft as f32; // seno a fondo de escala → magnitud ≈ 1
    let fft = FftPlanner::<f32>::new().plan_fft_forward(n_fft);
    let mut buf = vec![Complex::new(0f32, 0.0); n_fft];
    let mut scratch = vec![Complex::new(0f32, 0.0); fft.get_inplace_scratch_len()];
    let bin_hz = sr as f32 / n_fft as f32;

    let frames = if mono.len() >= n_fft { (mono.len() - n_fft) / hop + 1 } else { 0 };
    let mut f = Features { sr, n_fft, hop, bands: Vec::with_capacity(frames), rms: Vec::with_capacity(frames),
        centroid_hz: Vec::with_capacity(frames), band_hz, low_end, high_start };
    let mut mags = vec![0f32; n_fft / 2 + 1];
    for t in 0..frames {
        if t % 512 == 0 && cancel.load(Ordering::Relaxed) { return None }
        let seg = &mono[t * hop..t * hop + n_fft];
        let mut e = 0.0f32;
        for i in 0..n_fft { buf[i] = Complex::new(seg[i] * window[i], 0.0); e += seg[i] * seg[i]; }
        fft.process_with_scratch(&mut buf, &mut scratch);
        let (mut num, mut den) = (0f32, 1e-12f32);
        for k in 0..=n_fft / 2 {
            let m = buf[k].norm() * norm;
            mags[k] = m;
            num += m * k as f32 * bin_hz; den += m;
        }
        let mut row = [0f32; N_BANDS];
        for (b, (k0, w)) in bank.iter().enumerate() {
            let x: f32 = w.iter().enumerate().map(|(i, wi)| wi * mags[k0 + i]).sum();
            row[b] = (1.0 + 100.0 * x).ln();
        }
        f.bands.push(row);
        f.rms.push((e / n_fft as f32).sqrt());
        f.centroid_hz.push(num / den);
    }
    Some(f)
}

/// Flujo espectral positivo tipo SuperFlux: referencia = máximo local en frecuencia
/// (±1 banda) del fotograma `lag` atrás, lo que ignora el vibrato y las modulaciones lentas.
/// Devuelve (total, bajo, medio, alto).
pub fn flux(f: &Features, lag: usize) -> [Vec<f32>; 4] {
    let n = f.frames();
    let mut out: [Vec<f32>; 4] = [vec![0.0; n], vec![0.0; n], vec![0.0; n], vec![0.0; n]];
    for t in lag..n {
        let (cur, prev) = (&f.bands[t], &f.bands[t - lag]);
        for b in 0..N_BANDS {
            let r = prev[b].max(if b > 0 { prev[b - 1] } else { 0.0 }).max(if b + 1 < N_BANDS { prev[b + 1] } else { 0.0 });
            let d = (cur[b] - r).max(0.0);
            out[0][t] += d;
            let region = if b < f.low_end { 1 } else if b < f.high_start { 2 } else { 3 };
            out[region][t] += d;
        }
    }
    out
}
