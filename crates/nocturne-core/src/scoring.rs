//! Fórmulas de puntuación centralizadas. Ningún otro módulo las duplica.
//!
//! * Precisión: media ponderada por juicio (osu!mania-like): Marvelous/Perfect 100 %,
//!   Great 66,7 %, Good 33,3 %, Bad 16,7 %, Miss 0 %.
//! * Puntuación: 1 000 000 máx., repartida por nota según peso del juicio; un combo
//!   alto no multiplica, solo se registra (lectura limpia y comparable entre canciones).
//! * Fin de partida: vida (0..=100) con ganancia/pérdida por juicio; llega a 0 = fallo.

use crate::time::Micros;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Judgement { Marvelous, Perfect, Great, Good, Bad, Miss }

impl Judgement {
    pub const ALL: [Judgement; 6] = [Self::Marvelous, Self::Perfect, Self::Great, Self::Good, Self::Bad, Self::Miss];
    pub fn index(self) -> usize { Self::ALL.iter().position(|j| *j == self).unwrap() }
    pub fn accuracy_weight(self) -> f64 {
        match self {
            Self::Marvelous | Self::Perfect => 1.0,
            Self::Great => 2.0 / 3.0,
            Self::Good => 1.0 / 3.0,
            Self::Bad => 1.0 / 6.0,
            Self::Miss => 0.0,
        }
    }
    pub fn breaks_combo(self) -> bool { matches!(self, Self::Bad | Self::Miss) }
    fn life_delta(self) -> f64 {
        match self {
            Self::Marvelous | Self::Perfect => 0.5,
            Self::Great => 0.25,
            Self::Good => 0.0,
            Self::Bad => -2.0,
            Self::Miss => -5.0,
        }
    }
    pub fn label(self) -> &'static str {
        ["Marvelous", "Perfect", "Great", "Good", "Bad", "Miss"][self.index()]
    }
}

/// Ventanas simétricas en ms (error absoluto máximo para cada categoría).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct HitWindows {
    pub marvelous_ms: f64,
    pub perfect_ms: f64,
    pub great_ms: f64,
    pub good_ms: f64,
    pub bad_ms: f64,
}

impl Default for HitWindows {
    fn default() -> Self {
        Self { marvelous_ms: 18.0, perfect_ms: 43.0, great_ms: 76.0, good_ms: 106.0, bad_ms: 127.0 }
    }
}

impl HitWindows {
    /// Ventana más ancha: fuera de ella una pulsación no puntúa y la nota queda como Miss.
    pub fn miss_after(&self) -> Micros { Micros::from_ms(self.bad_ms) }
    pub fn classify(&self, abs_error_ms: f64) -> Option<Judgement> {
        let e = abs_error_ms;
        if e <= self.marvelous_ms { Some(Judgement::Marvelous) }
        else if e <= self.perfect_ms { Some(Judgement::Perfect) }
        else if e <= self.great_ms { Some(Judgement::Great) }
        else if e <= self.good_ms { Some(Judgement::Good) }
        else if e <= self.bad_ms { Some(Judgement::Bad) }
        else { None }
    }
    /// Validación de configuración: crecientes y positivas.
    pub fn is_valid(&self) -> bool {
        let v = [self.marvelous_ms, self.perfect_ms, self.great_ms, self.good_ms, self.bad_ms];
        v[0] > 0.0 && v.windows(2).all(|w| w[0] < w[1]) && v[4] <= 500.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreState {
    pub total_judgements: u32,
    pub counts: [u32; 6],
    pub combo: u32,
    pub max_combo: u32,
    pub life: f64,
    pub failed: bool,
    /// Errores firmados (ms, negativo = pulsación anticipada) de cada acierto.
    pub errors_ms: Vec<f32>,
    weight_sum: f64,
}

impl ScoreState {
    /// `total` = número de juicios que generará el mapa (notas + finales de hold).
    pub fn new(total: u32) -> Self {
        Self { total_judgements: total, counts: [0; 6], combo: 0, max_combo: 0, life: 100.0,
               failed: false, errors_ms: Vec::new(), weight_sum: 0.0 }
    }
    pub fn register(&mut self, j: Judgement, signed_error_ms: Option<f64>) {
        self.counts[j.index()] += 1;
        self.weight_sum += j.accuracy_weight();
        if j.breaks_combo() { self.combo = 0 } else { self.combo += 1; self.max_combo = self.max_combo.max(self.combo) }
        self.life = (self.life + j.life_delta()).clamp(0.0, 100.0);
        if self.life <= 0.0 { self.failed = true }
        if let (Some(e), false) = (signed_error_ms, j == Judgement::Miss) { self.errors_ms.push(e as f32) }
    }
    pub fn judged(&self) -> u32 { self.counts.iter().sum() }
    /// Precisión sobre lo ya juzgado (0..=100); 100 si aún no hay juicios.
    pub fn accuracy(&self) -> f64 {
        let n = self.judged();
        if n == 0 { 100.0 } else { 100.0 * self.weight_sum / n as f64 }
    }
    /// Puntuación 0..=1_000_000 proporcional al peso acumulado sobre el total del mapa.
    pub fn score(&self) -> u32 {
        if self.total_judgements == 0 { return 0 }
        (1_000_000.0 * self.weight_sum / self.total_judgements as f64).round() as u32
    }
    pub fn mean_error_ms(&self) -> Option<f64> {
        if self.errors_ms.is_empty() { None }
        else { Some(self.errors_ms.iter().map(|e| *e as f64).sum::<f64>() / self.errors_ms.len() as f64) }
    }
    pub fn grade(&self) -> &'static str { grade_for(self.accuracy()) }
}

/// Nota (S…D) para una precisión 0..=100.
pub fn grade_for(accuracy: f64) -> &'static str {
    match accuracy { a if a >= 98.0 => "S", a if a >= 95.0 => "A", a if a >= 90.0 => "B", a if a >= 80.0 => "C", _ => "D" }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_classify_boundaries() {
        let w = HitWindows::default();
        assert_eq!(w.classify(0.0), Some(Judgement::Marvelous));
        assert_eq!(w.classify(18.0), Some(Judgement::Marvelous));
        assert_eq!(w.classify(18.1), Some(Judgement::Perfect));
        assert_eq!(w.classify(127.0), Some(Judgement::Bad));
        assert_eq!(w.classify(127.1), None);
        assert!(w.is_valid());
    }
    #[test]
    fn perfect_run_scores_max() {
        let mut s = ScoreState::new(10);
        for _ in 0..10 { s.register(Judgement::Marvelous, Some(0.0)) }
        assert_eq!(s.score(), 1_000_000);
        assert_eq!(s.accuracy(), 100.0);
        assert_eq!(s.max_combo, 10);
    }
    #[test]
    fn miss_breaks_combo_and_costs_life() {
        let mut s = ScoreState::new(4);
        s.register(Judgement::Perfect, Some(-3.0));
        s.register(Judgement::Perfect, Some(5.0));
        s.register(Judgement::Miss, None);
        s.register(Judgement::Great, Some(60.0));
        assert_eq!((s.combo, s.max_combo), (1, 2));
        assert!((s.accuracy() - 100.0 * (2.0 + 2.0 / 3.0) / 4.0).abs() < 1e-9);
        assert_eq!(s.errors_ms.len(), 3);
    }
    #[test]
    fn life_depletion_fails() {
        let mut s = ScoreState::new(100);
        for _ in 0..20 { s.register(Judgement::Miss, None) }
        assert!(s.failed);
    }
    #[test]
    fn grades_follow_accuracy_thresholds() {
        assert_eq!([100.0, 98.0, 97.9, 95.0, 90.0, 80.0, 79.9, 0.0].map(grade_for), ["S", "S", "A", "A", "B", "C", "D", "D"]);
        let mut s = ScoreState::new(2);
        s.register(Judgement::Marvelous, Some(0.0));
        s.register(Judgement::Miss, None);
        assert_eq!(s.grade(), "D");
    }
    #[test]
    fn invalid_windows_detected() {
        let mut w = HitWindows::default();
        w.great_ms = 10.0;
        assert!(!w.is_valid());
    }
}
