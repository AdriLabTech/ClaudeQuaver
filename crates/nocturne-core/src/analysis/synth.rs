//! Generadores de señales con instantes conocidos para validar el analizador.
//! También se usan desde el binario de herramientas para producir audio de prueba.

pub struct Rng(u64);
impl Rng {
    pub fn new(seed: u64) -> Self { Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1) }
    pub fn next_f32(&mut self) -> f32 {
        self.0 ^= self.0 << 13; self.0 ^= self.0 >> 7; self.0 ^= self.0 << 17;
        ((self.0 >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
    }
}

pub struct Track { pub sr: u32, pub samples: Vec<f32> }

impl Track {
    pub fn silence(sr: u32, secs: f64) -> Self { Self { sr, samples: vec![0.0; (secs * sr as f64) as usize] } }
    pub fn len_secs(&self) -> f64 { self.samples.len() as f64 / self.sr as f64 }

    fn add_at(&mut self, start: f64, data: &[f32]) {
        let s = (start * self.sr as f64).round() as usize;
        for (i, v) in data.iter().enumerate() { if let Some(x) = self.samples.get_mut(s + i) { *x += v } }
    }

    /// Golpe percusivo: ruido (hi-hat/caja) + seno que decae (bombo). `tone_hz = 0` → solo ruido.
    pub fn hit(&mut self, at: f64, amp: f32, tone_hz: f32, decay_ms: f32, seed: u64) {
        let n = (self.sr as f32 * decay_ms * 5.0 / 1000.0) as usize;
        let mut rng = Rng::new(seed);
        let d: Vec<f32> = (0..n).map(|i| {
            let t = i as f32 / self.sr as f32;
            let env = (-t * 1000.0 / decay_ms).exp();
            let tone = if tone_hz > 0.0 { (2.0 * std::f32::consts::PI * tone_hz * t).sin() } else { 0.0 };
            let noise = if tone_hz > 0.0 { 0.0 } else { rng.next_f32() };
            amp * env * (tone + noise)
        }).collect();
        self.add_at(at, &d);
    }
    /// Impulso corto (click de 1 ms).
    pub fn click(&mut self, at: f64, amp: f32) { self.hit(at, amp, 1800.0, 3.0, 1) }

    /// Nota con ataque rápido y sostenimiento.
    pub fn tone(&mut self, at: f64, secs: f64, hz: f32, amp: f32) {
        let n = (secs * self.sr as f64) as usize;
        let d: Vec<f32> = (0..n).map(|i| {
            let t = i as f32 / self.sr as f32;
            let env = (t / 0.004).min(1.0) * (1.0 - (t / secs as f32).powi(8)).max(0.0);
            amp * env * (2.0 * std::f32::consts::PI * hz * t).sin()
        }).collect();
        self.add_at(at, &d);
    }
    pub fn sustained_tone(sr: u32, secs: f64, hz: f32, amp: f32) -> Self {
        let mut t = Self::silence(sr, secs);
        for (i, s) in t.samples.iter_mut().enumerate() {
            *s = amp * (2.0 * std::f64::consts::PI * hz as f64 * i as f64 / sr as f64).sin() as f32;
        }
        t
    }
    pub fn add_noise(&mut self, amp: f32, seed: u64) {
        let mut r = Rng::new(seed);
        for s in &mut self.samples { *s += amp * r.next_f32() }
    }
    pub fn normalize_peak(&mut self, peak: f32) {
        let m = self.samples.iter().fold(0f32, |a, b| a.max(b.abs()));
        if m > 0.0 { self.samples.iter_mut().for_each(|s| *s *= peak / m) }
    }
}

/// Patrón estilo bombo-caja-hihat a `bpm`, con `beats` pulsos desde `start`. Devuelve instantes de los golpes.
pub fn drum_pattern(sr: u32, bpm: f64, start: f64, beats: usize, tail: f64) -> (Track, Vec<f64>) {
    let spb = 60.0 / bpm;
    let mut t = Track::silence(sr, start + beats as f64 * spb + tail);
    let mut times = vec![];
    for b in 0..beats {
        let at = start + b as f64 * spb;
        if b % 2 == 0 { t.hit(at, 0.9, 60.0, 120.0, b as u64) } else { t.hit(at, 0.7, 0.0, 70.0, b as u64 + 99) }
        times.push(at);
        t.hit(at + spb / 2.0, 0.25, 0.0, 25.0, b as u64 + 500); // hi-hat a corcheas (más débil)
    }
    (t, times)
}
