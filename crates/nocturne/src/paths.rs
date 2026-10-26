//! Rutas XDG. `NOCTURNE_HOME` redirige todo (pruebas y ejecuciones aisladas).
use std::path::PathBuf;

pub fn home_dir() -> PathBuf { std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")) }

fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home_dir().join(fallback))
}

pub fn config_dir() -> PathBuf {
    match std::env::var_os("NOCTURNE_HOME") {
        Some(h) => PathBuf::from(h).join("config"),
        None => xdg("XDG_CONFIG_HOME", ".config").join("nocturne"),
    }
}

pub fn data_dir() -> PathBuf {
    match std::env::var_os("NOCTURNE_HOME") {
        Some(h) => PathBuf::from(h).join("data"),
        None => xdg("XDG_DATA_HOME", ".local/share").join("nocturne"),
    }
}

pub fn maps_dir() -> PathBuf { data_dir().join("maps") }
pub fn db_path() -> PathBuf { data_dir().join("nocturne.db") }
pub fn settings_path() -> PathBuf { config_dir().join("settings.json") }

/// Sustituye el directorio personal por `~` para no escribir rutas personales en logs.
pub fn redact(p: &std::path::Path) -> String {
    let s = p.to_string_lossy().to_string();
    let h = home_dir().to_string_lossy().to_string();
    if !h.is_empty() && s.starts_with(&h) { format!("~{}", &s[h.len()..]) } else { s }
}
