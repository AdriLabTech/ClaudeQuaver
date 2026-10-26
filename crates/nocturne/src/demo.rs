//! Mapa de demostración determinista (no es salida del analizador): 120 BPM, 4/4, 64 compases cortos.
use nocturne_core::beatmap::*;
use nocturne_core::Micros;

pub fn demo_map() -> Beatmap {
    let spb = 0.5; // 120 BPM
    let mut notes = vec![];
    let mut id = 0;
    let mut add = |t: f64, lane: u8, hold: Option<f64>| {
        notes.push(Note { id, time: Micros::from_secs(t), lane, kind: hold.map_or(NoteKind::Tap, |e| NoteKind::Hold { end: Micros::from_secs(e) }),
            confidence: 1.0, pattern: None, manual: false });
        id += 1;
    };
    let start = 2.0;
    for bar in 0..16 {
        let b = start + bar as f64 * 4.0 * spb;
        // Bombo en 1 y 3, caja en 2 y 4, corcheas alternas cada dos compases.
        add(b, 0, None);
        add(b + spb, 3, None);
        add(b + 2.0 * spb, 1, None);
        add(b + 3.0 * spb, 2, None);
        if bar % 2 == 1 { for k in 0..4 { add(b + (k as f64 + 0.5) * spb, [1u8, 2, 0, 3][k], None) } }
        if bar % 4 == 3 { add(b + 1.5 * spb, 1, Some(b + 2.5 * spb)); }
    }
    notes.sort_by_key(|n| (n.time, n.lane));
    // Reparar solapes del demo (nota dentro de un hold en el mismo carril).
    let mut fixed: Vec<Note> = vec![];
    for n in notes {
        if fixed.iter().any(|p| p.lane == n.lane && p.end() + Micros::from_ms(60.0) > n.time && p.time <= n.time) { continue }
        fixed.push(n);
    }
    for (i, n) in fixed.iter_mut().enumerate() { n.id = i as u32 }
    Beatmap {
        format_version: FORMAT_VERSION, generator_version: "demo".into(), title: "Demostración".into(), artist: "Nocturne".into(),
        audio_fingerprint: "demo".into(), duration: Micros::from_secs(start + 16.0 * 4.0 * spb + 2.0),
        music: MusicInfo { bpm: Some(120.0), bpm_confidence: 1.0, bpm_candidates: vec![(120.0, 1.0)], first_beat: Some(Micros::from_secs(start)) },
        difficulty: Difficulty::Normal, parameters: "demo".into(), notes: fixed, song_offset_ms: 0.0, edited_manually: false, repairs: vec![],
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn demo_map_is_valid() { super::demo_map().validate().unwrap(); }
}
