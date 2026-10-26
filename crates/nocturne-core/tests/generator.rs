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
    println!("notas E/N/H/X/M/E = {counts:?}; nps = {:?}", maps.iter().map(|m| (m.notes_per_second() * 10.0).round() / 10.0).collect::<Vec<_>>());
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
        // Dos notas a la vez como mucho, salvo las manos de tres de Master y Extreme.
        let max = if matches!(m.difficulty, Difficulty::Master | Difficulty::Extreme) { 3 } else { 2 };
        assert!(times.values().all(|c| *c <= max), "{:?}", m.difficulty);
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

// --- Niveles altos (Master y Extreme) ---------------------------------------------------------

/// 192 pulsos a 128 BPM: bombo y caja, hi-hat a corcheas y *shaker* a semicorcheas mucho más suave
/// (justo el tipo de golpe que el umbral adaptativo marca como «débil»), más un arpegio.
fn dense_song() -> Analysis {
    let bpm = 128.0; let spb = 60.0 / bpm;
    let mut t = Track::silence(44100, 2.0 + 192.0 * spb + 3.0);
    for b in 0..192 {
        let at = 1.0 + b as f64 * spb;
        if b % 2 == 0 { t.hit(at, 0.9, 55.0, 130.0, b) } else { t.hit(at, 0.8, 0.0, 80.0, b + 7) }
        t.hit(at + spb / 2.0, 0.22, 0.0, 22.0, b + 900);
        t.hit(at + spb * 0.25, 0.12, 0.0, 15.0, b + 2000);
        t.hit(at + spb * 0.75, 0.12, 0.0, 15.0, b + 3000);
        if b % 4 == 3 { t.tone(at + spb * 0.5, spb * 0.3, 330.0 + (b % 8) as f32 * 40.0, 0.2) }
    }
    t.normalize_peak(0.9);
    let a = DecodedAudio { samples: t.samples, sample_rate: 44100, channels: 1 };
    analyze(&a, &AnalysisParams::default(), &AtomicBool::new(false), &mut |_, _| {}).unwrap()
}

fn max_in_one_second(m: &Beatmap) -> usize {
    m.notes.iter().map(|n| m.notes.iter().filter(|o| o.time >= n.time && o.time < n.time + nocturne_core::Micros::from_secs(1.0)).count()).max().unwrap_or(0)
}

#[test]
fn the_ladder_reaches_competitive_density() {
    let an = dense_song();
    let maps = generate_all(&an, &meta(), &GenOptions::default());
    assert_eq!(maps.len(), 6);
    for m in &maps {
        m.validate().unwrap();
        println!("{:>7}: {:4} notas, {:5.1} nps, pico {:2}/s, nivel {:5.1}, parámetros: {}", m.difficulty.label(), m.notes.len(), m.notes_per_second(), max_in_one_second(m), m.rating(), m.parameters);
    }
    let by = |d: Difficulty| maps.iter().find(|m| m.difficulty == d).unwrap();
    let ratings: Vec<f64> = maps.iter().map(|m| m.rating()).collect();
    assert!(ratings.windows(2).all(|w| w[0] < w[1]), "los niveles deben crecer estrictamente: {ratings:?}");
    assert!(by(Difficulty::Expert).notes_per_second() >= 5.0, "Expert antes daba ~3 notas/s en música así");
    assert!(by(Difficulty::Master).notes_per_second() >= 8.0, "{}", by(Difficulty::Master).notes_per_second());
    assert!(by(Difficulty::Extreme).notes_per_second() >= 11.0, "{}", by(Difficulty::Extreme).notes_per_second());
    assert!(by(Difficulty::Extreme).rating() >= 20.0, "{}", by(Difficulty::Extreme).rating());
    assert!(by(Difficulty::Extreme).rating() > 2.0 * by(Difficulty::Hard).rating());
}

#[test]
fn top_tiers_use_weak_hits_fill_and_hands() {
    let an = dense_song();
    let m = generate(&an, Difficulty::Extreme, &meta(), &GenOptions::default());
    let mut at = std::collections::HashMap::new();
    for n in &m.notes { *at.entry(n.time).or_insert(0usize) += 1 }
    let triples = at.values().filter(|c| **c == 3).count();
    let jumps = at.values().filter(|c| **c == 2).count();
    println!("Extreme: {jumps} saltos, {triples} manos, parámetros {}", m.parameters);
    assert!(m.parameters.contains("fill="), "música con tempo claro y activa: se rellena la cuadrícula");
    assert!(jumps > 40 && triples > 0, "{jumps} saltos, {triples} manos");
    // Mucho más que los golpes que el analizador aceptó por sí solo.
    assert!(m.notes.len() > 2 * an.stats.accepted, "{} notas vs {} golpes aceptados", m.notes.len(), an.stats.accepted);
    let easy = generate(&an, Difficulty::Easy, &meta(), &GenOptions::default());
    assert!(easy.notes.len() * 8 < m.notes.len(), "la distancia entre extremos de la escalera debe ser enorme");
}

#[test]
fn density_is_capped_and_scales_with_the_setting() {
    let an = dense_song();
    let at = |scale: f32| generate(&an, Difficulty::Extreme, &meta(), &GenOptions { holds: true, density_scale: scale });
    let (low, normal, high) = (at(0.5), at(1.0), at(2.0));
    println!("pico por segundo: {} / {} / {}", max_in_one_second(&low), max_in_one_second(&normal), max_in_one_second(&high));
    assert!(low.notes.len() < normal.notes.len() && normal.notes.len() <= high.notes.len());
    // Tope exacto: eventos por segundo × 1,3 de margen para los acordes, en CUALQUIER ventana de 1 s
    // (el evento en sí cuenta una vez, así que el acorde que se le añade ya está dentro del margen).
    assert!(max_in_one_second(&low) as f32 <= 20.0 * 0.5 * 1.3 + 1.0, "{}", max_in_one_second(&low));
    assert!(max_in_one_second(&normal) as f32 <= 20.0 * 1.3 + 1.0, "{}", max_in_one_second(&normal));
    for m in [&low, &normal, &high] { m.validate().unwrap() }
}

#[test]
fn no_fill_when_the_tempo_does_not_explain_the_audio() {
    // Golpes a intervalos irregulares (sin pulso): no hay cuadrícula con la que rellenar.
    let mut rng = Rng::new(42);
    let mut t = Track::silence(44100, 45.0);
    let mut at = 1.0;
    while at < 42.0 {
        t.hit(at, 0.8, 0.0, 60.0, (at * 1000.0) as u64);
        at += 0.11 + (rng.next_f32() as f64 + 1.0) * 0.13;
    }
    t.normalize_peak(0.9);
    let a = DecodedAudio { samples: t.samples, sample_rate: 44100, channels: 1 };
    let an = analyze(&a, &AnalysisParams::default(), &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    let m = generate(&an, Difficulty::Extreme, &meta(), &GenOptions::default());
    println!("irregular: {} notas; {} ; {:?}", m.notes.len(), m.parameters, m.repairs);
    m.validate().unwrap();
    assert!(!m.parameters.contains("fill="), "no se inventa una cuadrícula que no existe");
}

#[test]
fn the_top_tiers_stay_playable_for_a_perfect_player() {
    let an = dense_song();
    for d in [Difficulty::Master, Difficulty::Extreme] {
        let m = generate(&an, d, &meta(), &GenOptions::default());
        let mut j = nocturne_core::gameplay::Judge::new(&m, Default::default());
        let mut notes = m.notes.clone();
        notes.sort_by_key(|n| (n.time, n.lane));
        for n in &notes { j.press(n.lane, n.time); if let NoteKind::Hold { end } = n.kind { j.release(n.lane, end); } j.advance(n.end()); }
        j.advance(m.duration);
        assert!(j.is_finished(), "{d:?}");
        assert_eq!(j.score.score(), 1_000_000, "{d:?}: un jugador perfecto debe poder acertarlo todo");
    }
}
