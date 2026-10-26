//! Ajustes persistentes. Todo valor se valida al cargar: lo inválido vuelve a su valor seguro
//! y se informa en `Settings::warnings` (visible en la pantalla de ajustes).

use bevy::prelude::{KeyCode, Resource};
use nocturne_core::analysis::AnalysisParams;
use nocturne_core::scoring::HitWindows;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const SETTINGS_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KeyProfile {
    pub name: String,
    pub keys: [KeyCode; 4],
}

#[derive(Debug, Clone, Serialize, Deserialize, Resource)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    // Gameplay
    /// 1..40; píxeles/s = velocidad × 55 sobre un alto de diseño de 1080.
    pub scroll_speed: f32,
    pub note_scale: f32,
    /// Altura de la línea de impacto sobre el borde inferior (px de diseño).
    pub hit_line_height: f32,
    pub show_judgement: bool,
    pub show_hit_effects: bool,
    pub show_key_hints: bool,
    pub show_progress: bool,
    pub show_life: bool,
    pub no_fail: bool,
    pub hit_windows: HitWindows,
    // Controles
    pub profiles: Vec<KeyProfile>,
    pub active_profile: usize,
    // Audio
    pub music_volume: f32,
    pub sfx_volume: f32,
    pub audio_device: Option<String>,
    pub global_offset_ms: f32,
    // Gráficos
    pub fullscreen: bool,
    pub vsync: bool,
    /// 0 = sin límite.
    pub fps_limit: u32,
    pub ui_scale: f32,
    pub reduce_motion: bool,
    pub show_diagnostics: bool,
    // Análisis
    pub onset_sensitivity: f32,
    pub generate_holds: bool,
    pub density_scale: f32,
    #[serde(skip)]
    pub warnings: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            version: SETTINGS_VERSION,
            scroll_speed: 20.0, note_scale: 1.0, hit_line_height: 180.0,
            show_judgement: true, show_hit_effects: true, show_key_hints: true, show_progress: true, show_life: true,
            no_fail: false, hit_windows: HitWindows::default(),
            profiles: vec![KeyProfile { name: "Estándar".into(), keys: [KeyCode::KeyD, KeyCode::KeyF, KeyCode::KeyJ, KeyCode::KeyK] }],
            active_profile: 0,
            music_volume: 0.7, sfx_volume: 0.5, audio_device: None, global_offset_ms: 0.0,
            fullscreen: false, vsync: true, fps_limit: 0, ui_scale: 1.0, reduce_motion: false, show_diagnostics: false,
            onset_sensitivity: 0.5, generate_holds: true, density_scale: 1.0,
            warnings: vec![],
        }
    }
}

impl Settings {
    pub fn keys(&self) -> [KeyCode; 4] { self.profiles[self.active_profile.min(self.profiles.len() - 1)].keys }
    pub fn scroll_px_per_s(&self) -> f32 { self.scroll_speed * 55.0 }

    pub fn analysis_params(&self) -> AnalysisParams {
        AnalysisParams { onset_sensitivity: self.onset_sensitivity, ..Default::default() }
    }

    /// Corrige valores fuera de rango. Devuelve las correcciones aplicadas.
    pub fn validate(&mut self) -> Vec<String> {
        let mut w = vec![];
        fn clamp(w: &mut Vec<String>, name: &str, v: &mut f32, lo: f32, hi: f32, default: f32) {
            if !v.is_finite() { *v = default; w.push(format!("{name}: valor no válido, restaurado a {default}")) }
            else if *v < lo || *v > hi { let old = *v; *v = v.clamp(lo, hi); w.push(format!("{name}: {old} fuera de rango, ajustado a {}", *v)) }
        }
        let d = Settings::default();
        clamp(&mut w, "velocidad", &mut self.scroll_speed, 1.0, 40.0, d.scroll_speed);
        clamp(&mut w, "escala de notas", &mut self.note_scale, 0.6, 1.5, d.note_scale);
        clamp(&mut w, "altura de la línea", &mut self.hit_line_height, 60.0, 400.0, d.hit_line_height);
        clamp(&mut w, "volumen de música", &mut self.music_volume, 0.0, 1.0, d.music_volume);
        clamp(&mut w, "volumen de efectos", &mut self.sfx_volume, 0.0, 1.0, d.sfx_volume);
        clamp(&mut w, "offset global", &mut self.global_offset_ms, -300.0, 300.0, 0.0);
        clamp(&mut w, "escala de interfaz", &mut self.ui_scale, 0.7, 1.6, d.ui_scale);
        clamp(&mut w, "sensibilidad", &mut self.onset_sensitivity, 0.0, 1.0, d.onset_sensitivity);
        clamp(&mut w, "densidad", &mut self.density_scale, 0.5, 2.0, d.density_scale);
        if self.fps_limit != 0 && !(30..=1000).contains(&self.fps_limit) {
            w.push(format!("límite de FPS {} no válido, desactivado", self.fps_limit));
            self.fps_limit = 0;
        }
        if !self.hit_windows.is_valid() {
            w.push("ventanas de precisión no válidas, restauradas".into());
            self.hit_windows = HitWindows::default();
        }
        if self.profiles.is_empty() {
            w.push("sin perfiles de teclas, restaurado el estándar".into());
            self.profiles = d.profiles.clone();
        }
        for p in &mut self.profiles {
            if let Some(dup) = duplicate_key(&p.keys) {
                w.push(format!("perfil «{}»: tecla {dup:?} repetida, restaurado", p.name));
                p.keys = d.profiles[0].keys;
            }
            if p.keys.iter().any(|k| !is_bindable(*k)) {
                w.push(format!("perfil «{}»: tecla no permitida, restaurado", p.name));
                p.keys = d.profiles[0].keys;
            }
        }
        if self.active_profile >= self.profiles.len() { self.active_profile = 0; w.push("perfil activo inexistente, usando el primero".into()) }
        w
    }

    /// Carga desde disco. Archivo ausente → valores por defecto; corrupto → copia `.bak` + defecto.
    pub fn load(path: &Path) -> Settings {
        let mut s = match std::fs::read_to_string(path) {
            Ok(txt) => match serde_json::from_str::<Settings>(&txt) {
                Ok(s) => s,
                Err(e) => {
                    let _ = std::fs::rename(path, path.with_extension("json.bak"));
                    let mut d = Settings::default();
                    d.warnings.push(format!("ajustes ilegibles ({e}); copia guardada como settings.json.bak"));
                    return d;
                }
            },
            Err(_) => return Settings::default(),
        };
        s.warnings = s.validate();
        s
    }

    /// Guardado atómico (escribe a un temporal y renombra).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)? }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(tmp, path)
    }
}

pub fn duplicate_key(keys: &[KeyCode; 4]) -> Option<KeyCode> {
    (0..4).find_map(|i| (i + 1..4).find(|j| keys[i] == keys[*j]).map(|_| keys[i]))
}

/// Teclas que pueden asignarse a un carril (se excluyen las reservadas a la interfaz).
pub fn is_bindable(k: KeyCode) -> bool {
    !matches!(k, KeyCode::Escape | KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Tab | KeyCode::F11 | KeyCode::Unidentified(_)
        | KeyCode::SuperLeft | KeyCode::SuperRight | KeyCode::Fn | KeyCode::FnLock)
}

/// Nombre legible de una tecla (para el HUD y los ajustes).
pub fn key_label(k: KeyCode) -> String {
    let s = format!("{k:?}");
    let s = s.strip_prefix("Key").or_else(|| s.strip_prefix("Digit")).unwrap_or(&s).to_string();
    match s.as_str() {
        "Space" => "Espacio".into(), "ArrowLeft" => "←".into(), "ArrowRight" => "→".into(), "ArrowUp" => "↑".into(), "ArrowDown" => "↓".into(),
        "Semicolon" => ";".into(), "Comma" => ",".into(), "Period" => ".".into(), "Slash" => "/".into(), "Quote" => "'".into(),
        "BracketLeft" => "[".into(), "BracketRight" => "]".into(), "ShiftLeft" | "ShiftRight" => "Shift".into(),
        _ => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("nocturne-set-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.join(name)
    }
    #[test]
    fn defaults_are_valid() {
        let mut s = Settings::default();
        assert!(s.validate().is_empty());
    }
    #[test]
    fn roundtrip_persists() {
        let p = tmp("a.json");
        let mut s = Settings::default();
        s.scroll_speed = 31.0; s.global_offset_ms = -12.5; s.profiles[0].keys = [KeyCode::KeyA, KeyCode::KeyS, KeyCode::KeyK, KeyCode::KeyL];
        s.save(&p).unwrap();
        let l = Settings::load(&p);
        assert_eq!(l.scroll_speed, 31.0);
        assert_eq!(l.global_offset_ms, -12.5);
        assert_eq!(l.keys(), [KeyCode::KeyA, KeyCode::KeyS, KeyCode::KeyK, KeyCode::KeyL]);
        assert!(l.warnings.is_empty());
    }
    #[test]
    fn out_of_range_values_are_repaired_and_reported() {
        let p = tmp("b.json");
        std::fs::write(&p, r#"{"scroll_speed": 900.0, "music_volume": 3.0, "fps_limit": 7, "ui_scale": -1.0}"#).unwrap();
        let s = Settings::load(&p);
        assert_eq!(s.scroll_speed, 40.0);
        assert_eq!(s.music_volume, 1.0);
        assert_eq!(s.fps_limit, 0);
        assert!(s.ui_scale >= 0.7);
        assert!(s.warnings.len() >= 4, "{:?}", s.warnings);
    }
    #[test]
    fn corrupt_file_is_backed_up() {
        let p = tmp("c.json");
        std::fs::write(&p, "{ esto no es json").unwrap();
        let s = Settings::load(&p);
        assert_eq!(s.scroll_speed, Settings::default().scroll_speed);
        assert!(!s.warnings.is_empty());
        assert!(p.with_extension("json.bak").exists());
    }
    #[test]
    fn duplicate_and_reserved_keys_rejected() {
        assert_eq!(duplicate_key(&[KeyCode::KeyD, KeyCode::KeyD, KeyCode::KeyJ, KeyCode::KeyK]), Some(KeyCode::KeyD));
        assert!(!is_bindable(KeyCode::Escape));
        assert!(is_bindable(KeyCode::KeyD));
        let mut s = Settings::default();
        s.profiles[0].keys = [KeyCode::KeyD, KeyCode::KeyD, KeyCode::KeyJ, KeyCode::KeyK];
        assert!(!s.validate().is_empty());
        assert_eq!(s.keys()[1], KeyCode::KeyF);
    }
    #[test]
    fn key_labels() {
        assert_eq!(key_label(KeyCode::KeyD), "D");
        assert_eq!(key_label(KeyCode::Digit4), "4");
        assert_eq!(key_label(KeyCode::Space), "Espacio");
    }
}
