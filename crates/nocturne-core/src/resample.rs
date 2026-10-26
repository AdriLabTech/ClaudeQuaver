//! Conversión a estéreo y remuestreo (Hermite cúbico) para llevar la canción al formato del
//! dispositivo de salida *antes* de reproducir: el callback de audio solo copia muestras.

/// Entrelazado `channels` → estéreo (mono se duplica; >2 canales: se mezclan en L/R).
pub fn to_stereo(samples: &[f32], channels: usize) -> Vec<f32> {
    match channels {
        0 => vec![],
        1 => samples.iter().flat_map(|s| [*s, *s]).collect(),
        2 => samples.to_vec(),
        n => samples.chunks_exact(n).flat_map(|f| {
            let (mut l, mut r) = (0.0, 0.0);
            for (i, s) in f.iter().enumerate() { if i % 2 == 0 { l += s } else { r += s } }
            let k = (n as f32 / 2.0).ceil();
            [l / k, r / k]
        }).collect(),
    }
}

/// Remuestrea estéreo entrelazado de `from` Hz a `to` Hz con interpolación de Hermite (Catmull-Rom).
pub fn resample_stereo(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.len() < 8 { return input.to_vec() }
    let frames = input.len() / 2;
    let out_frames = (frames as u64 * to as u64 / from as u64) as usize;
    let step = from as f64 / to as f64;
    let at = |i: isize, c: usize| -> f32 { input[(i.clamp(0, frames as isize - 1) as usize) * 2 + c] };
    let mut out = Vec::with_capacity(out_frames * 2);
    for n in 0..out_frames {
        let pos = n as f64 * step;
        let i = pos.floor() as isize;
        let t = (pos - i as f64) as f32;
        for c in 0..2 {
            let (y0, y1, y2, y3) = (at(i - 1, c), at(i, c), at(i + 1, c), at(i + 2, c));
            let (a, b) = (-0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3, y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3);
            let cc = -0.5 * y0 + 0.5 * y2;
            out.push(((a * t + b) * t + cc) * t + y1);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tone(sr: u32, hz: f64, secs: f64) -> Vec<f32> {
        (0..(sr as f64 * secs) as usize).flat_map(|i| { let v = (2.0 * std::f64::consts::PI * hz * i as f64 / sr as f64).sin() as f32; [v, v] }).collect()
    }
    fn zero_crossings(x: &[f32]) -> usize { x.chunks_exact(2).map(|f| f[0]).collect::<Vec<_>>().windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count() }

    #[test]
    fn keeps_duration_and_pitch() {
        for (a, b) in [(44100, 48000), (48000, 44100), (22050, 48000), (44100, 96000)] {
            let y = resample_stereo(&tone(a, 440.0, 2.0), a, b);
            let dur = y.len() as f64 / 2.0 / b as f64;
            assert!((dur - 2.0).abs() < 0.001, "{a}->{b} dur {dur}");
            assert!((zero_crossings(&y) as i64 - 880).abs() <= 2, "{a}->{b} cruces {}", zero_crossings(&y));
        }
    }
    #[test]
    fn identity_when_same_rate() {
        let t = tone(44100, 100.0, 0.1);
        assert_eq!(resample_stereo(&t, 44100, 44100), t);
    }
    #[test]
    fn channel_layouts() {
        assert_eq!(to_stereo(&[0.5, 0.25], 1), vec![0.5, 0.5, 0.25, 0.25]);
        assert_eq!(to_stereo(&[1.0, 2.0], 2), vec![1.0, 2.0]);
        let s = to_stereo(&[1.0, 0.0, 1.0, 0.0, 0.0, 0.0], 6); // 5.1: L,R,C,LFE,Ls,Rs
        assert_eq!(s.len(), 2);
    }
}
