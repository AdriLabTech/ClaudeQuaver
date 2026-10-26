//! Motor de audio: reproducción por `cpal` con reloj propio.
//!
//! Reglas del callback (hilo de tiempo real): sin asignaciones, sin bloqueos, sin E/S. La canción
//! llega ya remuestreada a estéreo al formato del dispositivo; los comandos entran por un canal
//! `mpsc` (solo `try_recv`); la posición sale por atómicos con un seqlock.
//!
//! Posición audible = `frame_inicio_buffer + (t_pared − t_callback − latencia) · fs`, donde la
//! latencia es `playback − callback` según el timestamp de cpal. Sin dispositivo se usa un reloj
//! silencioso basado en `Instant` para que el juego siga siendo utilizable y comprobable.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig};
use nocturne_core::{clock::Smoother, decode::DecodedAudio, resample, Micros};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::*};
use std::sync::{mpsc, Arc};
use std::time::Instant;

const SFX_VOICES: usize = 12;

enum Cmd {
    Load(Arc<Vec<f32>>),
    Play,
    Pause,
    Seek(u64),
    Unload,
}

struct Shared {
    epoch: Instant,
    playing: AtomicBool,
    ended: AtomicBool,
    next_frame: AtomicU64,
    volume: AtomicU32,
    sfx_volume: AtomicU32,
    sfx_counter: AtomicU32,
    seq: AtomicU32,
    t_cb_ns: AtomicU64,
    start_frame: AtomicU64,
    latency_ns: AtomicU64,
    buffer_frames: AtomicU32,
    callbacks: AtomicU64,
    xruns: AtomicU32,
}

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub name: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub host: String,
}

struct Device_ {
    /// Al soltarlo, el hilo que posee el `Stream` (no `Send` en algunos backends) lo cierra.
    _stop: mpsc::Sender<()>,
    tx: mpsc::Sender<Cmd>,
    shared: Arc<Shared>,
    rate: u32,
    info: DeviceInfo,
}

/// Fuente de tiempo para el juego. Un único objeto para menús, vista previa y partida.
#[derive(bevy::prelude::Resource)]
pub struct Audio {
    dev: Option<Device_>,
    pub device_error: Option<String>,
    song: Option<Arc<Vec<f32>>>,
    song_frames: u64,
    silent_rate: u32,
    // Reloj silencioso (sin dispositivo)
    silent_base_us: f64,
    silent_since: Option<Instant>,
    // Estado lógico
    playing: bool,
    held_us: f64,
    smoother: Smoother,
    epoch: Instant,
    pub volume: f32,
    pub sfx_volume: f32,
    pub duration: Micros,
    last_time: Micros,
    hold_until: Option<Instant>,
}

fn f32_bits(v: f32) -> u32 { v.to_bits() }
fn bits_f32(v: u32) -> f32 { f32::from_bits(v) }

/// Lista los nombres de dispositivos de salida disponibles.
pub fn list_output_devices() -> Vec<String> {
    let host = cpal::default_host();
    const NOISE: [&str; 9] = ["Rate Converter", "Plugin", "Discard", "Open Sound System", "JACK", "Speex", "upmix", "downmix", "Default ALSA"];
    let mut v: Vec<String> = host.output_devices()
        .map(|it| it.filter_map(|d| d.description().ok().map(|d| d.name().to_string())).collect())
        .unwrap_or_default();
    v.retain(|n| !NOISE.iter().any(|x| n.contains(x)) && !n.trim().is_empty());
    v.dedup();
    let mut seen = std::collections::HashSet::new();
    v.retain(|n| seen.insert(n.clone()));
    v
}

fn pick_device(name: Option<&str>) -> Option<Device> {
    let host = cpal::default_host();
    if let Some(n) = name {
        if let Ok(it) = host.output_devices() {
            for d in it { if d.description().map(|x| x.name() == n).unwrap_or(false) { return Some(d) } }
        }
    }
    host.default_output_device()
}

impl Audio {
    pub fn new(device_name: Option<&str>) -> Self {
        let epoch = Instant::now();
        let mut a = Audio {
            dev: None, device_error: None, song: None, song_frames: 0, silent_rate: 48_000,
            silent_base_us: 0.0, silent_since: None, playing: false, held_us: 0.0,
            smoother: Smoother::default(), epoch, volume: 0.8, sfx_volume: 0.5,
            duration: Micros::ZERO, last_time: Micros::ZERO, hold_until: None,
        };
        match Self::open(device_name, epoch) {
            Ok(d) => a.dev = Some(d),
            Err(e) => { a.device_error = Some(e); }
        }
        a
    }

    fn open(name: Option<&str>, epoch: Instant) -> Result<Device_, String> {
        let name = name.map(str::to_owned);
        let shared = Arc::new(Shared {
            epoch, playing: AtomicBool::new(false), ended: AtomicBool::new(false), next_frame: AtomicU64::new(0),
            volume: AtomicU32::new(f32_bits(0.8)), sfx_volume: AtomicU32::new(f32_bits(0.5)), sfx_counter: AtomicU32::new(0),
            seq: AtomicU32::new(0), t_cb_ns: AtomicU64::new(0), start_frame: AtomicU64::new(0), latency_ns: AtomicU64::new(0),
            buffer_frames: AtomicU32::new(0), callbacks: AtomicU64::new(0), xruns: AtomicU32::new(0),
        });
        let (tx, rx) = mpsc::channel::<Cmd>();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(u32, DeviceInfo), String>>();
        let sh = shared.clone();
        std::thread::Builder::new().name("nocturne-audio".into()).spawn(move || {
            match Self::build_stream(name.as_deref(), rx, sh) {
                Ok((stream, rate, info)) => {
                    let _ = ready_tx.send(Ok((rate, info)));
                    let _ = stop_rx.recv(); // bloquea hasta que `Audio` se destruya
                    drop(stream);
                }
                Err(e) => { let _ = ready_tx.send(Err(e)); }
            }
        }).map_err(|e| e.to_string())?;
        let (rate, info) = ready_rx.recv().map_err(|_| "el hilo de audio terminó inesperadamente".to_string())??;
        Ok(Device_ { _stop: stop_tx, tx, shared, rate, info })
    }

    fn build_stream(name: Option<&str>, rx: mpsc::Receiver<Cmd>, sh: Arc<Shared>) -> Result<(Stream, u32, DeviceInfo), String> {
        let device = pick_device(name).ok_or("no hay dispositivo de salida de audio")?;
        let cfg = device.default_output_config().map_err(|e| format!("configuración de audio: {e}"))?;
        let fmt = cfg.sample_format();
        let config: StreamConfig = cfg.into();
        let (rate, channels) = (config.sample_rate, config.channels as usize);
        let err_sh = sh.clone();
        let err_fn = move |e: cpal::Error| {
            if matches!(e.kind(), cpal::ErrorKind::Xrun) { err_sh.xruns.fetch_add(1, Relaxed); }
            eprintln!("audio: {e}");
        };
        let mut state = CallbackState::new(rate, rx, sh);
        macro_rules! build {
            ($t:ty) => {
                device.build_output_stream(
                    config.clone(),
                    move |data: &mut [$t], info: &cpal::OutputCallbackInfo| state.fill(data, channels, info),
                    err_fn, None)
            };
        }
        let stream = match fmt {
            SampleFormat::F32 => build!(f32),
            SampleFormat::I16 => build!(i16),
            SampleFormat::I32 => build!(i32),
            SampleFormat::U16 => build!(u16),
            other => return Err(format!("formato de muestra no soportado: {other}")),
        }.map_err(|e| format!("no se pudo abrir el dispositivo: {e}"))?;
        stream.play().map_err(|e| format!("no se pudo iniciar la reproducción: {e}"))?;
        let host = cpal::default_host().id().to_string();
        let info = DeviceInfo { name: device.description().map(|d| d.name().to_string()).unwrap_or_else(|_| "desconocido".into()),
            sample_rate: rate, channels: channels as u16, host };
        Ok((stream, rate, info))
    }

    pub fn device_info(&self) -> Option<&DeviceInfo> { self.dev.as_ref().map(|d| &d.info) }
    pub fn has_device(&self) -> bool { self.dev.is_some() }
    pub fn sample_rate(&self) -> u32 { self.dev.as_ref().map_or(self.silent_rate, |d| d.rate) }

    /// Latencia de salida estimada en ms (0 sin dispositivo).
    pub fn latency_ms(&self) -> f64 {
        self.dev.as_ref().map_or(0.0, |d| d.shared.latency_ns.load(Relaxed) as f64 / 1e6)
    }
    pub fn buffer_frames(&self) -> u32 { self.dev.as_ref().map_or(0, |d| d.shared.buffer_frames.load(Relaxed)) }
    pub fn xruns(&self) -> u32 { self.dev.as_ref().map_or(0, |d| d.shared.xruns.load(Relaxed)) }

    /// Reabre el dispositivo (cambio de salida desde ajustes).
    pub fn reopen(&mut self, name: Option<&str>) {
        let song = self.song.clone();
        let pos = self.position();
        let was_playing = self.playing;
        *self = { let mut n = Audio::new(name); n.volume = self.volume; n.sfx_volume = self.sfx_volume; n };
        if let Some(s) = song { self.install(s, pos, was_playing) }
    }

    /// Prepara el PCM del dispositivo. Costoso: llamar desde un hilo de fondo (ver [`prepare`]).
    pub fn install(&mut self, buf: Arc<Vec<f32>>, at: Micros, play: bool) {
        self.song_frames = (buf.len() / 2) as u64;
        self.duration = Micros::from_samples(self.song_frames, self.sample_rate());
        self.song = Some(buf.clone());
        if let Some(d) = &self.dev {
            d.shared.ended.store(false, Relaxed);
            let _ = d.tx.send(Cmd::Load(buf));
        }
        self.playing = false;
        self.seek(at);
        if play { self.play() }
    }

    pub fn unload(&mut self) {
        self.pause();
        self.song = None;
        self.song_frames = 0;
        self.duration = Micros::ZERO;
        if let Some(d) = &self.dev { let _ = d.tx.send(Cmd::Unload); }
        self.held_us = 0.0;
        self.smoother.reset();
    }

    pub fn is_loaded(&self) -> bool { self.song.is_some() }
    pub fn is_playing(&self) -> bool { self.playing }

    pub fn set_volume(&mut self, v: f32, sfx: f32) {
        self.volume = v.clamp(0.0, 1.0);
        self.sfx_volume = sfx.clamp(0.0, 1.0);
        if let Some(d) = &self.dev {
            d.shared.volume.store(f32_bits(self.volume), Relaxed);
            d.shared.sfx_volume.store(f32_bits(self.sfx_volume), Relaxed);
        }
    }

    /// Dispara el sonido de acierto (síntesis en el callback).
    pub fn hit_sound(&self) { if let Some(d) = &self.dev { d.shared.sfx_counter.fetch_add(1, Relaxed); } }

    pub fn play(&mut self) {
        if self.playing || self.song.is_none() { return }
        self.playing = true;
        self.smoother.reset();
        match &self.dev {
            Some(d) => { let _ = d.tx.send(Cmd::Play); d.shared.playing.store(true, Relaxed); }
            None => { self.silent_since = Some(Instant::now()); }
        }
    }

    /// Pausa y congela la posición en el valor audible actual.
    pub fn pause(&mut self) {
        if !self.playing { return }
        let p = self.position();
        self.playing = false;
        self.held_us = p.0 as f64;
        if let Some(d) = &self.dev {
            let _ = d.tx.send(Cmd::Pause);
            d.shared.playing.store(false, Relaxed);
            // Reanudar desde la posición congelada (no desde la cola ya escrita en el buffer).
            let _ = d.tx.send(Cmd::Seek(p.to_samples(d.rate).max(0) as u64));
        } else {
            self.silent_base_us = p.0 as f64;
            self.silent_since = None;
        }
        self.smoother.reset();
    }

    pub fn seek(&mut self, t: Micros) {
        let t = Micros(t.0.clamp(0, self.duration.0.max(0)));
        self.held_us = t.0 as f64;
        self.last_time = t;
        self.smoother.reset();
        // Hasta que el callback publique la nueva posición, la antigua no es válida: se retiene.
        self.hold_until = Some(Instant::now() + std::time::Duration::from_millis(30 + 3 * (self.buffer_frames() as u64 * 1000 / self.sample_rate().max(1) as u64)));
        if let Some(d) = &self.dev {
            let _ = d.tx.send(Cmd::Seek(t.to_samples(d.rate).max(0) as u64));
            d.shared.ended.store(false, Relaxed);
        } else {
            self.silent_base_us = t.0 as f64;
            if self.playing { self.silent_since = Some(Instant::now()) }
        }
    }

    pub fn restart(&mut self) { self.pause(); self.seek(Micros::ZERO); self.play(); }

    /// ¿Ha terminado la reproducción de la canción?
    pub fn is_finished(&self) -> bool {
        if !self.is_loaded() { return false }
        match &self.dev {
            Some(d) => d.shared.ended.load(Relaxed),
            None => self.playing && self.position() >= self.duration,
        }
    }

    /// Posición cruda del dispositivo en µs (sin suavizar), o la del reloj silencioso.
    fn raw_us(&self) -> f64 {
        if !self.playing { return self.held_us }
        if self.hold_until.map_or(false, |h| Instant::now() < h) { return self.held_us }
        match &self.dev {
            None => self.silent_base_us + self.silent_since.map_or(0.0, |s| s.elapsed().as_micros() as f64),
            Some(d) => {
                let sh = &d.shared;
                let (start, t_cb, lat) = loop {
                    let s1 = sh.seq.load(Acquire);
                    if s1 & 1 == 1 { std::hint::spin_loop(); continue }
                    let v = (sh.start_frame.load(Relaxed), sh.t_cb_ns.load(Relaxed), sh.latency_ns.load(Relaxed));
                    if sh.seq.load(Acquire) == s1 { break v }
                };
                if sh.callbacks.load(Relaxed) == 0 { return self.held_us }
                let now_ns = self.epoch_ns(sh);
                let elapsed = now_ns.saturating_sub(t_cb) as f64 - lat as f64;
                let t = start as f64 / d.rate as f64 * 1e6 + elapsed / 1e3;
                t.max(self.held_us) // tras reanudar, no retroceder mientras llega el primer buffer
            }
        }
    }

    fn epoch_ns(&self, sh: &Shared) -> u64 { sh.epoch.elapsed().as_nanos() as u64 }

    /// Posición musical suavizada. Llamar una vez por fotograma.
    pub fn position(&self) -> Micros {
        if !self.playing { return Micros(self.held_us as i64) }
        Micros(self.raw_us() as i64)
    }

    /// Versión con suavizado para dibujar/juzgar (muta el filtro).
    pub fn clock(&mut self) -> Micros {
        if !self.playing { self.last_time = Micros(self.held_us as i64); return self.last_time }
        let raw = self.raw_us();
        let wall = self.epoch.elapsed().as_micros() as f64;
        let t = Micros(self.smoother.update(raw, wall) as i64);
        self.last_time = t;
        t
    }
}

/// Convierte una canción decodificada al formato del dispositivo (estéreo, su frecuencia).
pub fn prepare(audio: &DecodedAudio, device_rate: u32) -> Arc<Vec<f32>> {
    let st = resample::to_stereo(&audio.samples, audio.channels);
    Arc::new(resample::resample_stereo(&st, audio.sample_rate, device_rate))
}

// ---------------------------------------------------------------------------------------------

struct Voice { left: u32, phase: f32 }

struct CallbackState {
    rate: u32,
    rx: mpsc::Receiver<Cmd>,
    sh: Arc<Shared>,
    song: Option<Arc<Vec<f32>>>,
    pos: u64,
    seen_sfx: u32,
    voices: [Voice; SFX_VOICES],
}

impl CallbackState {
    fn new(rate: u32, rx: mpsc::Receiver<Cmd>, sh: Arc<Shared>) -> Self {
        CallbackState { rate, rx, sh, song: None, pos: 0, seen_sfx: 0,
            voices: std::array::from_fn(|_| Voice { left: 0, phase: 0.0 }) }
    }

    fn fill<T: cpal::SizedSample + cpal::FromSample<f32>>(&mut self, out: &mut [T], channels: usize, info: &cpal::OutputCallbackInfo) {
        let now_ns = self.sh.epoch.elapsed().as_nanos() as u64;
        while let Ok(c) = self.rx.try_recv() {
            match c {
                Cmd::Load(b) => { self.song = Some(b); self.pos = 0; }
                Cmd::Play => {}
                Cmd::Pause => {}
                Cmd::Seek(f) => self.pos = f,
                Cmd::Unload => { self.song = None; self.pos = 0; }
            }
        }
        let frames = out.len() / channels;
        let playing = self.sh.playing.load(Relaxed);
        let (vol, sfx_vol) = (bits_f32(self.sh.volume.load(Relaxed)), bits_f32(self.sh.sfx_volume.load(Relaxed)));
        let ts = info.timestamp();
        let latency = ts.playback.saturating_duration_since(ts.callback).as_nanos() as u64;
        // Nuevos aciertos desde la última pasada.
        let c = self.sh.sfx_counter.load(Relaxed);
        for _ in 0..c.wrapping_sub(self.seen_sfx).min(4) {
            if let Some(v) = self.voices.iter_mut().find(|v| v.left == 0) { v.left = (self.rate as f32 * 0.035) as u32; v.phase = 0.0; }
        }
        self.seen_sfx = c;
        let start = self.pos;
        let sr = self.rate as f32;
        for f in 0..frames {
            let (mut l, mut r) = (0f32, 0f32);
            if playing {
                if let Some(song) = &self.song {
                    let i = (self.pos as usize) * 2;
                    if i + 1 < song.len() {
                        l = song[i] * vol; r = song[i + 1] * vol;
                        self.pos += 1;
                    } else {
                        self.sh.ended.store(true, Relaxed);
                    }
                }
            }
            for v in self.voices.iter_mut() {
                if v.left > 0 {
                    let env = v.left as f32 / (sr * 0.035);
                    let s = (v.phase * std::f32::consts::TAU).sin() * env * env * 0.35 * sfx_vol;
                    v.phase = (v.phase + 1800.0 / sr).fract();
                    v.left -= 1;
                    l += s; r += s;
                }
            }
            let (l, r) = (l.clamp(-1.0, 1.0), r.clamp(-1.0, 1.0));
            let base = f * channels;
            match channels {
                1 => out[base] = T::from_sample((l + r) * 0.5),
                _ => { out[base] = T::from_sample(l); out[base + 1] = T::from_sample(r);
                       for c in 2..channels { out[base + c] = T::from_sample(0.0); } }
            }
        }
        // Publicación (seqlock): par = estable.
        let sh = &self.sh;
        sh.seq.fetch_add(1, Release);
        sh.start_frame.store(start, Relaxed);
        sh.t_cb_ns.store(now_ns, Relaxed);
        sh.latency_ns.store(latency, Relaxed);
        sh.seq.fetch_add(1, Release);
        sh.next_frame.store(self.pos, Relaxed);
        sh.buffer_frames.store(frames as u32, Relaxed);
        sh.callbacks.fetch_add(1, Relaxed);
    }
}
