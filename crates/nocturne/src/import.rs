//! Importación y análisis de una canción (se ejecuta en un hilo de fondo).
//! decodificar → huella → analizar → generar las dificultades → guardar mapas JSON.

use crate::settings::Settings;
use nocturne_core::analysis::analyze;
use nocturne_core::beatmap::{fingerprint, generator::*, Beatmap, Difficulty};
use nocturne_core::decode::decode_file;
use nocturne_core::{Error, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// Extensiones que el explorador ofrece; el formato real se detecta por contenido al decodificar.
pub const AUDIO_EXTS: [&str; 9] = ["mp3", "wav", "flac", "ogg", "m4a", "aac", "mp4", "m4v", "mov"];

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

/// Borra los mapas de una canción (todos los niveles); los que no existan se ignoran.
pub fn delete_maps(maps_dir: &Path, fp: &str) {
    for d in Difficulty::ALL { let _ = std::fs::remove_file(map_path(maps_dir, fp, d)); }
}

/// Archivos de audio de una carpeta (sin recursión), ordenados por nombre.
pub fn audio_files_in(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path())
        .filter(|p| p.is_file() && is_audio_candidate(p)).collect();
    v.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    v
}

/// Datos de un mapa que muestra la biblioteca.
#[derive(Debug, Clone, PartialEq)]
pub struct MapSummary { pub notes: usize, pub holds: usize, pub nps: f64, pub rating: f64, pub edited: bool }

pub fn summarize(m: &Beatmap) -> MapSummary {
    MapSummary { notes: m.notes.len(), holds: m.notes.iter().filter(|n| n.is_hold()).count(), nps: m.notes_per_second(), rating: m.rating(), edited: m.edited_manually }
}

/// `notes`: número de notas generadas por dificultad, en el orden de `Difficulty::ALL`.
pub struct Outcome { pub bpm: Option<f64>, pub notes: Vec<usize>, pub warnings: Vec<String>, pub elapsed_ms: u128 }

/// Fase lenta: análisis y generación. `progress(fracción, etapa)`. Con `keep_edited`, un mapa que ya
/// existe y fue editado a mano no se sobrescribe (se avisa); sirve para generar solo los niveles que faltan.
pub fn analyze_and_generate(p: &Probed, artist: &str, s: &Settings, maps_dir: &Path, keep_edited: bool, cancel: &AtomicBool,
                            progress: &mut dyn FnMut(f32, &'static str)) -> Result<Outcome> {
    let an = analyze(&p.audio, &s.analysis_params(), cancel, progress)?;
    if cancel.load(std::sync::atomic::Ordering::Relaxed) { return Err(Error::Cancelled) }
    let meta = Meta { title: p.title.clone(), artist: artist.into(), fingerprint: p.fingerprint.clone() };
    let opts = GenOptions { holds: s.generate_holds, density_scale: s.density_scale };
    let mut notes = vec![];
    let mut warnings = an.warnings.clone();
    for m in generate_all(&an, &meta, &opts) {
        if let Some(old) = keep_edited.then(|| load_map(maps_dir, &p.fingerprint, m.difficulty).ok()).flatten().filter(|o| o.edited_manually) {
            warnings.push(format!("{}: se conservan tus ediciones", m.difficulty.label()));
            notes.push(old.notes.len());
            continue;
        }
        notes.push(m.notes.len());
        save_map(maps_dir, &m)?;
    }
    if notes.iter().all(|n| *n == 0) { warnings.push("no se generó ninguna nota: la pista no tiene ritmo detectable".into()) }
    Ok(Outcome { bpm: an.tempo.as_ref().map(|t| t.bpm), notes, warnings, elapsed_ms: an.stats.elapsed_ms })
}

/// Importación síncrona para la línea de órdenes (`--play`): reutiliza la canción si ya está lista
/// y sus mapas existen; si no, la analiza aquí mismo. Devuelve la canción de la base de datos.
pub fn import_blocking(db: &crate::db::Db, path: &Path, s: &Settings, maps_dir: &Path) -> Result<crate::db::Song> {
    use crate::db::Status;
    let cancel = AtomicBool::new(false);
    let path = &std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let p = probe(path, &cancel)?;
    let existing = db.song_by_fingerprint(&p.fingerprint).ok().flatten();
    if let Some(song) = &existing {
        if song.status == Status::Ready && Difficulty::ALL.iter().all(|d| map_path(maps_dir, &p.fingerprint, *d).exists()) { return Ok(song.clone()) }
    }
    let id = match existing {
        Some(song) => { let _ = db.update_path(song.id, &path.to_string_lossy()); song.id }
        None => db.add_song(&p.fingerprint, &path.to_string_lossy(), &p.title, "", p.audio.duration().0 / 1000)
            .map_err(|e| Error::InvalidBeatmap(format!("base de datos: {e}")))?,
    };
    match analyze_and_generate(&p, "", s, maps_dir, true, &cancel, &mut |_, _| {}) {
        Ok(out) => { let _ = db.set_status(id, Status::Ready, None, out.bpm); }
        Err(e) => { let _ = db.set_status(id, Status::Failed, Some(&e.to_string()), None); return Err(e) }
    }
    db.song(id).ok().flatten().ok_or_else(|| Error::InvalidBeatmap("la canción no quedó registrada".into()))
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
        let out = analyze_and_generate(&p, "", &Settings::default(), &dir.join("maps"), false, &cancel, &mut |_, _| {}).unwrap();
        assert_eq!(out.notes.len(), Difficulty::ALL.len());
        assert!(out.notes.windows(2).all(|w| w[0] <= w[1]) && out.notes[0] < *out.notes.last().unwrap(), "{:?}", out.notes);
        for d in Difficulty::ALL {
            let m = load_map(&dir.join("maps"), &p.fingerprint, d).unwrap();
            assert_eq!(m.difficulty, d);
        }
        // Mismo audio → misma huella (detección de duplicados).
        assert_eq!(probe(&wav, &cancel).unwrap().fingerprint, p.fingerprint);
    }
    #[test]
    fn regenerating_can_keep_manual_edits() {
        let dir = std::env::temp_dir().join(format!("nocturne-imp3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (mut t, _) = drum_pattern(44100, 120.0, 1.0, 48, 2.0);
        t.normalize_peak(0.9);
        let wav = dir.join("c.wav");
        write_wav(&wav, &t.samples, 44100, 1).unwrap();
        let cancel = AtomicBool::new(false);
        let p = probe(&wav, &cancel).unwrap();
        let maps = dir.join("maps");
        analyze_and_generate(&p, "", &Settings::default(), &maps, false, &cancel, &mut |_, _| {}).unwrap();
        // El jugador edita el mapa Hard a mano.
        let mut hard = load_map(&maps, &p.fingerprint, Difficulty::Hard).unwrap();
        hard.edited_manually = true;
        hard.notes.truncate(3);
        save_map(&maps, &hard).unwrap();
        // Regenerar «solo lo que falta» respeta la edición y avisa…
        let out = analyze_and_generate(&p, "", &Settings::default(), &maps, true, &cancel, &mut |_, _| {}).unwrap();
        assert_eq!(load_map(&maps, &p.fingerprint, Difficulty::Hard).unwrap().notes.len(), 3);
        assert!(out.warnings.iter().any(|w| w.starts_with("Hard:")), "{:?}", out.warnings);
        assert_eq!(out.notes.len(), Difficulty::ALL.len(), "el orden de `notes` sigue el de Difficulty::ALL");
        assert_eq!(out.notes[Difficulty::Hard as usize], 3);
        // …y una reanálisis explícito (ya confirmado por el usuario) la sustituye.
        analyze_and_generate(&p, "", &Settings::default(), &maps, false, &cancel, &mut |_, _| {}).unwrap();
        assert!(load_map(&maps, &p.fingerprint, Difficulty::Hard).unwrap().notes.len() > 3);
    }
    #[test]
    fn candidates_by_extension() {
        assert!(is_audio_candidate(Path::new("a/B.MP3")));
        assert!(is_audio_candidate(Path::new("v.mp4")));
        assert!(!is_audio_candidate(Path::new("notas.txt")));
    }

    #[test]
    fn folder_listing_delete_and_blocking_import() {
        let dir = std::env::temp_dir().join(format!("nocturne-imp2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (mut t, _) = drum_pattern(44100, 128.0, 1.0, 48, 2.0);
        t.normalize_peak(0.9);
        write_wav(&dir.join("b.wav"), &t.samples, 44100, 1).unwrap();
        write_wav(&dir.join("A.WAV"), &t.samples, 44100, 1).unwrap();
        std::fs::write(dir.join("notas.txt"), "x").unwrap();
        let names: Vec<String> = audio_files_in(&dir).iter().map(|p| p.file_name().unwrap().to_string_lossy().into()).collect();
        assert_eq!(names, vec!["A.WAV", "b.wav"]);

        let db = crate::db::Db::open_memory().unwrap();
        let maps = dir.join("maps");
        let song = import_blocking(&db, &dir.join("b.wav"), &Settings::default(), &maps).unwrap();
        assert_eq!(song.status, crate::db::Status::Ready);
        assert!(song.bpm.is_some());
        let again = import_blocking(&db, &dir.join("b.wav"), &Settings::default(), &maps).unwrap();
        assert_eq!(again.id, song.id, "reutiliza la canción ya analizada");
        let m = load_map(&maps, &song.fingerprint, Difficulty::Hard).unwrap();
        let sum = summarize(&m);
        assert_eq!(sum.notes, m.notes.len());
        assert!(!sum.edited && sum.rating > 0.0);

        delete_maps(&maps, &song.fingerprint);
        assert!(load_map(&maps, &song.fingerprint, Difficulty::Hard).is_err());
        // sin mapas en disco, la importación vuelve a generarlos aunque la canción conste como lista
        import_blocking(&db, &dir.join("b.wav"), &Settings::default(), &maps).unwrap();
        assert!(load_map(&maps, &song.fingerprint, Difficulty::Easy).is_ok());
        assert!(import_blocking(&db, &dir.join("notas.txt"), &Settings::default(), &maps).is_err());
    }
}
