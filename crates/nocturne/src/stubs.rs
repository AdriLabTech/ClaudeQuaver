//! Pantallas aún sin implementar: vuelven al inicio (se sustituyen una a una).
use crate::app::Screen;
use bevy::prelude::*;

pub fn plugin(app: &mut App) {
    for s in [Screen::Library, Screen::Stats, Screen::Settings, Screen::Editor, Screen::About, Screen::Calibrate] {
        app.add_systems(Update, back.run_if(in_state(s)));
    }
}
fn back(mut next: ResMut<NextState<Screen>>) { next.set(Screen::Home) }
