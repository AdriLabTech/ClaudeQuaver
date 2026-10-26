//! Tiempo inequívoco. Todo instante musical es un entero en microsegundos
//! desde el inicio del audio; se evita acumular errores de coma flotante.

use serde::{Deserialize, Serialize};
use std::ops::{Add, Sub};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Micros(pub i64);

impl Micros {
    pub const ZERO: Micros = Micros(0);
    pub fn from_ms(ms: f64) -> Self { Micros((ms * 1000.0).round() as i64) }
    pub fn from_secs(s: f64) -> Self { Micros((s * 1_000_000.0).round() as i64) }
    /// Conversión exacta (redondeada) desde un índice de muestra.
    pub fn from_samples(samples: u64, sample_rate: u32) -> Self {
        Micros(((samples as u128 * 1_000_000 + sample_rate as u128 / 2) / sample_rate as u128) as i64)
    }
    pub fn to_samples(self, sample_rate: u32) -> i64 {
        (self.0 as i128 * sample_rate as i128 / 1_000_000) as i64
    }
    pub fn as_ms(self) -> f64 { self.0 as f64 / 1000.0 }
    pub fn as_secs(self) -> f64 { self.0 as f64 / 1_000_000.0 }
    pub fn abs(self) -> Micros { Micros(self.0.abs()) }
}

impl Add for Micros { type Output = Micros; fn add(self, o: Micros) -> Micros { Micros(self.0 + o.0) } }
impl Sub for Micros { type Output = Micros; fn sub(self, o: Micros) -> Micros { Micros(self.0 - o.0) } }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn samples_roundtrip() {
        for sr in [22050u32, 44100, 48000, 96000] {
            for s in [0u64, 1, 12345, 44100 * 600] {
                let t = Micros::from_samples(s, sr);
                assert!((t.to_samples(sr) - s as i64).abs() <= 1, "sr={sr} s={s}");
            }
        }
    }
    #[test]
    fn ms_conversion() {
        assert_eq!(Micros::from_ms(16.5), Micros(16_500));
        assert_eq!(Micros::from_secs(1.0).as_ms(), 1000.0);
    }
}
