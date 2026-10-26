use super::*;
use crate::beatmap::{Beatmap, Difficulty, MusicInfo, Note, NoteKind, FORMAT_VERSION};

fn map(notes: Vec<Note>) -> Beatmap {
    Beatmap { format_version: FORMAT_VERSION, generator_version: "t".into(), title: "".into(), artist: "".into(),
        audio_fingerprint: "".into(), duration: Micros::from_secs(60.0),
        music: MusicInfo { bpm: None, bpm_confidence: 0.0, bpm_candidates: vec![], first_beat: None },
        difficulty: Difficulty::Easy, parameters: "".into(), notes, song_offset_ms: 0.0, edited_manually: false, repairs: vec![] }
}
fn tap(id: u32, ms: f64, lane: u8) -> Note {
    Note { id, time: Micros::from_ms(ms), lane, kind: NoteKind::Tap, confidence: 1.0, pattern: None, manual: false }
}
fn hold(id: u32, ms: f64, end: f64, lane: u8) -> Note {
    Note { kind: NoteKind::Hold { end: Micros::from_ms(end) }, ..tap(id, ms, lane) }
}
fn judged(e: &[Event]) -> Vec<Judgement> {
    e.iter().filter_map(|e| match e { Event::Judged { judgement, .. } | Event::HoldEnd { judgement, .. } => Some(*judgement), _ => None }).collect()
}
fn judge(notes: Vec<Note>) -> Judge { Judge::new(&map(notes), HitWindows::default()) }

#[test]
fn exact_hit_is_marvelous() {
    let mut j = judge(vec![tap(0, 1000.0, 0)]);
    let e = j.press(0, Micros::from_ms(1000.0));
    assert_eq!(judged(&e), vec![Judgement::Marvelous]);
    assert!(j.is_finished());
}
#[test]
fn early_and_late_keep_sign() {
    let mut j = judge(vec![tap(0, 1000.0, 0), tap(1, 2000.0, 0)]);
    let e = j.press(0, Micros::from_ms(960.0));
    assert!(matches!(e[0], Event::Judged { judgement: Judgement::Perfect, error_ms: Some(x), .. } if (x + 40.0).abs() < 1e-9));
    let e = j.press(0, Micros::from_ms(2050.0));
    assert!(matches!(e[0], Event::Judged { judgement: Judgement::Great, error_ms: Some(x), .. } if (x - 50.0).abs() < 1e-9));
    assert!(j.score.mean_error_ms().is_some());
}
#[test]
fn outside_window_is_ghost_and_note_survives() {
    let mut j = judge(vec![tap(0, 1000.0, 0)]);
    let e = j.press(0, Micros::from_ms(700.0));
    assert_eq!(e, vec![Event::GhostTap { lane: 0 }]);
    assert!(!j.is_finished());
    assert_eq!(j.score.judged(), 0);
}
#[test]
fn double_press_scores_once() {
    let mut j = judge(vec![tap(0, 1000.0, 0)]);
    j.press(0, Micros::from_ms(1000.0));
    let e = j.press(0, Micros::from_ms(1005.0));
    assert_eq!(e, vec![Event::GhostTap { lane: 0 }]);
    assert_eq!(j.score.judged(), 1);
}
#[test]
fn missed_note_registers_at_right_time() {
    let mut j = judge(vec![tap(0, 1000.0, 2)]);
    assert!(j.advance(Micros::from_ms(1100.0)).is_empty());
    let e = j.advance(Micros::from_ms(1128.0));
    assert_eq!(judged(&e), vec![Judgement::Miss]);
    assert_eq!(j.score.combo, 0);
}
#[test]
fn simultaneous_notes_independent_lanes() {
    let mut j = judge(vec![tap(0, 1000.0, 0), tap(1, 1000.0, 3)]);
    j.press(0, Micros::from_ms(1000.0));
    assert!(!j.is_finished());
    j.press(3, Micros::from_ms(1010.0));
    assert!(j.is_finished());
    assert_eq!(j.score.counts[Judgement::Marvelous.index()], 2);
}
#[test]
fn hold_full_cycle() {
    let mut j = judge(vec![hold(0, 1000.0, 2000.0, 1)]);
    j.press(1, Micros::from_ms(1000.0));
    assert!(!j.is_finished());
    let e = j.release(1, Micros::from_ms(2000.0));
    assert_eq!(judged(&e), vec![Judgement::Marvelous]);
    assert!(j.is_finished());
    assert_eq!(j.score.judged(), 2);
}
#[test]
fn hold_released_too_early_misses_tail() {
    let mut j = judge(vec![hold(0, 1000.0, 2000.0, 1)]);
    j.press(1, Micros::from_ms(1000.0));
    let e = j.release(1, Micros::from_ms(1500.0));
    assert_eq!(judged(&e), vec![Judgement::Miss]);
    assert_eq!(j.score.combo, 0);
}
#[test]
fn hold_kept_past_end_autocompletes() {
    let mut j = judge(vec![hold(0, 1000.0, 2000.0, 1)]);
    j.press(1, Micros::from_ms(1000.0));
    assert!(j.advance(Micros::from_ms(2100.0)).is_empty());
    let e = j.advance(Micros::from_ms(2107.0));
    assert_eq!(judged(&e), vec![Judgement::Perfect]);
}
#[test]
fn unpressed_hold_gives_two_misses() {
    let mut j = judge(vec![hold(0, 1000.0, 2000.0, 1)]);
    let e = j.advance(Micros::from_ms(1200.0));
    assert_eq!(judged(&e), vec![Judgement::Miss, Judgement::Miss]);
}
#[test]
fn pause_is_just_not_advancing() {
    // Congelar el tiempo no genera misses aunque pase "tiempo real".
    let mut j = judge(vec![tap(0, 1000.0, 0)]);
    j.advance(Micros::from_ms(900.0));
    j.advance(Micros::from_ms(900.0));
    assert_eq!(j.score.judged(), 0);
}
#[test]
fn restart_resets_state() {
    let m = map(vec![tap(0, 1000.0, 0)]);
    let mut j = Judge::new(&m, HitWindows::default());
    j.press(0, Micros::from_ms(1000.0));
    let j2 = Judge::new(&m, HitWindows::default());
    assert!(j.is_finished() && !j2.is_finished());
    assert_eq!(j2.score.judged(), 0);
}
#[test]
fn full_song_final_score() {
    let notes: Vec<_> = (0..20).map(|i| tap(i, 1000.0 + i as f64 * 250.0, (i % 4) as u8)).collect();
    let mut j = judge(notes.clone());
    for n in &notes { j.press(n.lane, n.time); j.advance(n.time) ; }
    assert!(j.is_finished());
    assert_eq!(j.score.score(), 1_000_000);
    assert_eq!(j.score.max_combo, 20);
}
