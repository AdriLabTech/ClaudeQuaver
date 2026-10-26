//! Importación y análisis de una canción (se ejecuta en un hilo de fondo).
//! decodificar → huella → analizar → generar 4 dificultades → guardar mapas JSON.

use crate::settings::Settings;
use nocturne_core::analysis::analyze;
use nocturne_core::beatmap::{fingerprint, generator::*, Beatmap, Difficulty};
use nocturne_core::decode::decode_file;
use nocturne_core::{Error, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

pub const AUDIO_EXTS: [&str; 10] = ["mp3", "wav", "flac", "ogg", "m4a", "aac", "mp4", "m4v", "mov", "opus"];

pub fn is_audio_candidate(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).map_or(false, |e| AUDIO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

pub fn map_path(maps_dir: &Path, fingerprint: &str, d: Difficulty) -> PathBuf {
    maps_dir.join(format!("{fingerprint}.{}.json", d.label().to_lowercase()))
}

/// Guardado atómico de un mapa (temporal + renombrado).
pub fn save_map(maps_dir: &Path, m: &Beatmap) -> Result<()> {
    std::fs::create_dir_all(maps_dir)?;
    let p = map_path(maps_dir, &m.audio_fingerprint, m.difficulty);
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, m.to_json()?)?;
    std::fs::rename(tmp, p)?;
    Ok(())
}

pub fn load_map(maps_dir: &Path, fp: &str, d: Difficulty) -> Result<Beatmap> {
    Beatmap::from_json(&std::fs::read_to_string(map_path(maps_dir, fp, d))?)
}

/// Resultado de la fase rápida (decodificar + huella): permite detectar duplicados antes de analizar.
pub struct Probed { pub audio: nocturne_core::decode::DecodedAudio, pub fingerprint: String, pub title: String }

pub fn probe(path: &Path, cancel: &AtomicBool) -> Result<Probed> {
    let audio = decode_file(path, cancel)?;
    let fingerprint = fingerprint(&audio.samples);
    let title = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Sin título").replace(['_', '.'], " ").trim().to_string();
    Ok(Probed { audio, fingerprint, title })
}

pub struct Outcome { pub bpm: Option<f64>, pub notes: [usize; 4], pub warnings: Vec<String>, pub elapsed_ms: u128 }

/// Fase lenta: análisis y generación. `progress(fracción, etapa)`.
pub fn analyze_and_generate(p: &Probed, artist: &str, s: &Settings, maps_dir: &Path, cancel: &AtomicBool,
                            progress: &mut dyn FnMut(f32, &'static str)) -> Result<Outcome> {
    let an = analyze(&p.audio, &s.analysis_params(), cancel, progress)?;
    if cancel.load(std::sync::atomic::Ordering::Relaxed) { return Err(Error::Cancelled) }
    let meta = Meta { title: p.title.clone(), artist: artist.into(), fingerprint: p.fingerprint.clone() };
    let opts = GenOptions { holds: s.generate_holds, density_scale: s.density_scale };
    let mut notes = [0; 4];
    for (i, m) in generate_all(&an, &meta, &opts).into_iter().enumerate() {
        notes[i] = m.notes.len();
        save_map(maps_dir, &m)?;
    }
    let mut warnings = an.warnings.clone();
    if notes.iter().all(|n| *n == 0) { warnings.push("no se generó ninguna nota: la pista no tiene ritmo detectable".into()) }
    Ok(Outcome { bpm: an.tempo.as_ref().map(|t| t.bpm), notes, warnings, elapsed_ms: an.stats.elapsed_ms })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nocturne_core::analysis::synth::drum_pattern;
    use nocturne_core::decode::write_wav;

    #[test]
    fn import_end_to_end_produces_four_loadable_maps() {
        let dir = std::env::temp_dir().join(format!("nocturne-imp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (mut t, _) = drum_pattern(44100, 124.0, 1.0, 64, 2.0);
        t.normalize_peak(0.9);
        let wav = dir.join("mi_cancion.wav");
        write_wav(&wav, &t.samples, 44100, 1).unwrap();
        let cancel = AtomicBool::new(false);
        let p = probe(&wav, &cancel).unwrap();
        assert_eq!(p.title, "mi cancion");
        let out = analyze_and_generate(&p, "", &Settings::default(), &dir.join("maps"), &cancel, &mut |_, _| {}).unwrap();
        assert!(out.notes.windows(2).all(|w| w[0] <= w[1]) && out.notes[0] < out.notes[3], "{:?}", out.notes);
        for d in Difficulty::ALL {
            let m = load_map(&dir.join("maps"), &p.fingerprint, d).unwrap();
            assert_eq!(m.difficulty, d);
        }
        // Mismo audio → misma huella (detección de duplicados).
        assert_eq!(probe(&wav, &cancel).unwrap().fingerprint, p.fingerprint);
    }
    #[test]
    fn candidates_by_extension() {
        assert!(is_audio_candidate(Path::new("a/B.MP3")));
        assert!(is_audio_candidate(Path::new("v.mp4")));
        assert!(!is_audio_candidate(Path::new("notas.txt")));
    }
}
