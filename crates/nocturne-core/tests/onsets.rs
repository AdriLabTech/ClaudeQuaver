use nocturne_core::analysis::{onset, spectral, synth::*, AnalysisParams};
use std::sync::atomic::AtomicBool;

fn detect(t: &Track, p: &AnalysisParams) -> Vec<f64> {
    let f = spectral::compute(&t.samples, t.sr, &AtomicBool::new(false)).unwrap();
    onset::detect(&t.samples, &f, p).candidates.iter().filter(|c| c.rejected.is_none())
        .map(|c| c.sample as f64 / t.sr as f64).collect()
}
/// (errores medios, falsos positivos, falsos negativos) con tolerancia en segundos.
fn score(found: &[f64], truth: &[f64], tol: f64) -> (f64, usize, usize) {
    let (mut err, mut hit) = (0.0, 0);
    for t in truth { if let Some(f) = found.iter().min_by(|a, b| (*a - t).abs().partial_cmp(&(*b - t).abs()).unwrap()) {
        if (f - t).abs() <= tol { err += (f - t).abs(); hit += 1 } } }
    let fp = found.iter().filter(|f| !truth.iter().any(|t| (*f - t).abs() <= tol)).count();
    (if hit > 0 { err / hit as f64 } else { f64::NAN }, fp, truth.len() - hit)
}

#[test]
fn silence_has_no_onsets() {
    assert!(detect(&Track::silence(44100, 5.0), &AnalysisParams::default()).is_empty());
}
#[test]
fn single_click() {
    let mut t = Track::silence(44100, 3.0);
    t.click(1.2345, 0.8);
    let f = detect(&t, &AnalysisParams::default());
    let (e, fp, fnn) = score(&f, &[1.2345], 0.010);
    println!("single click: found={f:?} err={e:?}");
    assert_eq!((fp, fnn), (0, 0));
}
#[test]
fn regular_drums_120bpm() {
    let (mut t, truth) = drum_pattern(44100, 120.0, 0.5, 32, 1.0);
    t.normalize_peak(0.9);
    let f = detect(&t, &AnalysisParams::default());
    let (e, fp, fnn) = score(&f, &truth, 0.020);
    println!("drums: err={:.1}ms fp(incl. hi-hats)={fp} fn={fnn}", e * 1000.0);
    assert_eq!(fnn, 0);
    assert!(e < 0.010, "error medio {e}");
}
#[test]
fn sustained_tone_has_no_onsets_after_attack() {
    let t = Track::sustained_tone(44100, 5.0, 440.0, 0.5);
    let f = detect(&t, &AnalysisParams::default());
    assert!(f.iter().all(|x| *x < 0.2), "onsets espurios en tono sostenido: {f:?}");
}
#[test]
fn amplitude_invariance() {
    let (t0, truth) = drum_pattern(44100, 100.0, 0.5, 24, 1.0);
    for gain in [1.0f32, 0.1, 0.02] {
        let mut t = Track { sr: t0.sr, samples: t0.samples.clone() };
        t.samples.iter_mut().for_each(|s| *s *= gain);
        let f = detect(&t, &AnalysisParams::default());
        let (_, _, fnn) = score(&f, &truth, 0.020);
        assert!(fnn <= 1, "gain {gain}: {fnn} falsos negativos");
    }
}
#[test]
fn noise_robustness() {
    let (mut t, truth) = drum_pattern(44100, 110.0, 0.5, 32, 1.0);
    t.normalize_peak(0.5);
    t.add_noise(0.02, 7);
    let f = detect(&t, &AnalysisParams::default());
    let (_, _, fnn) = score(&f, &truth, 0.025);
    assert!(fnn <= 3, "{fnn} falsos negativos con ruido");
}
