use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("no se pudo abrir el archivo: {0}")]
    Io(#[from] std::io::Error),
    #[error("formato de audio no reconocido o no compatible")]
    UnsupportedFormat,
    #[error("el archivo no contiene ninguna pista de audio")]
    NoAudioTrack,
    #[error("audio corrupto o ilegible: {0}")]
    Decode(String),
    #[error("el audio está vacío o es demasiado corto ({0} ms)")]
    TooShort(i64),
    #[error("la señal contiene valores no finitos")]
    NonFinite,
    #[error("análisis cancelado")]
    Cancelled,
    #[error("mapa inválido: {0}")]
    InvalidBeatmap(String),
    #[error("versión de formato de mapa no compatible: {found} (soportada: {supported})")]
    IncompatibleVersion { found: u32, supported: u32 },
    #[error("error de serialización: {0}")]
    Serde(#[from] serde_json::Error),
}
