//! Efectos y animación. Todo es nativo de Bevy (nada de webview: `wry` solo incrusta en X11 y en
//! Wayland —Hyprland, niri— no funciona; además un juego de ritmo no debe pasar la entrada ni el
//! dibujo de las notas por un navegador).
//!
//! * La **UI se dibuja por debajo del mundo 2D**. Por eso los fondos (estrellas, luz ambiental) son
//!   nodos de UI en una capa trasera, y lo que debe alinearse con el campo (haces, ráfagas, combo)
//!   son entidades del mundo con su `z` explícito.
//! * Cada efecto respeta los ajustes: «Desactivar efectos visuales» lo apaga todo, «Reducir
//!   movimiento» deja las animaciones en su estado final, y fondo y resplandor tienen interruptor propio.

use crate::app::Screen;
use crate::settings::Settings;
use crate::theme::*;
use bevy::asset::RenderAssetUsages;
use bevy::color::Mix;
use bevy::ecs::hierarchy::ChildSpawnerCommands;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::picking::Pickable;
use bevy::post_process::bloom::{Bloom, BloomCompositeMode, BloomPrefilter};
use bevy::prelude::*;

// --- Curvas de animación ------------------------------------------------------------------------

pub fn ease_out_cubic(t: f32) -> f32 { 1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3) }
/// Sobrepasa un poco el destino y vuelve: da el «rebote» de los números que aparecen.
pub fn ease_out_back(t: f32) -> f32 {
    let (c1, c3) = (1.70158f32, 2.70158f32);
    let u = t.clamp(0.0, 1.0) - 1.0;
    1.0 + c3 * u.powi(3) + c1 * u.powi(2)
}
pub fn lerp(a: f32, b: f32, t: f32) -> f32 { a + (b - a) * t }

/// Pulso 1→0 que cae rápido al empezar cada pulso del compás. `phase` es la fracción de pulso (0..1).
pub fn beat_pulse(phase: f32) -> f32 { (1.0 - phase.rem_euclid(1.0)).powi(3) }

// --- Color de acento por canción ------------------------------------------------------------------

/// Color de acento estable para una canción: el matiz sale de su huella, la luminosidad es fija.
pub fn accent_for(seed: &str) -> Color {
    let mut h: u32 = 0x811c9dc5;
    for b in seed.bytes() { h ^= b as u32; h = h.wrapping_mul(16777619) }
    Color::hsl((h % 360) as f32, 0.62, 0.66)
}

/// Color de acento actual (se acerca suavemente al objetivo; lo usan fondo, barras y resultados).
#[derive(Resource)]
pub struct Accent { pub target: Color, pub current: Color }

impl Default for Accent {
    fn default() -> Self { let c = Color::srgb_u8(0x8a, 0x93, 0xb8); Accent { target: c, current: c } }
}

/// Intensidad del pulso al compás (0..1), la fija la partida cada fotograma.
#[derive(Resource, Default)]
pub struct Pulse(pub f32);

/// Qué tan presente debe estar el fondo (1 en menús; menos durante la partida).
#[derive(Resource)]
pub struct BackdropLevel { pub target: f32, pub current: f32 }
impl Default for BackdropLevel { fn default() -> Self { BackdropLevel { target: 1.0, current: 1.0 } } }

// --- Generador pseudoaleatorio (los efectos no necesitan reproducibilidad, sí rapidez) ----------------

#[derive(Resource)]
pub struct FxRng(pub u64);
impl Default for FxRng { fn default() -> Self { FxRng(0x9E3779B97F4A7C15) } }
impl FxRng {
    /// Valor en [0, 1).
    pub fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13; self.0 ^= self.0 >> 7; self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 { lo + (hi - lo) * self.next() }
}

// --- Mallas con color por vértice (degradados sin texturas ni shaders) ------------------------------

fn mesh_from(positions: Vec<[f32; 3]>, colors: Vec<[f32; 4]>, indices: Vec<u32>) -> Mesh {
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
        .with_inserted_indices(Indices::U32(indices))
}

/// Disco de radio 1: opaco en el centro y transparente en el borde (resplandor suave).
pub fn glow_mesh(segments: usize) -> Mesh {
    let mut pos = vec![[0.0, 0.0, 0.0]];
    let mut col = vec![[1.0, 1.0, 1.0, 1.0]];
    let mut idx = vec![];
    for i in 0..segments {
        let a = i as f32 / segments as f32 * std::f32::consts::TAU;
        pos.push([a.cos(), a.sin(), 0.0]);
        col.push([1.0, 1.0, 1.0, 0.0]);
        idx.extend([0, 1 + i as u32, 1 + ((i + 1) % segments) as u32]);
    }
    mesh_from(pos, col, idx)
}

/// Rectángulo de ancho 1 y alto 1 sobre el eje y: opaco abajo y transparente arriba (haz de luz).
pub fn fade_up_mesh() -> Mesh {
    mesh_from(
        vec![[-0.5, 0.0, 0.0], [0.5, 0.0, 0.0], [0.5, 1.0, 0.0], [-0.5, 1.0, 0.0]],
        vec![[1.0, 1.0, 1.0, 1.0], [1.0, 1.0, 1.0, 1.0], [1.0, 1.0, 1.0, 0.0], [1.0, 1.0, 1.0, 0.0]],
        vec![0, 1, 2, 0, 2, 3],
    )
}

/// Recursos gráficos compartidos de los efectos del mundo.
#[derive(Resource, Clone)]
pub struct Fx {
    pub circle: Handle<Mesh>,
    pub glow: Handle<Mesh>,
    pub fade_up: Handle<Mesh>,
    /// Blanco por encima de 1: con el resplandor activo, brilla.
    pub hot: Handle<ColorMaterial>,
}

fn setup_fx(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut mats: ResMut<Assets<ColorMaterial>>) {
    commands.insert_resource(Fx {
        circle: meshes.add(Circle::new(1.0).mesh().resolution(32)),
        glow: meshes.add(glow_mesh(48)),
        fade_up: meshes.add(fade_up_mesh()),
        hot: mats.add(Color::srgb(1.8, 1.8, 1.8)),
    });
}

// --- Portada procedural --------------------------------------------------------------------------------

/// Portada de una canción: degradado y tres círculos derivados de su huella. Es siempre la misma
/// para el mismo `seed`, así que sirve de identidad visual aunque no haya carátula real.
pub fn spawn_cover(parent: &mut ChildSpawnerCommands, seed: &str, size: f32) {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in seed.bytes() { h ^= b as u64; h = h.wrapping_mul(0x100000001b3) }
    let mut next = move || { h ^= h << 13; h ^= h >> 7; h ^= h << 17; (h >> 40) as f32 / (1u64 << 24) as f32 };
    let Hsla { hue, .. } = Hsla::from(accent_for(seed));
    let (base, deep) = (Color::hsl(hue, 0.55, 0.30), Color::hsl((hue + 42.0) % 360.0, 0.5, 0.09));
    parent.spawn((
        Node { width: px(size), height: px(size), flex_shrink: 0.0, overflow: Overflow::clip(), border_radius: BorderRadius::all(px(size * 0.1)), ..default() },
        BackgroundGradient::from(LinearGradient::to_bottom_right(vec![ColorStop::new(base, percent(0.0)), ColorStop::new(deep, percent(100.0))])),
    )).with_children(|c| {
        for i in 0..3 {
            let d = size * (0.45 + 0.5 * next());
            let (x, y) = (size * next() - d * 0.5, size * next() - d * 0.5);
            let col = Color::hsl((hue + 25.0 * i as f32 + 20.0 * next()) % 360.0, 0.6, 0.62);
            c.spawn((Node { position_type: PositionType::Absolute, left: px(x), top: px(y), width: px(d), height: px(d),
                            border: UiRect::all(px((size * 0.012).max(1.0))), border_radius: BorderRadius::MAX, ..default() },
                     BackgroundColor(col.with_alpha(0.16 + 0.12 * next())), BorderColor::all(col.with_alpha(0.55))));
        }
    });
}

// --- Partículas ------------------------------------------------------------------------------------

#[derive(Component)]
pub struct Particle { vel: Vec2, drag: f32, age: f32, life: f32, size: f32 }

/// Ráfaga de chispas que salen de `origin` y se apagan encogiéndose.
pub fn burst(commands: &mut Commands, screen: Screen, origin: Vec2, count: u32, speed: f32, size: f32, life: f32) {
    commands.queue(move |w: &mut World| {
        let fx = w.resource::<Fx>().clone();
        w.resource_scope(|w, mut rng: Mut<FxRng>| {
            for _ in 0..count {
                let a = rng.range(0.0, std::f32::consts::TAU);
                let v = rng.range(0.45, 1.0) * speed;
                w.spawn((DespawnOnExit(screen),
                    Particle { vel: Vec2::new(a.cos(), a.sin()) * v + Vec2::Y * speed * 0.25, drag: 3.2, age: 0.0, life: life * rng.range(0.7, 1.15), size: size * rng.range(0.6, 1.2) },
                    Mesh2d(fx.circle.clone()), MeshMaterial2d(fx.hot.clone()),
                    Transform::from_translation(origin.extend(6.0)).with_scale(Vec3::splat(size))));
            }
        });
    });
}

fn particles(mut commands: Commands, time: Res<Time>, mut q: Query<(Entity, &mut Particle, &mut Transform)>) {
    let dt = time.delta_secs();
    for (e, mut p, mut t) in &mut q {
        p.age += dt;
        let k = p.age / p.life;
        if k >= 1.0 { commands.entity(e).despawn(); continue }
        let (vel, drag) = (p.vel, p.drag);
        t.translation += (vel * dt).extend(0.0);
        p.vel *= (1.0 - drag * dt).max(0.0);
        let s = p.size * (1.0 - k).powf(1.4);
        t.scale = Vec3::new(s, s, 1.0);
    }
}

// --- Fondo vivo: luz ambiental y estrellas (capa de UI trasera) --------------------------------------

#[derive(Component)] struct Backdrop;
#[derive(Component)] struct AmbientGlow { which: usize }
#[derive(Component)] struct Star { phase: f32, speed: f32, sway: Vec2, base: f32 }

const STARS: usize = 72;

fn glow_gradient(c: Color, a: f32) -> BackgroundGradient {
    RadialGradient::new(UiPosition::CENTER, RadialGradientShape::ClosestSide,
        vec![ColorStop::percent(c.with_alpha(a), 0.0), ColorStop::percent(c.with_alpha(0.0), 100.0)]).into()
}

fn setup_backdrop(mut commands: Commands, accent: Res<Accent>) {
    let mut rng = FxRng(0xC0FFEE);
    commands.spawn((Backdrop, GlobalZIndex(-100), Pickable::IGNORE, ui_full())).with_children(|b| {
        for (which, (l, t, w, h)) in [(0usize, (-25.0, -35.0, 95.0, 110.0)), (1, (45.0, 35.0, 85.0, 100.0))] {
            b.spawn((AmbientGlow { which }, Pickable::IGNORE,
                Node { position_type: PositionType::Absolute, left: percent(l), top: percent(t), width: percent(w), height: percent(h), ..default() },
                glow_gradient(accent.current, 0.12)));
        }
        for _ in 0..STARS {
            let size = rng.range(1.6, 4.2);
            let base = rng.range(0.18, 0.6);
            b.spawn((Star { phase: rng.range(0.0, 6.28), speed: rng.range(0.4, 1.6), sway: Vec2::new(rng.range(6.0, 26.0), rng.range(4.0, 16.0)), base }, Pickable::IGNORE,
                Node { position_type: PositionType::Absolute, left: percent(rng.range(0.0, 100.0)), top: percent(rng.range(0.0, 100.0)),
                       width: px(size), height: px(size), border_radius: BorderRadius::MAX, ..default() },
                BackgroundColor(Color::srgba(1.0, 1.0, 1.0, base)), UiTransform::default()));
        }
    });
}

fn ui_full() -> Node {
    Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), overflow: Overflow::clip(), ..default() }
}

/// Acerca el acento actual al objetivo y mueve el nivel del fondo según la pantalla.
fn drive_backdrop(time: Res<Time>, screen: Res<State<Screen>>, mut accent: ResMut<Accent>, mut level: ResMut<BackdropLevel>) {
    let k = 1.0 - (-4.0 * time.delta_secs()).exp();
    let (cur, tgt) = (accent.current, accent.target);
    accent.current = cur.mix(&tgt, k);
    level.target = if *screen.get() == Screen::Playing { 0.2 } else { 1.0 };
    let (c, t) = (level.current, level.target);
    level.current = lerp(c, t, k);
}

#[allow(clippy::too_many_arguments)]
fn animate_backdrop(
    time: Res<Time>, settings: Res<Settings>, accent: Res<Accent>, level: Res<BackdropLevel>, pulse: Res<Pulse>,
    mut root: Query<&mut Node, With<Backdrop>>,
    mut glows: Query<(&AmbientGlow, &mut BackgroundGradient, &mut UiTransform), Without<Star>>,
    mut stars: Query<(&Star, &mut BackgroundColor, &mut UiTransform), Without<AmbientGlow>>,
) {
    let on = settings.background_fx();
    for mut n in &mut root { n.display = if on { Display::Flex } else { Display::None } }
    if !on { return }
    let t = time.elapsed_secs();
    let still = settings.motion_reduced();
    let lv = level.current;
    for (g, mut grad, mut tf) in &mut glows {
        let breathe = if still { 0.0 } else { (t * 0.7 + g.which as f32 * 2.0).sin() * 0.5 + 0.5 };
        let a = (0.075 + 0.04 * breathe + 0.07 * pulse.0) * lv;
        if let Some(Gradient::Radial(r)) = grad.0.first_mut() {
            r.stops = vec![ColorStop::percent(accent.current.with_alpha(a), 0.0), ColorStop::percent(accent.current.with_alpha(0.0), 100.0)];
        }
        tf.scale = Vec2::splat(1.0 + 0.04 * pulse.0 + 0.03 * breathe);
    }
    for (s, mut bg, mut tf) in &mut stars {
        let tw = if still { 1.0 } else { 0.55 + 0.45 * (t * s.speed + s.phase).sin() };
        bg.0 = Color::srgba(1.0, 1.0, 1.0, s.base * tw * lv);
        if !still { tf.translation = Val2::px((t * 0.12 * s.speed + s.phase).sin() * s.sway.x, (t * 0.1 * s.speed + s.phase * 1.7).cos() * s.sway.y) }
    }
}

// --- Fundido entre pantallas ----------------------------------------------------------------------

#[derive(Component)] struct FadeOverlay;
#[derive(Resource)] struct FadeProgress(f32);

fn setup_fade(mut commands: Commands) {
    commands.insert_resource(FadeProgress(0.0));
    commands.spawn((FadeOverlay, GlobalZIndex(200), Pickable::IGNORE, BackgroundColor(BG), Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() }));
}

fn restart_fade(mut p: ResMut<FadeProgress>) { p.0 = 0.0 }

fn fade(time: Res<Time>, settings: Res<Settings>, mut p: ResMut<FadeProgress>, mut q: Query<(&mut Node, &mut BackgroundColor), With<FadeOverlay>>) {
    p.0 = (p.0 + time.delta_secs() / 0.32).min(1.0);
    let a = if settings.motion_reduced() { 0.0 } else { 1.0 - ease_out_cubic(p.0) };
    for (mut n, mut bg) in &mut q {
        let d = if a > 0.004 { Display::Flex } else { Display::None };
        if n.display != d { n.display = d }
        bg.0 = BG.with_alpha(a);
    }
}

// --- Apariciones escalonadas ----------------------------------------------------------------------

/// Añadida a la raíz de un grupo de UI: aparece con desvanecido y deslizamiento tras `delay`.
#[derive(Component)]
pub struct Appear { delay: f32, dur: f32, offset: Vec2, age: f32, ready: bool }

/// Alfas de partida de un nodo animado (se restituyen al terminar).
#[derive(Component, Clone, Copy)]
struct BaseAlpha { text: f32, bg: f32, border: f32 }

/// Bundle de aparición: oculto hasta que el sistema registra los colores originales.
pub fn appear(delay: f32, offset: Vec2) -> impl Bundle {
    (Appear { delay, dur: 0.45, offset, age: 0.0, ready: false }, Visibility::Hidden, UiTransform::default())
}

#[allow(clippy::too_many_arguments)]
fn appear_system(
    mut commands: Commands, time: Res<Time>, settings: Res<Settings>,
    mut roots: Query<(Entity, &mut Appear, &mut Visibility, &mut UiTransform)>,
    kids: Query<&Children>,
    mut all: Query<(Option<&mut TextColor>, Option<&mut BackgroundColor>, Option<&mut BorderColor>, Option<&BaseAlpha>)>,
) {
    let dt = time.delta_secs();
    let skip = settings.motion_reduced();
    for (root, mut a, mut vis, mut tf) in &mut roots {
        let mut nodes = vec![root];
        nodes.extend(kids.iter_descendants(root));
        if !a.ready {
            for e in &nodes {
                if let Ok((t, b, bo, base)) = all.get(*e) {
                    if base.is_none() && (t.is_some() || b.is_some() || bo.is_some()) {
                        commands.entity(*e).insert(BaseAlpha {
                            text: t.map_or(1.0, |c| c.0.alpha()), bg: b.map_or(1.0, |c| c.0.alpha()), border: bo.map_or(1.0, |c| c.top.alpha()),
                        });
                    }
                }
            }
            a.ready = true;
            continue; // los `BaseAlpha` existen a partir del siguiente fotograma
        }
        a.age += dt;
        let f = if skip { 1.0 } else { ease_out_cubic((a.age - a.delay) / a.dur) };
        if *vis != Visibility::Inherited { *vis = Visibility::Inherited }
        let off = a.offset * (1.0 - f);
        tf.translation = Val2::px(off.x, off.y);
        for e in &nodes {
            if let Ok((t, b, bo, Some(base))) = all.get_mut(*e) {
                if let Some(mut t) = t { t.0 = t.0.with_alpha(base.text * f) }
                if let Some(mut b) = b { b.0 = b.0.with_alpha(base.bg * f) }
                if let Some(mut bo) = bo {
                    let b = &mut *bo;
                    for side in [&mut b.top, &mut b.right, &mut b.bottom, &mut b.left] { *side = side.with_alpha(base.border * f) }
                }
            }
        }
        if f >= 1.0 {
            for e in &nodes { commands.entity(*e).remove::<BaseAlpha>(); }
            commands.entity(root).remove::<Appear>();
            tf.translation = Val2::ZERO;
        }
    }
}

// --- Contadores animados --------------------------------------------------------------------------

/// Texto que cuenta de 0 al valor final.
#[derive(Component)]
pub struct CountUp { pub to: f64, pub delay: f32, pub dur: f32, pub age: f32, pub fmt: fn(f64) -> String }

impl CountUp {
    pub fn new(to: f64, delay: f32, dur: f32, fmt: fn(f64) -> String) -> Self { CountUp { to, delay, dur, age: 0.0, fmt } }
}

fn count_up(time: Res<Time>, settings: Res<Settings>, mut q: Query<(&mut CountUp, &mut Text)>) {
    for (mut c, mut t) in &mut q {
        c.age += time.delta_secs();
        let k = if settings.motion_reduced() { 1.0 } else { ease_out_cubic((c.age - c.delay) / c.dur) };
        let s = (c.fmt)(c.to * k as f64);
        if **t != s { **t = s }
    }
}

// --- Resplandor (bloom) -----------------------------------------------------------------------------

/// Parámetros del resplandor: solo brilla lo que supera 1,0 (notas y chispas con color «caliente»);
/// la interfaz, que nunca pasa de 1,0, no se ve afectada.
pub fn bloom_settings() -> Bloom {
    Bloom {
        intensity: 0.32, composite_mode: BloomCompositeMode::Additive,
        prefilter: BloomPrefilter { threshold: 1.0, threshold_softness: 0.5 },
        ..Bloom::NATURAL
    }
}

fn sync_glow(settings: Res<Settings>, cam: Query<(Entity, Has<Bloom>), With<Camera2d>>, mut commands: Commands) {
    for (e, has) in &cam {
        match (settings.glow_on(), has) {
            (true, false) => { commands.entity(e).insert(bloom_settings()); }
            (false, true) => { commands.entity(e).remove::<Bloom>(); }
            _ => {}
        }
    }
}

pub fn plugin(app: &mut App) {
    app.init_resource::<Accent>().init_resource::<Pulse>().init_resource::<BackdropLevel>().init_resource::<FxRng>()
        .add_systems(Startup, (setup_fx, setup_backdrop, setup_fade))
        .add_systems(Update, (particles, drive_backdrop, animate_backdrop.after(drive_backdrop), fade, count_up, sync_glow, restart_fade.run_if(state_changed::<Screen>)))
        .add_systems(Last, appear_system);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_curves_hit_their_endpoints_and_stay_in_range() {
        for f in [ease_out_cubic] {
            assert!(f(0.0).abs() < 1e-6 && (f(1.0) - 1.0).abs() < 1e-6);
            assert_eq!((f(-3.0), f(7.0)), (f(0.0), f(1.0)), "fuera de rango se recorta");
            let mut prev = -1.0;
            for i in 0..=20 { let v = f(i as f32 / 20.0); assert!(v >= prev, "monótona"); prev = v }
        }
        assert!((ease_out_back(1.0) - 1.0).abs() < 1e-5 && ease_out_back(0.0).abs() < 1e-5);
        assert!((0..=100).map(|i| ease_out_back(i as f32 / 100.0)).fold(0.0f32, f32::max) > 1.0, "sobrepasa y vuelve");
        assert!((lerp(2.0, 6.0, 0.25) - 3.0).abs() < 1e-6);
    }

    #[test]
    fn beat_pulse_is_sharp_at_the_beat_and_wraps() {
        assert!((beat_pulse(0.0) - 1.0).abs() < 1e-6);
        assert!(beat_pulse(0.5) < 0.13 && beat_pulse(0.99) < 1e-3);
        assert!((beat_pulse(2.0) - beat_pulse(0.0)).abs() < 1e-6, "periódica");
        assert!((beat_pulse(-0.25) - beat_pulse(0.75)).abs() < 1e-6, "fases negativas válidas");
    }

    #[test]
    fn accents_are_stable_and_varied() {
        let a = accent_for("4928ef5b2d3e462a");
        assert_eq!(a, accent_for("4928ef5b2d3e462a"));
        let hues: std::collections::HashSet<u32> = (0..40).map(|i| { let Hsla { hue, .. } = Hsla::from(accent_for(&format!("song{i}"))); (hue / 30.0) as u32 }).collect();
        assert!(hues.len() >= 6, "los acentos de canciones distintas deben repartirse por el círculo cromático: {hues:?}");
        let Hsla { lightness, saturation, .. } = Hsla::from(accent_for("x"));
        assert!((lightness - 0.66).abs() < 0.01 && (saturation - 0.62).abs() < 0.01, "luminosidad fija: siempre legible sobre fondo oscuro");
    }

    #[test]
    fn rng_is_in_range_and_not_constant() {
        let mut r = FxRng::default();
        let v: Vec<f32> = (0..1000).map(|_| r.next()).collect();
        assert!(v.iter().all(|x| (0.0..1.0).contains(x)));
        let mean = v.iter().sum::<f32>() / v.len() as f32;
        assert!((mean - 0.5).abs() < 0.06, "{mean}");
        assert!(v.windows(2).any(|w| w[0] != w[1]));
        assert!((0..100).all(|_| { let x = r.range(3.0, 4.0); (3.0..4.0).contains(&x) }));
    }

    #[test]
    fn gradient_meshes_have_matching_buffers() {
        let g = glow_mesh(24);
        assert_eq!(g.count_vertices(), 25);
        assert_eq!(g.indices().unwrap().len(), 24 * 3);
        let f = fade_up_mesh();
        assert_eq!(f.count_vertices(), 4);
        assert_eq!(f.indices().unwrap().len(), 6);
    }

    #[test]
    fn count_up_formats_progressively() {
        let fmt: fn(f64) -> String = |v| format!("{:07}", v.round() as u64);
        let mut c = CountUp::new(984_210.0, 0.2, 1.0, fmt);
        c.age = 0.2;
        assert_eq!(fmt(c.to * ease_out_cubic((c.age - c.delay) / c.dur) as f64), "0000000");
        c.age = 1.2;
        assert_eq!(fmt(c.to * ease_out_cubic((c.age - c.delay) / c.dur) as f64), "0984210");
    }
}
