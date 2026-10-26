//! Sistema de diseño: tokens de color, tipografías y medidas. Nada más define colores sueltos.
//! Paleta de obsidiana: negros y grises fríos; el color solo informa (errores, estados).

use bevy::prelude::*;

pub const APP_NAME: &str = nocturne_core::APP_NAME;

const fn hex(r: u8, g: u8, b: u8) -> Color { Color::srgb_u8(r, g, b) }

pub const BG: Color = hex(0x08, 0x09, 0x0B);
pub const SURFACE: Color = hex(0x11, 0x13, 0x17);
pub const SURFACE_2: Color = hex(0x19, 0x1C, 0x21);
pub const BORDER: Color = hex(0x29, 0x2D, 0x34);
pub const TEXT_DIM: Color = hex(0x92, 0x98, 0xA2);
pub const TEXT: Color = hex(0xF2, 0xF3, 0xF5);
pub const WHITE: Color = hex(0xFF, 0xFF, 0xFF);
pub const TEXT_WARM: Color = hex(0xD9, 0xD4, 0xCC);
/// Solo funcional: fallo/error.
pub const DANGER: Color = hex(0xC4, 0x5B, 0x5B);
/// Solo funcional: confirmación.
pub const OK: Color = hex(0x9F, 0xC4, 0xA8);

pub fn alpha(c: Color, a: f32) -> Color { c.with_alpha(a) }

/// Color de cada juicio: luminancia primero, matiz solo para Bad/Miss.
pub fn judgement_color(j: nocturne_core::scoring::Judgement) -> Color {
    use nocturne_core::scoring::Judgement::*;
    match j {
        Marvelous => WHITE, Perfect => TEXT, Great => hex(0xC9, 0xCD, 0xD3), Good => TEXT_DIM,
        Bad => hex(0xB3, 0x8F, 0x6E), Miss => DANGER,
    }
}

/// Fuentes embebidas en el binario (Space Grotesk, SIL OFL 1.1): no dependen de rutas de instalación.
#[derive(Resource, Clone)]
pub struct Fonts { pub light: Handle<Font>, pub regular: Handle<Font>, pub medium: Handle<Font>, pub bold: Handle<Font> }

pub fn load_fonts(mut commands: Commands, mut fonts: ResMut<Assets<Font>>) {
    let mut add = |bytes: &'static [u8]| fonts.add(Font::from_bytes(bytes.to_vec()));
    commands.insert_resource(Fonts {
        light: add(include_bytes!("../../../assets/fonts/SpaceGrotesk-Light.otf")),
        regular: add(include_bytes!("../../../assets/fonts/SpaceGrotesk-Regular.otf")),
        medium: add(include_bytes!("../../../assets/fonts/SpaceGrotesk-Medium.otf")),
        bold: add(include_bytes!("../../../assets/fonts/SpaceGrotesk-Bold.otf")),
    });
}

#[derive(Clone, Copy)]
pub enum Weight { Light, Regular, Medium, Bold }

impl Fonts {
    pub fn get(&self, w: Weight) -> Handle<Font> {
        match w { Weight::Light => self.light.clone(), Weight::Regular => self.regular.clone(), Weight::Medium => self.medium.clone(), Weight::Bold => self.bold.clone() }
    }
}

/// Altura de diseño: toda la UI y el campo se componen a 1080 px y se escalan.
pub const DESIGN_H: f32 = 1080.0;
