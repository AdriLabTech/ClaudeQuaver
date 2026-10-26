//! Suavizado del reloj musical.
//!
//! El dispositivo de audio publica su posición en saltos (un valor por callback, con jitter de
//! planificación de ±1–5 ms). Dibujar con ese valor crudo produce temblor en las notas. Se
//! sigue la *diferencia* `posición_audio − reloj_monotónico` con un filtro exponencial: el
//! tiempo de juego avanza con el reloj monotónico (suave a cualquier FPS) y deriva hacia el
//! audio real. Saltos grandes (seek, reanudación) se aceptan al instante.

#[derive(Debug, Clone)]
pub struct Smoother {
    offset_us: f64,
    initialized: bool,
    /// Constante de seguimiento por segundo de reloj de pared (mayor = sigue más rápido).
    pub rate_per_s: f64,
    /// Diferencia por encima de la cual se considera un salto real y no jitter.
    pub snap_us: f64,
    last_wall_us: f64,
    last_out_us: f64,
}

impl Default for Smoother {
    fn default() -> Self { Self { offset_us: 0.0, initialized: false, rate_per_s: 4.0, snap_us: 60_000.0, last_wall_us: 0.0, last_out_us: 0.0 } }
}

impl Smoother {
    pub fn reset(&mut self) { self.initialized = false }

    /// `raw_us`: posición audible estimada; `wall_us`: reloj monotónico. Devuelve tiempo suavizado,
    /// monótono no decreciente salvo que se haya producido un salto (seek).
    pub fn update(&mut self, raw_us: f64, wall_us: f64) -> f64 {
        let target = raw_us - wall_us;
        if !self.initialized || (target - self.offset_us).abs() > self.snap_us {
            self.offset_us = target;
            self.initialized = true;
            self.last_wall_us = wall_us;
            self.last_out_us = wall_us + self.offset_us;
            return self.last_out_us;
        }
        let dt = ((wall_us - self.last_wall_us) / 1e6).clamp(0.0, 0.25);
        let k = 1.0 - (-self.rate_per_s * dt).exp();
        self.offset_us += (target - self.offset_us) * k;
        self.last_wall_us = wall_us;
        let out = wall_us + self.offset_us;
        self.last_out_us = out.max(self.last_out_us); // nunca retrocede dentro de la misma pista
        self.last_out_us
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reloj de audio ideal + jitter determinista en la publicación (por callbacks de 10 ms).
    fn run(jitter_us: f64) -> (f64, f64, bool) {
        let mut s = Smoother::default();
        let (mut raw_err, mut out_err) = (0f64, 0f64);
        let mut mono = true;
        let mut prev = f64::MIN;
        let mut rng = 12345u64;
        let mut published = 0.0f64;
        let mut n = 0.0;
        for i in 0..6000 {
            let wall = i as f64 * 1000.0; // 1 ms por iteración (1000 FPS simulados)
            if i % 10 == 0 {
                rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let j = ((rng >> 33) as f64 / (1u64 << 31) as f64 * 2.0 - 1.0) * jitter_us;
                published = wall + j; // posición real = wall, publicada con ruido
            }
            let raw = published + (wall % 10_000.0).min(wall - (wall - wall % 10_000.0)) ; // interpolada entre callbacks
            let out = s.update(raw, wall);
            if i > 1000 { raw_err += (raw - wall).abs(); out_err += (out - wall).abs(); n += 1.0; }
            if out < prev { mono = false }
            prev = out;
        }
        (raw_err / n, out_err / n, mono)
    }

    #[test]
    fn smoothing_reduces_jitter_and_is_monotonic() {
        let (raw, out, mono) = run(4000.0);
        println!("error medio: crudo {raw:.0} µs, suavizado {out:.0} µs");
        assert!(out < raw * 0.6, "{out} vs {raw}");
        assert!(mono);
    }
    #[test]
    fn seek_snaps_immediately() {
        let mut s = Smoother::default();
        for i in 0..100 { s.update(i as f64 * 1000.0, i as f64 * 1000.0); }
        let out = s.update(30_000_000.0, 100_000.0);
        assert!((out - 30_000_000.0).abs() < 1.0);
    }
    #[test]
    fn follows_real_drift() {
        // El audio va un 0,1 % más rápido que el reloj de pared: el suavizado debe seguirlo.
        let mut s = Smoother::default();
        let mut out = 0.0;
        for i in 0..20_000 { let w = i as f64 * 1000.0; out = s.update(w * 1.001, w); }
        assert!((out - 20_000.0 * 1000.0 * 1.001).abs() < 3000.0, "{}", out - 20_000_000.0 * 1.001);
    }
}
