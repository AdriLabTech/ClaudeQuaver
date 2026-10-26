//! Decodificación de audio y de la pista de audio de contenedores (MP4/M4A).
//! El formato real se detecta por contenido (probe), no por extensión; la
//! extensión solo se pasa como pista (`Hint`).

use crate::{Error, Result};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use symphonia::core::audio::sample::Sample;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

/// Límite de duración para acotar memoria (≈ 1,4 GB/h en estéreo 48 kHz f32 sería excesivo).
pub const MAX_DURATION_SECS: f64 = 30.0 * 60.0;

#[derive(Debug, Clone)]
pub struct DecodedAudio {
    /// PCM f32 entrelazado, tal cual se reproduce.
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: usize,
}

impl DecodedAudio {
    pub fn frames(&self) -> usize { self.samples.len() / self.channels.max(1) }
    pub fn duration(&self) -> crate::Micros { crate::Micros::from_samples(self.frames() as u64, self.sample_rate) }
    /// Mezcla mono para el análisis; no modifica la señal original.
    pub fn to_mono(&self) -> Vec<f32> {
        let c = self.channels.max(1);
        self.samples.chunks_exact(c).map(|f| f.iter().sum::<f32>() / c as f32).collect()
    }
}

/// Decodifica un archivo completo. `cancel` permite abortar desde otro hilo.
pub fn decode_file(path: &Path, cancel: &AtomicBool) -> Result<DecodedAudio> {
    let file = File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) { hint.with_extension(ext); }

    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|_| Error::UnsupportedFormat)?;
    let track = format.default_track(TrackType::Audio).ok_or(Error::NoAudioTrack)?;
    let params = track.codec_params.as_ref().and_then(|p| p.audio()).ok_or(Error::NoAudioTrack)?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|e| Error::Decode(e.to_string()))?;
    let track_id = track.id;
    let no_container_delay = track.delay.is_none();

    let mut out = DecodedAudio { samples: Vec::new(), sample_rate: 0, channels: 0 };
    let mut scratch: Vec<f32> = Vec::new();
    loop {
        if cancel.load(Ordering::Relaxed) { return Err(Error::Cancelled) }
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(SymError::IoError(_)) | Err(SymError::ResetRequired) => break,
            Err(e) => return Err(Error::Decode(e.to_string())),
        };
        if packet.track_id != track_id { continue }
        match decoder.decode(&packet) {
            Ok(buf) => {
                let spec = buf.spec();
                if out.sample_rate == 0 {
                    out.sample_rate = spec.rate();
                    out.channels = spec.channels().count();
                }
                scratch.resize(buf.samples_interleaved(), f32::MID);
                buf.copy_to_slice_interleaved(&mut scratch);
                out.samples.extend_from_slice(&scratch);
                if out.samples.len() as f64 / (out.sample_rate.max(1) as f64 * out.channels.max(1) as f64) > MAX_DURATION_SECS {
                    return Err(Error::Decode(format!("la canción supera el límite de {} min", MAX_DURATION_SECS / 60.0)));
                }
            }
            Err(SymError::DecodeError(_)) => continue, // paquete corrupto aislado: se omite
            Err(e) => return Err(Error::Decode(e.to_string())),
        }
    }
    if out.sample_rate == 0 || out.samples.is_empty() { return Err(Error::NoAudioTrack) }
    if out.samples.iter().any(|s| !s.is_finite()) { return Err(Error::NonFinite) }
    // symphonia 0.6 no expone la edit list de MP4/M4A: el retardo del codificador (p. ej. 1024
    // muestras de AAC ≈ 23 ms) desplazaría todo el mapa respecto al audio. Se recorta aquí.
    if no_container_delay {
        if let Some((media_time, timescale)) = mp4_audio_edit_start(path) {
            let frames = (media_time as u128 * out.sample_rate as u128 / timescale.max(1) as u128) as usize;
            let drop = (frames * out.channels).min(out.samples.len().saturating_sub(out.channels));
            out.samples.drain(..drop);
        }
    }
    Ok(out)
}

fn read_box_header(f: &mut File, end: u64) -> Option<([u8; 4], u64, u64)> {
    let pos = f.stream_position().ok()?;
    if pos + 8 > end { return None }
    let mut h = [0u8; 8];
    f.read_exact(&mut h).ok()?;
    let mut size = u32::from_be_bytes(h[..4].try_into().unwrap()) as u64;
    let kind: [u8; 4] = h[4..8].try_into().unwrap();
    let mut header = 8;
    if size == 1 {
        let mut b = [0u8; 8];
        f.read_exact(&mut b).ok()?;
        size = u64::from_be_bytes(b);
        header = 16;
    } else if size == 0 { size = end - pos }
    if size < header || pos + size > end { return None }
    Some((kind, pos + header, pos + size))
}

/// Busca la caja `want` entre los hijos de `[start, end)` y devuelve su payload.
fn find_box(f: &mut File, start: u64, end: u64, want: &[u8; 4]) -> Option<(u64, u64)> {
    f.seek(SeekFrom::Start(start)).ok()?;
    while let Some((kind, body, next)) = read_box_header(f, end) {
        if &kind == want { return Some((body, next)) }
        f.seek(SeekFrom::Start(next)).ok()?;
    }
    None
}

fn be(f: &mut File, n: usize) -> Option<u64> {
    let mut b = [0u8; 8];
    f.read_exact(&mut b[8 - n..]).ok()?;
    Some(u64::from_be_bytes(b))
}

/// `(media_time, timescale)` de la primera edición no vacía del track de audio de un MP4/M4A.
pub fn mp4_audio_edit_start(path: &Path) -> Option<(u64, u32)> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut ftyp = [0u8; 8];
    f.read_exact(&mut ftyp).ok()?;
    if &ftyp[4..8] != b"ftyp" { return None }
    let (moov, moov_end) = find_box(&mut f, 0, len, b"moov")?;
    f.seek(SeekFrom::Start(moov)).ok()?;
    while let Some((kind, trak, trak_end)) = read_box_header(&mut f, moov_end) {
        if &kind == b"trak" {
            if let Some(r) = audio_trak_edit(&mut f, trak, trak_end) { return Some(r) }
        }
        f.seek(SeekFrom::Start(trak_end)).ok()?;
    }
    None
}

fn audio_trak_edit(f: &mut File, trak: u64, trak_end: u64) -> Option<(u64, u32)> {
    let (mdia, mdia_end) = find_box(f, trak, trak_end, b"mdia")?;
    let (hdlr, _) = find_box(f, mdia, mdia_end, b"hdlr")?;
    f.seek(SeekFrom::Start(hdlr + 8)).ok()?;
    let mut ht = [0u8; 4];
    f.read_exact(&mut ht).ok()?;
    if &ht != b"soun" { return None }
    let (mdhd, _) = find_box(f, mdia, mdia_end, b"mdhd")?;
    f.seek(SeekFrom::Start(mdhd)).ok()?;
    let version = be(f, 1)?;
    f.seek(SeekFrom::Current(if version == 1 { 3 + 16 } else { 3 + 8 })).ok()?;
    let timescale = be(f, 4)? as u32;
    let (edts, edts_end) = find_box(f, trak, trak_end, b"edts")?;
    let (elst, _) = find_box(f, edts, edts_end, b"elst")?;
    f.seek(SeekFrom::Start(elst)).ok()?;
    let v = be(f, 1)?;
    f.seek(SeekFrom::Current(3)).ok()?;
    let count = be(f, 4)?.min(16);
    for _ in 0..count {
        let (_, media_time) = if v == 1 { (be(f, 8)?, be(f, 8)? as i64) } else { (be(f, 4)?, be(f, 4)? as u32 as i32 as i64) };
        f.seek(SeekFrom::Current(4)).ok()?; // media_rate
        if media_time >= 0 { return Some((media_time as u64, timescale)) }
    }
    None
}

/// Escribe un WAV PCM16 (herramientas y pruebas; no se usa en el juego).
pub fn write_wav(path: &Path, samples: &[f32], sample_rate: u32, channels: u16) -> std::io::Result<()> {
    use std::io::Write;
    let data_len = (samples.len() * 2) as u32;
    let mut f = std::io::BufWriter::new(File::create(path)?);
    f.write_all(b"RIFF")?; f.write_all(&(36 + data_len).to_le_bytes())?; f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?; f.write_all(&1u16.to_le_bytes())?; f.write_all(&channels.to_le_bytes())?;
    f.write_all(&sample_rate.to_le_bytes())?; f.write_all(&(sample_rate * channels as u32 * 2).to_le_bytes())?;
    f.write_all(&(channels * 2).to_le_bytes())?; f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?; f.write_all(&data_len.to_le_bytes())?;
    for s in samples { f.write_all(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())?; }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("nocturne-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.join(name)
    }
    #[test]
    fn wav_roundtrip_stereo() {
        let p = tmp("a.wav");
        let s: Vec<f32> = (0..4410).flat_map(|i| { let v = (i as f32 * 0.05).sin() * 0.5; [v, -v] }).collect();
        write_wav(&p, &s, 44100, 2).unwrap();
        let d = decode_file(&p, &AtomicBool::new(false)).unwrap();
        assert_eq!((d.sample_rate, d.channels, d.frames()), (44100, 2, 4410));
        assert!((d.samples[100] - s[100]).abs() < 1e-3);
        assert!(d.to_mono().iter().all(|m| m.abs() < 1e-3));
    }
    #[test]
    fn format_detected_by_content_not_extension() {
        let p = tmp("really_wav.mp3");
        write_wav(&p, &vec![0.1; 4410], 44100, 1).unwrap();
        assert!(decode_file(&p, &AtomicBool::new(false)).is_ok());
    }
    #[test]
    fn garbage_and_empty_are_recoverable_errors() {
        let p = tmp("junk.mp3");
        std::fs::write(&p, b"esto no es audio").unwrap();
        assert!(decode_file(&p, &AtomicBool::new(false)).is_err());
        let e = tmp("empty.wav");
        std::fs::write(&e, b"").unwrap();
        assert!(decode_file(&e, &AtomicBool::new(false)).is_err());
        assert!(matches!(decode_file(Path::new("/no/existe.mp3"), &AtomicBool::new(false)), Err(Error::Io(_))));
    }
    #[test]
    fn cancel_aborts() {
        let p = tmp("c.wav");
        write_wav(&p, &vec![0.1; 44100], 44100, 1).unwrap();
        assert!(matches!(decode_file(&p, &AtomicBool::new(true)), Err(Error::Cancelled)));
    }
}
