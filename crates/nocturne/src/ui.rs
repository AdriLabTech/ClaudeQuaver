//! Piezas de UI reutilizables y navegación de menús (teclado + ratón).

use crate::app::Screen;
use crate::theme::*;
use bevy::{
    ecs::hierarchy::ChildSpawnerCommands,
    picking::hover::Hovered,
    prelude::*,
    ui_widgets::{observe, Activate, Button},
};

pub fn plugin(app: &mut App) {
    app.init_resource::<Clicks>().init_resource::<MenuNav>()
        .add_systems(Update, (hover_tint, clear_clicks.run_if(state_changed::<Screen>)));
}

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
    /// Ajuste pedido con el ratón (flechas junto al valor); `menu_input` lo funde con el teclado.
    pub click_adjust: i8,
    pub enabled: bool,
}

impl MenuNav {
    pub fn reset(&mut self, count: usize) { self.selected = 0; self.count = count; self.activated = None; self.adjust = 0; self.click_adjust = 0; self.enabled = true }
}

/// Pulsaciones de botones sueltos (no son elementos de menú): cada pantalla consume los ids suyos.
#[derive(Resource, Default)]
pub struct Clicks(pub Vec<u32>);

impl Clicks {
    /// ¿Se pulsó el botón `id`? Lo consume.
    pub fn take(&mut self, id: u32) -> bool {
        match self.0.iter().position(|x| *x == id) { Some(p) => { self.0.remove(p); true } None => false }
    }
    /// Consume y devuelve el primer id del rango `[base, base + n)` como índice relativo.
    pub fn take_range(&mut self, base: u32, n: u32) -> Option<u32> {
        let p = self.0.iter().position(|x| *x >= base && *x < base + n)?;
        Some(self.0.remove(p) - base)
    }
}

fn clear_clicks(mut c: ResMut<Clicks>) { c.0.clear() }

/// Hace que el nodo sea un botón que registra `id` en [`Clicks`].
pub fn click(id: u32) -> impl Bundle {
    (Button, Hovered::default(), observe(move |_: On<Activate>, mut c: ResMut<Clicks>| c.0.push(id)))
}

/// Color de fondo que cambia al pasar el ratón.
#[derive(Component)]
pub struct HoverTint { pub normal: Color, pub hover: Color }

fn hover_tint(mut q: Query<(&Hovered, &HoverTint, &mut BackgroundColor), Changed<Hovered>>) {
    for (h, t, mut bg) in &mut q { bg.0 = if h.get() { t.hover } else { t.normal } }
}

/// Botón de texto pequeño (barras de acciones).
pub fn pill(fonts: &Fonts, id: u32, label: &str) -> impl Bundle {
    (
        click(id),
        Node { padding: UiRect::axes(px(16), px(8)), border: UiRect::all(px(1)), ..default() },
        BackgroundColor(Color::NONE), BorderColor::all(BORDER), HoverTint { normal: Color::NONE, hover: SURFACE_2 },
        children![text(fonts, label, 18.0, TEXT_DIM, Weight::Regular)],
    )
}

pub fn text(fonts: &Fonts, s: impl Into<String>, size: f32, color: Color, w: Weight) -> impl Bundle {
    (Text::new(s), TextFont { font: fonts.get(w).into(), font_size: FontSize::Px(size), ..default() }, TextColor(color))
}

/// Texto con espaciado de letras amplio (títulos).
pub fn spaced(fonts: &Fonts, s: impl Into<String>, size: f32, color: Color, w: Weight, spacing: f32) -> impl Bundle {
    (Text::new(s),
     TextFont { font: fonts.get(w).into(), font_size: FontSize::Px(size), ..default() },
     TextColor(color), bevy::text::LetterSpacing::Px(spacing))
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

/// Fila de ajustes: como [`menu_item`] pero a ancho completo y con un valor a la derecha.
pub fn menu_row(fonts: &Fonts, id: u32, order: usize, label: &str, value: &str, value_color: Color, size: f32) -> impl Bundle {
    (
        Button, Hovered::default(), MenuItem { id, order },
        Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, width: percent(100), column_gap: px(18), padding: UiRect::axes(px(8), px(10)), ..default() },
        observe(move |_: On<Activate>, mut nav: ResMut<MenuNav>| { nav.activated = Some(id); }),
        children![
            (Marker, Node { width: px(3), height: px(size * 0.9), ..default() }, BackgroundColor(Color::NONE)),
            (Node { flex_direction: FlexDirection::Column, ..default() },
             children![
                (ItemLabel, text(fonts, label, size, TEXT_DIM, Weight::Regular)),
                (ItemSub, text(fonts, "", size * 0.5, TEXT_DIM, Weight::Regular)),
             ]),
            Node { flex_grow: 1.0, ..default() },
            text(fonts, value, size * 0.92, value_color, Weight::Medium),
        ],
    )
}

#[derive(Component)] pub struct Marker;
#[derive(Component)] pub struct ItemLabel;
#[derive(Component)] pub struct ItemSub;

/// Repetición de teclas mantenidas (retardo 0,4 s y luego 20 pulsos/s).
#[derive(Default)]
pub struct KeyRepeat { held: Option<KeyCode>, t: f32 }

impl KeyRepeat {
    /// Devuelve la tecla de `candidates` que cuenta como pulsada en este fotograma.
    pub fn poll(&mut self, keys: &ButtonInput<KeyCode>, dt: f32, candidates: &[KeyCode]) -> Option<KeyCode> {
        for k in candidates {
            if keys.just_pressed(*k) { self.held = Some(*k); self.t = 0.0; return Some(*k) }
        }
        match self.held {
            Some(k) if keys.pressed(k) => {
                self.t += dt;
                if self.t >= 0.4 { self.t -= 0.05; return Some(k) }
                None
            }
            Some(_) => { self.held = None; None }
            None => None,
        }
    }
}

/// Teclado: ↑/↓/W/S mueven, ←/→ ajustan, Enter/Espacio activa. Ratón: hover selecciona, clic activa.
pub fn menu_input(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut nav: ResMut<MenuNav>,
    hovered: Query<(&MenuItem, &Hovered), Changed<Hovered>>,
    items: Query<&MenuItem>,
    mut vertical: Local<KeyRepeat>,
    mut horizontal: Local<KeyRepeat>,
) {
    nav.adjust = std::mem::take(&mut nav.click_adjust);
    if !nav.enabled || nav.count == 0 { return }
    let n = items.iter().count();
    if n == 0 { return }
    nav.count = n;
    nav.selected = nav.selected.min(n - 1);
    let dt = time.delta_secs();
    match vertical.poll(&keys, dt, &[KeyCode::ArrowDown, KeyCode::KeyS, KeyCode::ArrowUp, KeyCode::KeyW]) {
        Some(KeyCode::ArrowDown | KeyCode::KeyS) => nav.selected = (nav.selected + 1) % n,
        Some(KeyCode::ArrowUp | KeyCode::KeyW) => nav.selected = (nav.selected + n - 1) % n,
        _ => {}
    }
    match horizontal.poll(&keys, dt, &[KeyCode::ArrowLeft, KeyCode::ArrowRight]) {
        Some(KeyCode::ArrowLeft) => nav.adjust = -1,
        Some(KeyCode::ArrowRight) => nav.adjust = 1,
        _ => {}
    }
    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter) || keys.just_pressed(KeyCode::Space) {
        if let Some(it) = items.iter().find(|i| i.order == nav.selected) { nav.activated = Some(it.id) }
    }
    for (it, h) in &hovered { if h.get() { nav.selected = it.order } }
}

/// Resalta el elemento seleccionado.
pub fn menu_visuals(
    nav: Res<MenuNav>,
    time: Res<Time>,
    mut slide: Query<(&MenuItem, &mut UiTransform), Without<crate::fx::Appear>>,
    items: Query<(&MenuItem, &Children)>,
    kids: Query<&Children>,
    mut markers: Query<&mut BackgroundColor, With<Marker>>,
    mut labels: Query<&mut TextColor, (With<ItemLabel>, Without<ItemSub>)>,
    mut subs: Query<&mut TextColor, (With<ItemSub>, Without<ItemLabel>)>,
) {
    // El elemento elegido se desplaza unos píxeles a la derecha, con suavidad.
    let k = 1.0 - (-16.0 * time.delta_secs()).exp();
    for (it, mut tf) in &mut slide {
        let cur = if let Val::Px(x) = tf.translation.x { x } else { 0.0 };
        let target = if it.order == nav.selected { 10.0 } else { 0.0 };
        let next = cur + (target - cur) * k;
        if (next - cur).abs() > 0.01 { tf.translation = Val2::px(next, 0.0) }
    }
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

/// Separador vertical vacío.
pub fn gap(h: f32) -> impl Bundle { Node { height: px(h), ..default() } }

/// Histograma de errores temporales (30 casillas de 10 ms, −150…+150 ms) con su eje.
pub fn spawn_histogram(parent: &mut ChildSpawnerCommands, fonts: &Fonts, hist: &[u32], width: f32, height: f32) {
    let hmax = (*hist.iter().max().unwrap_or(&1)).max(1) as f32;
    let n = hist.len().max(1) as f32;
    let bar_w = (width / n - 2.0).max(2.0);
    parent.spawn(Node { height: px(height), width: px(width), align_items: AlignItems::FlexEnd, column_gap: px(2), margin: UiRect::top(px(10)), ..default() })
        .with_children(|h| {
            for (i, v) in hist.iter().enumerate() {
                let center = i == 14 || i == 15;
                h.spawn((Node { width: px(bar_w), height: px((*v as f32 / hmax * (height - 10.0)).max(2.0)), ..default() },
                    BackgroundColor(if center { WHITE } else if *v == 0 { BORDER } else { TEXT_DIM })));
            }
        });
    parent.spawn(Node { width: px(width), justify_content: JustifyContent::SpaceBetween, ..default() }).with_children(|a| {
        a.spawn(text(fonts, "−150 ms", 14.0, TEXT_DIM, Weight::Regular));
        a.spawn(text(fonts, "0", 14.0, TEXT_DIM, Weight::Regular));
        a.spawn(text(fonts, "+150 ms", 14.0, TEXT_DIM, Weight::Regular));
    });
}

/// `m:ss` a partir de milisegundos.
pub fn fmt_duration(ms: i64) -> String {
    let s = (ms.max(0) / 1000) as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// Recorta un texto largo con «…» (por caracteres, no por bytes).
pub fn ellipsize(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_string() } else { format!("{}…", s.chars().take(max.saturating_sub(1)).collect::<String>()) }
}

/// Fondo oscuro a pantalla completa para diálogos modales.
pub fn modal_backdrop() -> impl Bundle {
    (GlobalZIndex(50), BackgroundColor(alpha(BG, 0.97)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn durations_and_ellipsis() {
        assert_eq!(fmt_duration(0), "0:00");
        assert_eq!(fmt_duration(222_000), "3:42");
        assert_eq!(fmt_duration(3_725_000), "1:02:05");
        assert_eq!(fmt_duration(-5), "0:00");
        assert_eq!(ellipsize("corto", 10), "corto");
        assert_eq!(ellipsize("ñandú canción larga", 8), "ñandú c…");
    }
    #[test]
    fn clicks_are_consumed_once() {
        let mut c = Clicks(vec![5, 105, 7]);
        assert!(c.take(5) && !c.take(5));
        assert_eq!(c.take_range(100, 10), Some(5));
        assert_eq!(c.take_range(100, 10), None);
        assert!(c.take(7));
    }
}
