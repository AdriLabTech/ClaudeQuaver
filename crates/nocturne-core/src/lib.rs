//! Núcleo de Nocturne: lógica pura, sin ventana ni dispositivo de audio.
//! Todo lo que hay aquí se prueba en `cargo test` sin entorno gráfico.

pub mod analysis;
pub mod beatmap;
pub mod clock;
pub mod decode;
pub mod error;
pub mod gameplay;
pub mod resample;
pub mod scoring;
pub mod time;

pub use error::{Error, Result};
pub use time::Micros;

/// Nombre del producto en un único punto (sustituible).
pub const APP_NAME: &str = "Nocturne";
