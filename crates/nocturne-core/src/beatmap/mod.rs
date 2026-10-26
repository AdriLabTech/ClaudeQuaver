//! Formato de mapa versionado (JSON). Ver `docs/BEATMAP_FORMAT.md`.

pub mod generator;

use crate::{Error, Micros, Result};
use serde::{Deserialize, Serialize};

pub const FORMAT_VERSION: u32 = 1;
pub const LANES: u8 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Difficulty { Easy, Normal, Hard, Expert }

impl Difficulty {
    pub const ALL: [Difficulty; 4] = [Self::Easy, Self::Normal, Self::Hard, Self::Expert];
    pub fn label(self) -> &'static str { ["Easy", "Normal", "Hard", "Expert"][self as usize] }
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
    fn fingerprint_changes_with_audio() {
        assert_ne!(fingerprint(&[0.1, 0.2]), fingerprint(&[0.1, 0.3]));
        assert_eq!(fingerprint(&[0.1, 0.2]), fingerprint(&[0.1, 0.2]));
    }
}
