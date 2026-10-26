use crate::db::Db;
use bevy::prelude::*;
use std::sync::Mutex;

#[derive(States, Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub enum Screen {
    #[default]
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
