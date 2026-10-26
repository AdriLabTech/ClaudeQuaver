//! Formato de mapa versionado (JSON). Ver `docs/BEATMAP_FORMAT.md`.

pub mod generator;

use crate::{Error, Micros, Result};
use serde::{Deserialize, Serialize};

pub const FORMAT_VERSION: u32 = 1;
pub const LANES: u8 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Difficulty { Easy, Normal, Hard, Expert, Master, Extreme }

impl Difficulty {
    pub const ALL: [Difficulty; 6] = [Self::Easy, Self::Normal, Self::Hard, Self::Expert, Self::Master, Self::Extreme];
    pub fn label(self) -> &'static str { ["Easy", "Normal", "Hard", "Expert", "Master", "Extreme"][self as usize] }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoteKind {
    Tap,
    /// Nota larga: `end` es el instante de liberación.
    Hold { end: Micros },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    /// Estable dentro de un mapa; no se reutiliza al borrar.
    pub id: u32,
    pub time: Micros,
    pub lane: u8,
    pub kind: NoteKind,
    /// Heurística 0..1 del evento de origen (no es una probabilidad calibrada).
    pub confidence: f32,
    pub pattern: Option<u32>,
    /// `true` si la nota fue añadida o movida a mano en el editor.
    #[serde(default)]
    pub manual: bool,
}

impl Note {
    pub fn end(&self) -> Micros {
        match self.kind { NoteKind::Tap => self.time, NoteKind::Hold { end } => end }
    }
    pub fn is_hold(&self) -> bool { matches!(self.kind, NoteKind::Hold { .. }) }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MusicInfo {
    pub bpm: Option<f64>,
    pub bpm_confidence: f32,
    /// Hipótesis alternativas (BPM/2, BPM×2, …) con su puntuación.
    pub bpm_candidates: Vec<(f64, f32)>,
    pub first_beat: Option<Micros>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Beatmap {
    pub format_version: u32,
    pub generator_version: String,
    pub title: String,
    pub artist: String,
    /// Huella del audio (FNV-1a 64 de las muestras decodificadas, hex).
    pub audio_fingerprint: String,
    pub duration: Micros,
    pub music: MusicInfo,
    pub difficulty: Difficulty,
    /// Parámetros del analizador/generador serializados como texto legible.
    pub parameters: String,
    pub notes: Vec<Note>,
    /// Offset específico de la canción en ms (positivo = notas más tarde).
    #[serde(default)]
    pub song_offset_ms: f64,
    #[serde(default)]
    pub edited_manually: bool,
    #[serde(default)]
    pub repairs: Vec<String>,
}

impl Beatmap {
    pub fn total_judgements(&self) -> u32 {
        self.notes.iter().map(|n| if n.is_hold() { 2 } else { 1 }).sum()
    }
    pub fn notes_per_second(&self) -> f64 {
        let s = self.duration.as_secs();
        if s <= 0.0 { 0.0 } else { self.notes.len() as f64 / s }
    }

    /// Nivel numérico para comparar mapas entre canciones y dificultades: las notas por segundo de
    /// los pasajes más densos (ventanas de 2 s: 70 % la media del 15 % más denso, 30 % el pico),
    /// aumentadas por la proporción de acordes (varias notas a la vez) y de repeticiones rápidas en
    /// un mismo carril. Un mapa sin notas vale 0. No depende de la duración de la canción.
    pub fn rating(&self) -> f64 {
        if self.notes.is_empty() || self.duration <= Micros::ZERO { return 0.0 }
        const W: i64 = 2_000_000;
        let windows = (self.duration.0 / 1_000_000).max(1) as usize;
        let mut counts = vec![0u32; windows];
        for n in &self.notes {
            // Cada nota cuenta en las dos ventanas de 2 s (desplazadas 1 s) que la contienen.
            let k = (n.time.0 / 1_000_000) as usize;
            for w in k.saturating_sub(1)..=k { if w < windows && n.time.0 >= w as i64 * 1_000_000 && n.time.0 < w as i64 * 1_000_000 + W { counts[w] += 1 } }
        }
        counts.sort_unstable_by(|a, b| b.cmp(a));
        let top = ((counts.len() as f64 * 0.15).ceil() as usize).clamp(1, counts.len());
        let top_mean = counts[..top].iter().map(|c| *c as f64).sum::<f64>() / top as f64 / 2.0;
        let peak = counts[0] as f64 / 2.0;
        let total = self.notes.len() as f64;
        let mut sorted: Vec<&Note> = self.notes.iter().collect();
        sorted.sort_by_key(|n| (n.time, n.lane));
        let chords = sorted.windows(2).filter(|w| w[0].time == w[1].time).count() as f64;
        let mut last = [Micros(i64::MIN / 2); LANES as usize];
        let mut jacks = 0.0;
        for n in &sorted {
            if (n.time - last[n.lane as usize]).0 < 160_000 { jacks += 1.0 }
            last[n.lane as usize] = n.end();
        }
        let base = 0.7 * top_mean + 0.3 * peak;
        (base * (1.0 + 0.6 * (chords / total).min(1.0) + 0.8 * (jacks / total).min(1.0)) * 10.0).round() / 10.0
    }

    /// Comprueba invariantes. No modifica nada.
    pub fn validate(&self) -> Result<()> {
        let bad = |m: String| Err(Error::InvalidBeatmap(m));
        let mut ids = std::collections::HashSet::new();
        let mut last_by_lane = [Micros(i64::MIN); LANES as usize];
        let mut notes: Vec<&Note> = self.notes.iter().collect();
        notes.sort_by_key(|n| (n.time, n.lane));
        for n in notes {
            if n.lane >= LANES { return bad(format!("nota {} en carril {} fuera de rango", n.id, n.lane)) }
            if n.time < Micros::ZERO { return bad(format!("nota {} con tiempo negativo", n.id)) }
            if n.end() > self.duration { return bad(format!("nota {} más allá del final de la canción", n.id)) }
            if !ids.insert(n.id) { return bad(format!("id {} duplicado", n.id)) }
            if let NoteKind::Hold { end } = n.kind {
                if end <= n.time { return bad(format!("hold {} con duración no positiva", n.id)) }
            }
            if n.time <= last_by_lane[n.lane as usize] { return bad(format!("nota {} solapada en carril {}", n.id, n.lane)) }
            last_by_lane[n.lane as usize] = n.end();
        }
        Ok(())
    }

    pub fn to_json(&self) -> Result<String> { Ok(serde_json::to_string_pretty(self)?) }

    pub fn from_json(s: &str) -> Result<Self> {
        #[derive(Deserialize)]
        struct V { format_version: u32 }
        let v: V = serde_json::from_str(s)?;
        if v.format_version != FORMAT_VERSION {
            return Err(Error::IncompatibleVersion { found: v.format_version, supported: FORMAT_VERSION });
        }
        let m: Beatmap = serde_json::from_str(s)?;
        m.validate()?;
        Ok(m)
    }
}

/// FNV-1a 64 sobre muestras cuantizadas a i16: barata y estable entre ejecuciones.
pub fn fingerprint(samples: &[f32]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for s in samples {
        let q = (s.clamp(-1.0, 1.0) * 32767.0) as i16 as u16;
        for b in q.to_le_bytes() { h ^= b as u64; h = h.wrapping_mul(0x100000001b3) }
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn sample() -> Beatmap {
        Beatmap {
            format_version: FORMAT_VERSION, generator_version: "t".into(), title: "t".into(), artist: "a".into(),
            audio_fingerprint: "0".into(), duration: Micros::from_secs(10.0),
            music: MusicInfo { bpm: Some(120.0), bpm_confidence: 0.9, bpm_candidates: vec![], first_beat: None },
            difficulty: Difficulty::Easy, parameters: String::new(), song_offset_ms: 0.0,
            edited_manually: false, repairs: vec![],
            notes: vec![
                Note { id: 0, time: Micros::from_secs(1.0), lane: 0, kind: NoteKind::Tap, confidence: 1.0, pattern: None, manual: false },
                Note { id: 1, time: Micros::from_secs(2.0), lane: 1, kind: NoteKind::Hold { end: Micros::from_secs(3.0) }, confidence: 1.0, pattern: None, manual: false },
            ],
        }
    }

    #[test]
    fn roundtrip() {
        let m = sample();
        let back = Beatmap::from_json(&m.to_json().unwrap()).unwrap();
        assert_eq!(m, back);
        assert_eq!(m.total_judgements(), 3);
    }
    #[test]
    fn rejects_other_version() {
        let mut v: serde_json::Value = serde_json::to_value(sample()).unwrap();
        v["format_version"] = 99.into();
        assert!(matches!(Beatmap::from_json(&v.to_string()), Err(Error::IncompatibleVersion { found: 99, .. })));
    }
    #[test]
    fn validation_catches_problems() {
        let mut m = sample(); m.notes[0].lane = 4; assert!(m.validate().is_err());
        let mut m = sample(); m.notes[0].time = Micros(-1); assert!(m.validate().is_err());
        let mut m = sample(); m.notes[1].kind = NoteKind::Hold { end: Micros::from_secs(1.5) }; assert!(m.validate().is_err());
        let mut m = sample(); m.notes[1].id = 0; assert!(m.validate().is_err());
        let mut m = sample(); m.notes[0].time = Micros::from_secs(11.0); assert!(m.validate().is_err());
    }
    #[test]
    fn difficulty_ladder_is_ordered_and_labelled() {
        assert_eq!(Difficulty::ALL.len(), 6);
        assert!(Difficulty::ALL.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(Difficulty::ALL.map(Difficulty::label), ["Easy", "Normal", "Hard", "Expert", "Master", "Extreme"]);
        let json = serde_json::to_string(&Difficulty::Extreme).unwrap();
        assert_eq!(serde_json::from_str::<Difficulty>(&json).unwrap(), Difficulty::Extreme);
    }

    fn dense(per_second: u32, lanes: &[u8], chord: bool, seconds: i64) -> Beatmap {
        let mut m = sample();
        m.duration = Micros::from_secs(seconds as f64);
        m.notes.clear();
        let mut id = 0;
        for s in 0..seconds {
            for k in 0..per_second {
                let t = Micros(s * 1_000_000 + (k as i64 * 1_000_000 / per_second as i64) + 1000);
                let lane = lanes[((s as usize) * per_second as usize + k as usize) % lanes.len()];
                m.notes.push(Note { id, time: t, lane, kind: NoteKind::Tap, confidence: 1.0, pattern: None, manual: false });
                id += 1;
                if chord {
                    m.notes.push(Note { id, time: t, lane: (lane + 2) % 4, kind: NoteKind::Tap, confidence: 1.0, pattern: None, manual: false });
                    id += 1;
                }
            }
        }
        m
    }

    #[test]
    fn rating_tracks_density_chords_and_jacks() {
        assert_eq!(Beatmap { notes: vec![], ..sample() }.rating(), 0.0);
        let sparse = dense(2, &[0, 1, 2, 3], false, 30);
        let busy = dense(8, &[0, 1, 2, 3], false, 30);
        let chords = dense(8, &[0, 1], true, 30); // 16 notas/s, la mitad en acorde
        let flat = dense(16, &[0, 1, 2, 3], false, 30); // 16 notas/s sueltas
        let jacks = dense(8, &[0], false, 30);
        let (a, b, c, j, f) = (sparse.rating(), busy.rating(), chords.rating(), jacks.rating(), flat.rating());
        println!("niveles: {a} {b} {c} {j} {f}");
        assert!((a - 2.0).abs() < 0.3 && (b - 8.0).abs() < 0.8, "{a} {b}");
        assert!(c > f * 1.2, "a igual densidad, los acordes suben el nivel: {c} vs {f}");
        assert!(j > b * 1.5, "las repeticiones rápidas en un carril suben el nivel: {j} vs {b}");
    }

    #[test]
    fn rating_ignores_song_length_and_cares_about_the_hardest_part() {
        let short = dense(6, &[0, 1, 2, 3], false, 20);
        let long = dense(6, &[0, 1, 2, 3], false, 120);
        assert!((short.rating() - long.rating()).abs() < 0.3);
        // Un solo pasaje denso en una canción por lo demás tranquila debe notarse.
        let mut m = dense(2, &[0, 1, 2, 3], false, 60);
        let burst = dense(12, &[0, 1, 2, 3], false, 10);
        m.notes.extend(burst.notes.iter().map(|n| Note { id: n.id + 10_000, time: n.time + Micros::from_secs(20.0), ..n.clone() }));
        m.notes.sort_by_key(|n| (n.time, n.lane));
        assert!(m.rating() > 4.0, "{}", m.rating());
    }

    #[test]
    fn fingerprint_changes_with_audio() {
        assert_ne!(fingerprint(&[0.1, 0.2]), fingerprint(&[0.1, 0.3]));
        assert_eq!(fingerprint(&[0.1, 0.2]), fingerprint(&[0.1, 0.2]));
    }
}
