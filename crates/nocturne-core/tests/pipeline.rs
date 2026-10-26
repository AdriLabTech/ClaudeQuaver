use nocturne_core::analysis::{analyze, synth::*, AnalysisParams};
use nocturne_core::decode::DecodedAudio;
use std::sync::atomic::AtomicBool;

fn run(t: &Track) -> nocturne_core::analysis::Analysis {
    let a = DecodedAudio { samples: t.samples.clone(), sample_rate: t.sr, channels: 1 };
    analyze(&a, &AnalysisParams::default(), &AtomicBool::new(false), &mut |_, _| {}).unwrap()
}

#[test]
fn drum_loop_full_pipeline() {
    let (mut t, truth) = drum_pattern(44100, 120.0, 1.0, 64, 2.0);
    t.normalize_peak(0.9);
    let a = run(&t);
    let tempo = a.tempo.as_ref().unwrap();
    println!("bpm {:.2} conf {:.2} beats {} events {}/{} patterns {} elapsed {}ms warnings {:?}",
        tempo.bpm, tempo.confidence, a.beats.len(), a.stats.accepted, a.stats.candidates, a.stats.patterns, a.stats.elapsed_ms, a.warnings);
    assert!((tempo.bpm / 120.0 - 1.0).abs() < 0.02 || (tempo.bpm / 60.0 - 1.0).abs() < 0.02 || (tempo.bpm / 240.0 - 1.0).abs() < 0.02);
    // Beats alineados con los golpes reales (media < 15 ms).
    let errs: Vec<f64> = truth.iter().map(|tr| a.beats.iter().map(|b| (b.time.as_secs() - tr).abs()).fold(f64::MAX, f64::min)).collect();
    let mean = errs.iter().sum::<f64>() / errs.len() as f64;
    println!("beat→truth mean error {:.1} ms", mean * 1000.0);
    assert!(mean < 0.015, "{mean}");
    // Patrón repetitivo → pocos patrones distintos; golpes en cuadrícula.
    assert!(a.stats.patterns >= 1 && a.stats.patterns <= 4, "{}", a.stats.patterns);
    let on_grid = a.events.iter().filter(|e| e.accepted() && e.subdivision != nocturne_core::analysis::rhythm::Subdivision::OffGrid).count();
    assert!(on_grid as f64 >= 0.9 * a.stats.accepted as f64);
    for e in a.events.iter().take(3) { println!("{}", e.explain()) }
}
#[test]
fn degenerate_inputs() {
    let a = DecodedAudio { samples: vec![0.0; 44100 * 10], sample_rate: 44100, channels: 1 };
    let r = analyze(&a, &AnalysisParams::default(), &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    assert_eq!(r.stats.accepted, 0);
    assert!(!r.warnings.is_empty());
    let short = DecodedAudio { samples: vec![0.1; 4410], sample_rate: 44100, channels: 1 };
    assert!(analyze(&short, &AnalysisParams::default(), &AtomicBool::new(false), &mut |_, _| {}).is_err());
    let cancelled = DecodedAudio { samples: vec![0.1; 44100 * 10], sample_rate: 44100, channels: 1 };
    assert!(analyze(&cancelled, &AnalysisParams::default(), &AtomicBool::new(true), &mut |_, _| {}).is_err());
}
#[test]
fn stereo_out_of_phase_is_not_cancelled() {
    // Eventos distintos por canal: la mezcla mono no debe perder ninguno (se suman, no se restan).
    let mut l = Track::silence(44100, 6.0); let mut r = Track::silence(44100, 6.0);
    l.click(1.0, 0.8); r.click(2.0, 0.8); l.click(3.0, 0.8); r.click(4.0, 0.8);
    let samples: Vec<f32> = l.samples.iter().zip(&r.samples).flat_map(|(a, b)| [*a, *b]).collect();
    let a = DecodedAudio { samples, sample_rate: 44100, channels: 2 };
    let r = analyze(&a, &AnalysisParams::default(), &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    assert_eq!(r.stats.accepted, 4);
}
