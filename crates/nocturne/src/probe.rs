//! `nocturne --probe-audio`: mide el reloj del dispositivo frente al reloj monotónico.
use crate::audio::{list_output_devices, prepare, Audio};
use nocturne_core::{analysis::synth::drum_pattern, decode::DecodedAudio, Micros};

pub fn run(device: Option<&str>) {
    println!("dispositivos de salida: {:?}", list_output_devices());
    let mut a = Audio::new(device);
    a.set_volume(0.12, 0.1);
    match a.device_info() {
        Some(i) => println!("dispositivo: {} · {} Hz · {} canales · host {}", i.name, i.sample_rate, i.channels, i.host),
        None => { println!("sin dispositivo: {:?} (reloj silencioso)", a.device_error); }
    }
    let (t, _) = drum_pattern(44100, 120.0, 0.5, 24, 1.0);
    let d = DecodedAudio { samples: t.samples, sample_rate: 44100, channels: 1 };
    let buf = prepare(&d, a.sample_rate());
    a.install(buf, Micros::ZERO, true);
    let start = std::time::Instant::now();
    let mut worst = 0f64;
    let mut samples = vec![];
    while start.elapsed().as_secs_f64() < 5.0 {
        std::thread::sleep(std::time::Duration::from_millis(7));
        let c = a.clock().as_secs();
        let wall = start.elapsed().as_secs_f64();
        if wall > 1.0 { let e = (c - wall).abs(); worst = worst.max(e); samples.push(c - wall); }
    }
    let mean = samples.iter().sum::<f64>() / samples.len().max(1) as f64;
    println!("latencia de salida: {:.1} ms · buffer {} frames · xruns {}", a.latency_ms(), a.buffer_frames(), a.xruns());
    println!("reloj − pared: media {:+.2} ms, peor |Δ| {:.2} ms (incluye el arranque del stream)", mean * 1000.0, worst * 1000.0);
    a.pause();
    let p1 = a.clock();
    std::thread::sleep(std::time::Duration::from_millis(300));
    let p2 = a.clock();
    println!("pausa: posición congelada {} (Δ={} µs)", if p1 == p2 { "sí" } else { "NO" }, p2.0 - p1.0);
    a.seek(Micros::from_secs(2.0));
    a.play();
    std::thread::sleep(std::time::Duration::from_millis(500));
    println!("tras seek(2 s)+0,5 s: {:.3} s", a.clock().as_secs());
}
