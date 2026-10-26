//! Piezas de UI reutilizables y navegación de menús (teclado + ratón).

use crate::theme::*;
use bevy::{
    picking::hover::Hovered,
    prelude::*,
    ui_widgets::{observe, Activate, Button},
};

/// Cualquier cosa que el usuario pueda activar. `id` lo interpreta cada pantalla.
#[derive(Component)]
pub struct MenuItem { pub id: u32, pub order: usize }

/// Estado de navegación compartido. Cada pantalla consume `activated`.
#[derive(Resource, Default)]
pub struct MenuNav {
    pub selected: usize,
    pub count: usize,
    pub activated: Option<u32>,
    /// −1 / +1 cuando se pulsa izquierda/derecha sobre el elemento seleccionado.
    pub adjust: i8,
    pub enabled: bool,
}

impl MenuNav {
    pub fn reset(&mut self, count: usize) { self.selected = 0; self.count = count; self.activated = None; self.adjust = 0; self.enabled = true }
}

pub fn text(fonts: &Fonts, s: impl Into<String>, size: f32, color: Color, w: Weight) -> impl Bundle {
    (Text::new(s), TextFont { font: fonts.get(w).into(), font_size: FontSize::Px(size), ..default() }, TextColor(color))
}

/// Texto con espaciado de letras amplio (títulos).
pub fn spaced(fonts: &Fonts, s: impl Into<String>, size: f32, color: Color, w: Weight, spacing: f32) -> impl Bundle {
    (Text::new(s),
     TextFont { font: fonts.get(w).into(), font_size: FontSize::Px(size), letter_spacing: LetterSpacing::Px(spacing), ..default() },
     TextColor(color))
}

/// Elemento de menú: fila con marcador lateral; el texto cambia de brillo al seleccionarse.
pub fn menu_item(fonts: &Fonts, id: u32, order: usize, label: &str, sub: Option<&str>, size: f32) -> impl Bundle {
    (
        Button, Hovered::default(), MenuItem { id, order },
        Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(18), padding: UiRect::axes(px(8), px(10)), ..default() },
        observe(move |_: On<Activate>, mut nav: ResMut<MenuNav>| { nav.activated = Some(id); }),
        children![
            (Marker, Node { width: px(3), height: px(size * 0.9), ..default() }, BackgroundColor(Color::NONE)),
            (Node { flex_direction: FlexDirection::Column, ..default() },
             children![
                (ItemLabel, text(fonts, label, size, TEXT_DIM, Weight::Regular)),
                (ItemSub, text(fonts, sub.unwrap_or(""), size * 0.5, TEXT_DIM, Weight::Regular)),
             ]),
        ],
    )
}

#[derive(Component)] pub struct Marker;
#[derive(Component)] pub struct ItemLabel;
#[derive(Component)] pub struct ItemSub;

/// Teclado: ↑/↓/W/S mueven, ←/→ ajustan, Enter/Espacio activa. Ratón: hover selecciona, clic activa.
pub fn menu_input(
    keys: Res<ButtonInput<KeyCode>>,
    mut nav: ResMut<MenuNav>,
    hovered: Query<(&MenuItem, &Hovered), Changed<Hovered>>,
    items: Query<&MenuItem>,
) {
    nav.adjust = 0;
    if !nav.enabled || nav.count == 0 { return }
    nav.count = items.iter().count().max(nav.count.min(items.iter().count()));
    if keys.just_pressed(KeyCode::ArrowDown) || keys.just_pressed(KeyCode::KeyS) { nav.selected = (nav.selected + 1) % nav.count }
    if keys.just_pressed(KeyCode::ArrowUp) || keys.just_pressed(KeyCode::KeyW) { nav.selected = (nav.selected + nav.count - 1) % nav.count }
    if keys.just_pressed(KeyCode::ArrowLeft) { nav.adjust = -1 }
    if keys.just_pressed(KeyCode::ArrowRight) { nav.adjust = 1 }
    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter) || keys.just_pressed(KeyCode::Space) {
        if let Some(it) = items.iter().find(|i| i.order == nav.selected) { nav.activated = Some(it.id) }
    }
    for (it, h) in &hovered { if h.get() { nav.selected = it.order } }
}

/// Resalta el elemento seleccionado.
pub fn menu_visuals(
    nav: Res<MenuNav>,
    items: Query<(&MenuItem, &Children)>,
    kids: Query<&Children>,
    mut markers: Query<&mut BackgroundColor, With<Marker>>,
    mut labels: Query<&mut TextColor, (With<ItemLabel>, Without<ItemSub>)>,
    mut subs: Query<&mut TextColor, (With<ItemSub>, Without<ItemLabel>)>,
) {
    for (it, children) in &items {
        let sel = it.order == nav.selected;
        for c in children.iter() {
            if let Ok(mut m) = markers.get_mut(c) { m.0 = if sel { WHITE } else { Color::NONE } }
            if let Ok(inner) = kids.get(c) {
                for k in inner.iter() {
                    if let Ok(mut l) = labels.get_mut(k) { l.0 = if sel { WHITE } else { TEXT_DIM } }
                    if let Ok(mut l) = subs.get_mut(k) { l.0 = if sel { TEXT_DIM } else { alpha(TEXT_DIM, 0.6) } }
                }
            }
        }
    }
}

pub fn full_screen_root() -> Node {
    Node { width: percent(100), height: percent(100), position_type: PositionType::Absolute, ..default() }
}

/// Fina línea divisoria.
pub fn divider(width: Val) -> impl Bundle {
    (Node { width, height: px(1), ..default() }, BackgroundColor(BORDER))
}
