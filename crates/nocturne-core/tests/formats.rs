//! Decodificación de formatos reales. Requiere `ffmpeg` solo para *crear* los archivos de
//! prueba; si no está instalado, las pruebas se omiten (el juego no depende de ffmpeg).
use nocturne_core::analysis::{analyze, synth::*, AnalysisParams};
use nocturne_core::decode::{decode_file, write_wav};
use std::{path::PathBuf, process::Command, sync::atomic::AtomicBool};

fn have_ffmpeg() -> bool { Command::new("ffmpeg").arg("-version").output().is_ok() }
fn dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("nocturne-fmt-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn encode(src: &PathBuf, out: &PathBuf, args: &[&str]) {
    let st = Command::new("ffmpeg").args(["-v", "error", "-y", "-i"]).arg(src).args(args).arg(out).status().unwrap();
    assert!(st.success(), "ffmpeg falló para {out:?}");
}

#[test]
fn every_format_decodes_and_keeps_the_rhythm() {
    if !have_ffmpeg() { eprintln!("ffmpeg no disponible: omitido"); return }
    let (mut t, truth) = drum_pattern(44100, 120.0, 1.0, 40, 1.0);
    t.normalize_peak(0.9);
    let d = dir();
    let wav = d.join("src.wav");
    write_wav(&wav, &t.samples, 44100, 1).unwrap();
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("a.mp3", vec!["-c:a", "libmp3lame", "-b:a", "192k"]),
        ("a.flac", vec!["-c:a", "flac"]),
        ("a.ogg", vec!["-c:a", "libvorbis"]),
        ("a.m4a", vec!["-c:a", "aac", "-b:a", "192k"]),
        ("b.wav", vec!["-c:a", "pcm_s16le", "-ar", "48000", "-ac", "2"]),
    ];
    for (name, args) in cases {
        let out = d.join(name);
        encode(&wav, &out, &args);
        let a = decode_file(&out, &AtomicBool::new(false)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let dur = a.duration().as_secs();
        assert!((dur - t.len_secs()).abs() < 0.1, "{name}: duración {dur} vs {}", t.len_secs());
        let an = analyze(&a, &AnalysisParams::default(), &AtomicBool::new(false), &mut |_, _| {}).unwrap();
        let bpm = an.tempo.as_ref().unwrap().bpm;
        // Offset por retardo del codec (MP3/AAC añaden padding): se reporta, no se oculta.
        let off: f64 = truth.iter().take(8).map(|tr| {
            an.beats.iter().map(|b| b.time.as_secs() - tr).min_by(|a, b| a.abs().partial_cmp(&b.abs()).unwrap()).unwrap()
        }).sum::<f64>() / 8.0;
        println!("{name:8} {} Hz {}ch  dur {dur:.2}s  bpm {bpm:.1}  offset medio de beats {:+.1} ms", a.sample_rate, a.channels, off * 1000.0);
        // Garantía de sincronía: ningún formato desplaza los beats más de 8 ms (incluye el priming AAC).
        assert!(off.abs() < 0.008, "{name}: desfase {:+.1} ms", off * 1000.0);
        assert!([120.0, 60.0, 240.0].iter().any(|b| (bpm / b - 1.0).abs() < 0.03), "{name}: bpm {bpm}");
    }
}

#[test]
fn mp4_audio_track_is_extracted_and_missing_audio_is_an_error() {
    if !have_ffmpeg() { return }
    let (mut t, _) = drum_pattern(44100, 100.0, 1.0, 30, 1.0);
    t.normalize_peak(0.9);
    let d = dir();
    let wav = d.join("v.wav");
    write_wav(&wav, &t.samples, 44100, 1).unwrap();
    let mp4 = d.join("v.mp4");
    let st = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=c=black:s=64x64:r=10:d=20", "-i"]).arg(&wav)
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"]).arg(&mp4).status().unwrap();
    assert!(st.success());
    let a = decode_file(&mp4, &AtomicBool::new(false)).expect("mp4 con vídeo + audio");
    assert!(a.duration().as_secs() > 15.0);
    let silent = d.join("nosound.mp4");
    let st = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=c=black:s=64x64:r=10:d=2", "-c:v", "libx264", "-pix_fmt", "yuv420p"])
        .arg(&silent).status().unwrap();
    assert!(st.success());
    assert!(decode_file(&silent, &AtomicBool::new(false)).is_err());
}
