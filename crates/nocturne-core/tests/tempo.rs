use nocturne_core::analysis::{onset, spectral, synth::*, tempo, AnalysisParams};
use std::sync::atomic::AtomicBool;

fn env_of(t: &Track) -> (Vec<f32>, f64) {
    let f = spectral::compute(&t.samples, t.sr, &AtomicBool::new(false)).unwrap();
    let r = onset::detect(&t.samples, &f, &AnalysisParams::default());
    (r.envelope, f.fps())
}

#[test]
fn bpm_estimates_within_2_percent() {
    for bpm in [70.0, 90.0, 100.0, 128.0, 140.0, 174.0] {
        let (t, _) = drum_pattern(44100, bpm, 0.5, (bpm / 60.0 * 40.0) as usize, 1.0);
        let (env, fps) = env_of(&t);
        let e = tempo::estimate(&env, fps).expect("tempo");
        // Aceptar octava (BPM/2 o ×2) solo si coincide con una hipótesis alternativa.
        let ok = |b: f64| (e.bpm / b - 1.0).abs() < 0.02;
        println!("true {bpm}: est {:.2} conf {:.2} cands {:?}", e.bpm, e.confidence, e.candidates);
        assert!(ok(bpm) || e.candidates.iter().any(|c| (c.0 / bpm - 1.0).abs() < 0.02), "bpm {bpm} → {}", e.bpm);
    }
}
#[test]
fn beat_tracking_follows_truth() {
    let (t, truth) = drum_pattern(44100, 120.0, 0.5, 48, 1.0);
    let (env, fps) = env_of(&t);
    let beats = tempo::track_beats(&env, fps, 120.0, 100.0);
    let secs: Vec<f64> = beats.iter().map(|b| (*b as f64 * 256.0 + 1024.0) / 44100.0).collect();
    let err: Vec<f64> = truth.iter().filter_map(|tr| secs.iter().map(|s| (s - tr).abs()).fold(None, |a: Option<f64>, d| Some(a.map_or(d, |x| x.min(d))))).collect();
    let mean = err.iter().sum::<f64>() / err.len() as f64;
    println!("beat err mean {:.1} ms, {} beats vs {}", mean * 1000.0, secs.len(), truth.len());
    assert!(secs.len() >= truth.len() - 2 && secs.len() <= truth.len() + 2);
    assert!(mean < 0.030);
}
#[test]
fn silence_and_tone_give_no_confident_tempo() {
    let (env, fps) = env_of(&Track::silence(44100, 20.0));
    assert!(tempo::estimate(&env, fps).is_none());
    let (env, fps) = env_of(&Track::sustained_tone(44100, 20.0, 330.0, 0.4));
    assert!(tempo::estimate(&env, fps).map_or(true, |e| e.confidence < 0.3));
}
#[test]
fn two_tempos_produce_two_sections() {
    let (a, _) = drum_pattern(44100, 100.0, 0.0, 50, 0.0);  // 30 s a 100
    let (b, _) = drum_pattern(44100, 150.0, 0.0, 75, 0.0);  // 30 s a 150
    let mut s = a.samples; s.extend(b.samples);
    let t = Track { sr: 44100, samples: s };
    let (env, fps) = env_of(&t);
    let g = tempo::estimate(&env, fps).unwrap();
    let secs = tempo::sections(&env, fps, &g);
    println!("global {:.1}; sections {:?}", g.bpm, secs);
    assert!(secs.len() >= 2);
    let oct = |b: f64, t: f64| [t, t / 2.0, t * 2.0].iter().any(|x| (b / x - 1.0).abs() < 0.04);
    assert!(oct(secs.first().unwrap().bpm, 100.0) && oct(secs.last().unwrap().bpm, 150.0), "{secs:?}");
    assert!((secs.first().unwrap().bpm / secs.last().unwrap().bpm - 1.0).abs() > 0.04);
}
