//! Herramienta de línea de órdenes: analiza un archivo, imprime el diagnóstico y
//! escribe un mapa por dificultad. Uso: nocturne-analyze <audio> [directorio-salida]
use nocturne_core::analysis::{analyze, AnalysisParams};
use nocturne_core::beatmap::{fingerprint, generator::*};
use nocturne_core::decode::decode_file;
use std::{path::Path, sync::atomic::AtomicBool};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(input) = args.get(1) else { eprintln!("uso: nocturne-analyze <audio> [salida]"); std::process::exit(2) };
    let cancel = AtomicBool::new(false);
    let audio = decode_file(Path::new(input), &cancel).unwrap_or_else(|e| { eprintln!("error: {e}"); std::process::exit(1) });
    println!("audio: {} Hz, {} canales, {:.1} s", audio.sample_rate, audio.channels, audio.duration().as_secs());
    let an = analyze(&audio, &AnalysisParams::default(), &cancel, &mut |_, _| {}).unwrap_or_else(|e| { eprintln!("error: {e}"); std::process::exit(1) });
    match &an.tempo {
        Some(t) => println!("tempo: {:.1} BPM (confianza {:.2}); alternativas {:?}", t.bpm, t.confidence, t.candidates.iter().map(|c| c.0.round()).collect::<Vec<_>>()),
        None => println!("tempo: sin estimación fiable"),
    }
    println!("secciones de tempo: {}", an.tempo_sections.iter().map(|s| format!("{:.0}-{:.0}s@{:.0}", s.start_s, s.end_s, s.bpm)).collect::<Vec<_>>().join(", "));
    println!("{:?}", an.stats);
    for w in &an.warnings { println!("aviso: {w}") }
    let stem = Path::new(input).file_stem().and_then(|s| s.to_str()).unwrap_or("cancion").to_string();
    let meta = Meta { title: stem.clone(), artist: String::new(), fingerprint: fingerprint(&audio.samples) };
    for m in generate_all(&an, &meta, &GenOptions::default()) {
        println!("{:>7}: {:4} notas, {:.1} nps, nivel {:.1}, {} holds, {} acordes", m.difficulty.label(), m.notes.len(), m.notes_per_second(), m.rating(), m.notes.iter().filter(|n| n.is_hold()).count(), { let mut t = std::collections::HashMap::new(); for n in &m.notes { *t.entry(n.time).or_insert(0usize) += 1 } t.values().filter(|c| **c > 1).count() });
        if let Some(out) = args.get(2) {
            std::fs::create_dir_all(out).unwrap();
            std::fs::write(Path::new(out).join(format!("{stem}.{}.json", m.difficulty.label().to_lowercase())), m.to_json().unwrap()).unwrap();
        }
    }
}
