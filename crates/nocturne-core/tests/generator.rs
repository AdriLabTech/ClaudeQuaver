use nocturne_core::analysis::{analyze, synth::*, AnalysisParams, Analysis};
use nocturne_core::beatmap::{generator::*, Beatmap, Difficulty, NoteKind};
use nocturne_core::decode::DecodedAudio;
use std::sync::atomic::AtomicBool;

fn song() -> Analysis {
    // 32 compases 4/4 a 128 BPM con bombo, caja, hi-hats a corcheas/semicorcheas y una nota larga.
    let bpm = 128.0; let spb = 60.0 / bpm;
    let mut t = Track::silence(44100, 2.0 + 128.0 * spb + 3.0);
    for b in 0..128 {
        let at = 1.0 + b as f64 * spb;
        // Cada 16 pulsos: bombo + nota larga de 3 pulsos sin percusión encima.
        if b % 16 == 0 { t.hit(at, 0.9, 55.0, 130.0, b); t.tone(at, spb * 3.0, 220.0, 0.3); continue }
        if b % 16 == 1 || b % 16 == 2 { continue }
        if b % 2 == 0 { t.hit(at, 0.9, 55.0, 130.0, b) } else { t.hit(at, 0.8, 0.0, 80.0, b + 7) }
        t.hit(at + spb / 2.0, 0.35, 0.0, 25.0, b + 900);
        if b % 4 == 3 { t.hit(at + spb * 0.75, 0.3, 0.0, 20.0, b + 1900) }
    }
    t.normalize_peak(0.9);
    let a = DecodedAudio { samples: t.samples, sample_rate: 44100, channels: 1 };
    analyze(&a, &AnalysisParams::default(), &AtomicBool::new(false), &mut |_, _| {}).unwrap()
}
fn meta() -> Meta { Meta { title: "t".into(), artist: "a".into(), fingerprint: "00ff00ff00ff00ff".into() } }

#[test]
fn all_difficulties_valid_and_ordered() {
    let an = song();
    let maps = generate_all(&an, &meta(), &GenOptions::default());
    let counts: Vec<usize> = maps.iter().map(|m| m.notes.len()).collect();
    println!("notas E/N/H/X = {counts:?}; nps = {:?}", maps.iter().map(|m| (m.notes_per_second() * 10.0).round() / 10.0).collect::<Vec<_>>());
    for m in &maps { m.validate().unwrap(); assert!(!m.notes.is_empty(), "{:?} vacío", m.difficulty) }
    assert!(counts.windows(2).all(|w| w[0] < w[1]), "no estrictamente creciente: {counts:?}");
}
#[test]
fn deterministic() {
    let an = song();
    let a = generate_all(&an, &meta(), &GenOptions::default());
    let b = generate_all(&an, &meta(), &GenOptions::default());
    assert_eq!(a, b);
}
#[test]
fn lower_difficulty_is_subset_of_higher_in_time() {
    let an = song();
    let maps = generate_all(&an, &meta(), &GenOptions::default());
    for w in maps.windows(2) {
        let hi: Vec<_> = w[1].notes.iter().map(|n| n.time).collect();
        let missing = w[0].notes.iter().filter(|n| !hi.contains(&n.time)).count();
        let frac = missing as f64 / w[0].notes.len() as f64;
        println!("{:?}⊂{:?}: {:.0}% de las notas inferiores no están en la superior", w[0].difficulty, w[1].difficulty, frac * 100.0);
        assert!(frac < 0.2, "{frac}");
    }
}
#[test]
fn no_chords_on_easy_normal_and_no_overlap() {
    let an = song();
    for m in generate_all(&an, &meta(), &GenOptions::default()) {
        let mut times = std::collections::HashMap::new();
        for n in &m.notes { *times.entry(n.time).or_insert(0) += 1 }
        let chords = times.values().filter(|c| **c > 1).count();
        if matches!(m.difficulty, Difficulty::Easy | Difficulty::Normal) { assert_eq!(chords, 0) }
        assert!(times.values().all(|c| *c <= 2));
    }
}
#[test]
fn holds_only_when_enabled_and_sustained() {
    let an = song();
    let with = generate(&an, Difficulty::Hard, &meta(), &GenOptions::default());
    let without = generate(&an, Difficulty::Hard, &meta(), &GenOptions { holds: false, ..Default::default() });
    let h = with.notes.iter().filter(|n| matches!(n.kind, NoteKind::Hold { .. })).count();
    println!("holds en Hard: {h}");
    assert!(h > 0, "la nota sostenida debería producir al menos un hold");
    assert_eq!(without.notes.iter().filter(|n| n.is_hold()).count(), 0);
    assert!(with.notes.iter().filter(|n| n.is_hold()).count() as f32 <= with.notes.len() as f32 * 0.13);
}
#[test]
fn lanes_are_varied_without_fast_jacks() {
    let an = song();
    let m = generate(&an, Difficulty::Expert, &meta(), &GenOptions::default());
    let mut per = [0usize; 4];
    for n in &m.notes { per[n.lane as usize] += 1 }
    println!("reparto de carriles {per:?}");
    assert!(per.iter().all(|c| *c as f64 > 0.1 * m.notes.len() as f64), "{per:?}");
    let mut jacks = 0;
    for w in m.notes.windows(2) { if w[0].lane == w[1].lane && (w[1].time - w[0].time).as_ms() < 140.0 { jacks += 1 } }
    assert_eq!(jacks, 0);
}
#[test]
fn saved_map_roundtrips_and_is_playable() {
    let an = song();
    let m = generate(&an, Difficulty::Normal, &meta(), &GenOptions::default());
    let back = Beatmap::from_json(&m.to_json().unwrap()).unwrap();
    assert_eq!(m, back);
    // Jugable: un jugador perfecto obtiene 1 000 000.
    let mut j = nocturne_core::gameplay::Judge::new(&back, Default::default());
    for n in &back.notes { j.press(n.lane, n.time); if let NoteKind::Hold { end } = n.kind { j.release(n.lane, end); } j.advance(n.end()); }
    j.advance(back.duration);
    assert!(j.is_finished());
    assert_eq!(j.score.score(), 1_000_000);
}
#[test]
fn silence_produces_empty_map_not_noise() {
    let a = DecodedAudio { samples: vec![0.0; 44100 * 10], sample_rate: 44100, channels: 1 };
    let an = analyze(&a, &AnalysisParams::default(), &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    assert!(generate(&an, Difficulty::Expert, &meta(), &GenOptions::default()).notes.is_empty());
}
