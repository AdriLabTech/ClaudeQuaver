use crate::db::Db;
use bevy::prelude::*;
use std::sync::Mutex;

#[derive(States, Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub enum Screen {
    /// Estado inicial inerte: el `OnEnter` del estado inicial corre antes que ningún sistema de
    /// arranque, así que se sale de aquí a la pantalla pedida cuando ya existen fuentes y cámara.
    #[default]
    Boot,
    Home,
    Library,
    Playing,
    Results,
    Settings,
    Stats,
    Editor,
    About,
    Calibrate,
}

/// `rusqlite::Connection` no es `Sync`: se protege con un `Mutex`.
#[derive(Resource)]
pub struct AppDb(pub Mutex<Db>);

/// Pantalla a la que se vuelve al abandonar o terminar una partida (biblioteca, editor…).
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReturnTo(pub Screen);

impl Default for ReturnTo {
    fn default() -> Self { ReturnTo(Screen::Library) }
}

impl ReturnTo {
    pub fn label(self) -> &'static str {
        match self.0 { Screen::Editor => "Volver al editor", Screen::Home => "Volver al inicio", _ => "Volver a la biblioteca" }
    }
}
