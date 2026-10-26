//! Pantalla de ajustes: cinco secciones; cada cambio se aplica y se guarda al instante
//! (escritura atómica). Teclado: ↑↓ elegir, ←→ ajustar (Mayús = más rápido), Intro activar,
//! Q/E cambian de sección. Ratón: pasar por encima selecciona, clic activa, rueda ajusta.

use crate::app::Screen;
use crate::audio::{list_output_devices, Audio};
use crate::jobs::{Jobs, NoticeKind};
use crate::paths;
use crate::settings::{duplicate_key, is_bindable, key_label, KeyProfile, Settings};
use crate::theme::*;
use crate::ui::{self, Clicks, MenuNav};
use bevy::ecs::hierarchy::ChildSpawnerCommands;
use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;
use nocturne_core::scoring::HitWindows;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Field {
    ScrollSpeed, NoteScale, LaneGap, ReceptorBorder, HitLine, MinimalVisuals, Precision, NoFail, ShowJudgement, ShowEffects, ShowProgress, ShowLife,
    Profile, Key(usize), NewProfile, DeleteProfile, ResetProfile,
    MusicVolume, SfxVolume, Device, Offset, Calibrate,
    Fullscreen, Vsync, FpsLimit, InterfaceScale, ReduceMotion, Diagnostics,
    Sensitivity, Holds, Density,
    ResetAll,
}

use Field::*;

const PAGES: [(&str, &[Field]); 5] = [
    ("Juego", &[ScrollSpeed, NoteScale, LaneGap, ReceptorBorder, HitLine, MinimalVisuals, Precision, NoFail, ShowJudgement, ShowEffects, ShowProgress, ShowLife, ResetAll]),
    ("Controles", &[Profile, Key(0), Key(1), Key(2), Key(3), NewProfile, DeleteProfile, ResetProfile, ResetAll]),
    ("Audio", &[MusicVolume, SfxVolume, Device, Offset, Calibrate, ResetAll]),
    ("Gráficos", &[MinimalVisuals, Fullscreen, Vsync, FpsLimit, InterfaceScale, ReduceMotion, Diagnostics, ResetAll]),
    ("Análisis", &[Sensitivity, Holds, Density, ResetAll]),
];

const TAB_BASE: u32 = 100;
const MAX_PROFILES: usize = 8;
const PRECISION_PRESETS: [(&str, f64); 3] = [("Estricta", 0.75), ("Estándar", 1.0), ("Indulgente", 1.3)];

#[derive(Resource, Default)]
struct SettingsUi {
    page: usize,
    rebinding: Option<usize>,
    devices: Vec<String>,
    confirm_reset: bool,
    dirty: bool,
}

#[derive(Component)] struct SettingsRoot;
#[derive(Component)] struct HintText;

pub fn plugin(app: &mut App) {
    app.init_resource::<SettingsUi>()
        .add_systems(OnEnter(Screen::Settings), enter)
        .add_systems(Update, (ui::menu_input, input, render, update_hint, ui::menu_visuals).chain().run_if(in_state(Screen::Settings)));
}

fn enter(mut st: ResMut<SettingsUi>, mut nav: ResMut<MenuNav>) {
    st.page = 0;
    st.rebinding = None;
    st.confirm_reset = false;
    st.devices = list_output_devices();
    st.dirty = true;
    nav.reset(PAGES[0].1.len());
}

fn label(f: Field) -> &'static str {
    match f {
        ScrollSpeed => "Velocidad de las notas", NoteScale => "Tamaño de las notas", LaneGap => "Separación entre carriles",
        ReceptorBorder => "Borde de los círculos fijos", MinimalVisuals => "Desactivar efectos visuales", HitLine => "Altura de los círculos fijos",
        Precision => "Precisión exigida", NoFail => "Sin fallo", ShowJudgement => "Mostrar juicios", ShowEffects => "Efectos al acertar",
        ShowProgress => "Barra de progreso", ShowLife => "Barra de vida",
        Profile => "Perfil de teclas", Key(0) => "Carril 1", Key(1) => "Carril 2", Key(2) => "Carril 3", Key(_) => "Carril 4",
        NewProfile => "Crear perfil nuevo", DeleteProfile => "Eliminar este perfil", ResetProfile => "Restaurar teclas estándar",
        MusicVolume => "Volumen de la música", SfxVolume => "Volumen de los efectos", Device => "Dispositivo de salida",
        Offset => "Compensación de audio", Calibrate => "Calibrar con metrónomo",
        Fullscreen => "Pantalla completa", Vsync => "Sincronización vertical", FpsLimit => "Límite de FPS", InterfaceScale => "Escala de la interfaz",
        ReduceMotion => "Reducir movimiento", Diagnostics => "Mostrar diagnóstico",
        Sensitivity => "Sensibilidad de detección", Holds => "Generar notas largas", Density => "Densidad de notas",
        ResetAll => "Restablecer todos los ajustes",
    }
}

fn hint(f: Field) -> &'static str {
    match f {
        ScrollSpeed => "Cuánto tardan las notas en llegar a la línea. Más alto = más rápido y más espacio entre notas.",
        NoteScale => "Tamaño de las notas y de los círculos fijos. No cambia la dificultad.",
        LaneGap => "Espacio libre entre las notas de dos carriles contiguos. 0 = se tocan.",
        MinimalVisuals => "Modo competitivo: sin anillos al acertar, sin resalte al pulsar, sin desvanecidos ni animaciones. Los juicios, el combo y la vida siguen mostrándose según sus ajustes.",
        ReceptorBorder => "Grosor del borde de los círculos fijos donde hay que pulsar.",
        HitLine => "Distancia de los círculos fijos al borde inferior de la pantalla.",
        Precision => "Ancho de las ventanas de acierto. Estricta las reduce un 25 %; Indulgente las amplía un 30 %.",
        NoFail => "La partida no termina aunque la vida llegue a cero.",
        ShowJudgement => "Muestra Marvelous, Perfect, Great… sobre el campo.",
        ShowEffects => "Anillo al acertar una nota. Lo anula «Desactivar efectos visuales».",
        ShowProgress => "Línea fina de avance en el borde superior.", ShowLife => "Barra de vida junto al campo.",
        Profile => "← → cambia de perfil. Cada perfil guarda sus cuatro teclas.",
        Key(_) => "Intro y pulsa la nueva tecla. Si ya la usa otro carril, se intercambian. Esc cancela.",
        NewProfile => "Copia las teclas del perfil actual en un perfil nuevo.",
        DeleteProfile => "Elimina el perfil activo (siempre queda al menos uno).",
        ResetProfile => "Vuelve a D F J K en el perfil activo.",
        MusicVolume | SfxVolume => "Se aplica al momento.",
        Device => "← → recorre las salidas de audio disponibles. «Predeterminado» sigue el sistema.",
        Offset => "Positivo = las notas llegan más tarde. Si sueles pulsar tarde, súbelo. Mayús ajusta de 5 en 5.",
        Calibrate => "Pulsa al ritmo de un metrónomo y Nocturne calcula la compensación por ti.",
        Fullscreen => "También con F11.", Vsync => "Evita desgarros; puede añadir algo de latencia visual.",
        FpsLimit => "Limita los fotogramas por segundo (0 = sin límite).",
        InterfaceScale => "Tamaño de menús y textos.", ReduceMotion => "Quita giros y desvanecidos largos.",
        Diagnostics => "FPS y latencia de audio en una esquina.",
        Sensitivity => "Más alto detecta ataques más débiles. Solo afecta a los análisis nuevos (usa Reanalizar en la biblioteca).",
        Holds => "Permite notas largas donde la música sostiene el sonido. Afecta a los análisis nuevos.",
        Density => "Multiplica el límite de notas por segundo de cada dificultad. Afecta a los análisis nuevos.",
        ResetAll => "Vuelve a los valores de fábrica. Pide confirmación.",
    }
}

fn precision_windows(k: f64) -> HitWindows {
    let d = HitWindows::default();
    HitWindows { marvelous_ms: d.marvelous_ms * k, perfect_ms: d.perfect_ms * k, great_ms: d.great_ms * k, good_ms: d.good_ms * k, bad_ms: d.bad_ms * k }
}

fn precision_index(w: &HitWindows) -> Option<usize> {
    PRECISION_PRESETS.iter().position(|(_, k)| {
        let p = precision_windows(*k);
        (p.marvelous_ms - w.marvelous_ms).abs() < 0.01 && (p.perfect_ms - w.perfect_ms).abs() < 0.01 && (p.bad_ms - w.bad_ms).abs() < 0.01
    })
}

const FPS_STEPS: [u32; 7] = [0, 30, 60, 120, 144, 240, 360];

fn value(s: &Settings, f: Field, st: &SettingsUi) -> String {
    let on = |b: bool| if b { "Activado" } else { "Desactivado" }.to_string();
    match f {
        ScrollSpeed => format!("{:.0}", s.scroll_speed),
        NoteScale => format!("{:.2}×", s.note_scale),
        LaneGap => format!("{:.0} px", s.lane_gap),
        ReceptorBorder => format!("{:.1} px", s.receptor_border),
        HitLine => format!("{:.0} px", s.hit_line_height),
        Precision => precision_index(&s.hit_windows).map_or("Personalizada", |i| PRECISION_PRESETS[i].0).to_string(),
        NoFail => on(s.no_fail), ShowJudgement => on(s.show_judgement), ShowEffects => on(s.hit_effects()),
        MinimalVisuals => on(s.minimal_visuals),
        ShowProgress => on(s.show_progress), ShowLife => on(s.show_life),
        Profile => format!("{} ({}/{})", s.profiles[s.active_profile].name, s.active_profile + 1, s.profiles.len()),
        Key(i) => if st.rebinding == Some(i) { "Pulsa una tecla…".into() } else { key_label(s.keys()[i]) },
        NewProfile | DeleteProfile | ResetProfile | Calibrate => String::new(),
        MusicVolume => format!("{:.0} %", s.music_volume * 100.0),
        SfxVolume => format!("{:.0} %", s.sfx_volume * 100.0),
        Device => s.audio_device.clone().map_or("Predeterminado".into(), |d| ui::ellipsize(&d, 34)),
        Offset => format!("{:+.0} ms", s.global_offset_ms),
        Fullscreen => on(s.fullscreen), Vsync => on(s.vsync),
        FpsLimit => if s.fps_limit == 0 { "Sin límite".into() } else { s.fps_limit.to_string() },
        InterfaceScale => format!("{:.0} %", s.ui_scale * 100.0),
        ReduceMotion => on(s.reduce_motion), Diagnostics => on(s.show_diagnostics),
        Sensitivity => format!("{:.0} %", s.onset_sensitivity * 100.0),
        Holds => on(s.generate_holds),
        Density => format!("{:.2}×", s.density_scale),
        ResetAll => if st.confirm_reset { "Pulsa de nuevo para confirmar".into() } else { String::new() },
    }
}

/// Mueve `v` un paso en la rejilla de `step`, dentro de `[lo, hi]`, sin acumular error de coma flotante.
fn bump(v: &mut f32, dir: i8, step: f32, lo: f32, hi: f32) {
    let n = (*v / step).round() + dir as f32;
    *v = ((n * step).clamp(lo, hi) * 1000.0).round() / 1000.0;
}

/// Como [`bump`], pero activar (Intro o clic) en el máximo da la vuelta al mínimo.
fn slide(v: &mut f32, dir: i8, activate: bool, step: f32, lo: f32, hi: f32) {
    let before = *v;
    bump(v, dir, step, lo, hi);
    if activate && *v == before { *v = lo }
}

/// Qué hay que hacer fuera de `Settings` tras un cambio.
#[derive(Default, PartialEq, Debug)]
struct Effect { volume: bool, device: bool, preview: bool, calibrate: bool }

/// Cambia el campo `f` (`dir` = ±1; `activate` = Intro o clic).
fn change(s: &mut Settings, st: &mut SettingsUi, f: Field, dir: i8, fast: bool, activate: bool) -> Effect {
    let mut fx = Effect::default();
    let toggle = |b: &mut bool| *b = !*b;
    // Pulsar Intro o hacer clic en un control numérico avanza un paso (y da la vuelta al llegar al final).
    let dir = if activate && dir == 0 { 1 } else { dir };
    if f != ResetAll { st.confirm_reset = false }
    match f {
        ScrollSpeed => slide(&mut s.scroll_speed, dir, activate, 1.0, 1.0, 40.0),
        NoteScale => slide(&mut s.note_scale, dir, activate, 0.05, 0.6, 2.0),
        LaneGap => slide(&mut s.lane_gap, dir, activate, 1.0, 0.0, 80.0),
        ReceptorBorder => slide(&mut s.receptor_border, dir, activate, 0.5, 1.0, 16.0),
        HitLine => slide(&mut s.hit_line_height, dir, activate, 10.0, 60.0, 400.0),
        Precision => {
            let i = precision_index(&s.hit_windows).unwrap_or(1) as i8;
            let n = if activate { (i + 1) % PRECISION_PRESETS.len() as i8 } else { (i + dir).clamp(0, PRECISION_PRESETS.len() as i8 - 1) };
            s.hit_windows = precision_windows(PRECISION_PRESETS[n as usize].1);
        }
        NoFail => toggle(&mut s.no_fail), ShowJudgement => toggle(&mut s.show_judgement), ShowEffects => toggle(&mut s.show_hit_effects),
        MinimalVisuals => toggle(&mut s.minimal_visuals),
        ShowProgress => toggle(&mut s.show_progress), ShowLife => toggle(&mut s.show_life),
        Profile => {
            let n = s.profiles.len() as i32;
            s.active_profile = (s.active_profile as i32 + if dir == 0 { 1 } else { dir as i32 }).rem_euclid(n) as usize;
        }
        Key(i) => { if activate { st.rebinding = Some(i) } }
        NewProfile => if activate {
            if s.profiles.len() < MAX_PROFILES {
                let keys = s.keys();
                s.profiles.push(KeyProfile { name: format!("Perfil {}", s.profiles.len() + 1), keys });
                s.active_profile = s.profiles.len() - 1;
            }
        },
        DeleteProfile => if activate && s.profiles.len() > 1 {
            s.profiles.remove(s.active_profile);
            s.active_profile = s.active_profile.min(s.profiles.len() - 1);
        },
        ResetProfile => if activate { s.profiles[s.active_profile].keys = Settings::default().profiles[0].keys },
        MusicVolume => { slide(&mut s.music_volume, dir, activate, 0.05, 0.0, 1.0); fx.volume = true }
        SfxVolume => { slide(&mut s.sfx_volume, dir, activate, 0.05, 0.0, 1.0); fx.volume = true; fx.preview = true }
        Device => {
            let n = st.devices.len() as i32 + 1;
            let cur = s.audio_device.as_ref().and_then(|d| st.devices.iter().position(|x| x == d)).map_or(0, |i| i as i32 + 1);
            let next = (cur + if dir == 0 { 1 } else { dir as i32 }).rem_euclid(n);
            s.audio_device = if next == 0 { None } else { Some(st.devices[next as usize - 1].clone()) };
            fx.device = true;
        }
        Offset => {
            let step = if fast { 5.0 } else { 1.0 };
            s.global_offset_ms = (s.global_offset_ms + dir as f32 * step).clamp(-300.0, 300.0).round();
        }
        Calibrate => if activate { fx.calibrate = true },
        Fullscreen => toggle(&mut s.fullscreen), Vsync => toggle(&mut s.vsync),
        FpsLimit => {
            let i = FPS_STEPS.iter().position(|v| *v == s.fps_limit).unwrap_or(0) as i32;
            s.fps_limit = FPS_STEPS[(i + if dir == 0 { 1 } else { dir as i32 }).rem_euclid(FPS_STEPS.len() as i32) as usize];
        }
        InterfaceScale => slide(&mut s.ui_scale, dir, activate, 0.05, 0.7, 1.6),
        ReduceMotion => toggle(&mut s.reduce_motion), Diagnostics => toggle(&mut s.show_diagnostics),
        Sensitivity => slide(&mut s.onset_sensitivity, dir, activate, 0.05, 0.0, 1.0),
        Holds => toggle(&mut s.generate_holds),
        Density => slide(&mut s.density_scale, dir, activate, 0.05, 0.5, 3.0),
        ResetAll => if activate {
            if st.confirm_reset {
                let device = s.audio_device.clone();
                *s = Settings::default();
                s.audio_device = device;
                st.confirm_reset = false;
                fx.volume = true;
            } else { st.confirm_reset = true }
        },
    }
    fx
}

/// Asigna `key` al carril `lane` del perfil activo; si otro carril ya la tenía, se intercambian.
fn bind_key(s: &mut Settings, lane: usize, key: KeyCode) -> Result<(), &'static str> {
    if !is_bindable(key) { return Err("Esa tecla está reservada para la interfaz") }
    let p = s.active_profile;
    let mut keys = s.profiles[p].keys;
    if let Some(other) = keys.iter().position(|k| *k == key) { keys[other] = keys[lane] }
    keys[lane] = key;
    debug_assert!(duplicate_key(&keys).is_none());
    s.profiles[p].keys = keys;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn input(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut wheel: MessageReader<MouseWheel>,
    mut nav: ResMut<MenuNav>,
    mut clicks: ResMut<Clicks>,
    mut st: ResMut<SettingsUi>,
    mut settings: ResMut<Settings>,
    mut audio: ResMut<Audio>,
    mut jobs: ResMut<Jobs>,
    mut next: ResMut<NextState<Screen>>,
) {
    let fields = PAGES[st.page].1;
    let fast = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);

    // Captura de tecla para remapear: lo primero, para que Esc cancele en vez de salir.
    if let Some(lane) = st.rebinding {
        nav.enabled = false;
        let Some(key) = keys.get_just_pressed().next().copied() else { return };
        st.rebinding = None;
        nav.enabled = true;
        st.dirty = true;
        if key == KeyCode::Escape { return }
        let mut tmp = settings.clone();
        match bind_key(&mut tmp, lane, key) {
            Ok(()) => { *settings = tmp; save(&settings, &mut jobs) }
            Err(e) => jobs.notice(e, NoticeKind::Error),
        }
        return;
    }

    if keys.just_pressed(KeyCode::Escape) { next.set(Screen::Home); return }
    let mut page = st.page as i32;
    if keys.just_pressed(KeyCode::KeyQ) || (keys.just_pressed(KeyCode::Tab) && fast) { page -= 1 }
    if keys.just_pressed(KeyCode::KeyE) || (keys.just_pressed(KeyCode::Tab) && !fast) { page += 1 }
    if let Some(i) = clicks.take_range(TAB_BASE, PAGES.len() as u32) { page = i as i32 }
    let page = page.rem_euclid(PAGES.len() as i32) as usize;
    if page != st.page {
        st.page = page;
        st.confirm_reset = false;
        st.dirty = true;
        nav.reset(PAGES[page].1.len());
        return;
    }

    // Cambios pedidos este fotograma: (carril de la lista, dirección, activar).
    let mut todo: Vec<(Field, i8, bool)> = vec![];
    let sel = nav.selected.min(fields.len() - 1);
    if nav.adjust != 0 { todo.push((fields[sel], nav.adjust, false)) }
    for w in wheel.read() { if w.y != 0.0 { todo.push((fields[sel], if w.y > 0.0 { 1 } else { -1 }, false)) } }
    if mouse.just_pressed(MouseButton::Right) { todo.push((fields[sel], -1, false)) }
    if let Some(id) = nav.activated.take() { if let Some(f) = fields.get(id as usize) { todo.push((*f, 0, true)) } }
    if todo.is_empty() { return }

    let mut effect = Effect::default();
    for (f, dir, activate) in todo {
        let fx = change(&mut settings, &mut st, f, dir, fast, activate);
        effect.volume |= fx.volume; effect.device |= fx.device; effect.preview |= fx.preview; effect.calibrate |= fx.calibrate;
    }
    st.dirty = true;
    if effect.volume { audio.set_volume(settings.music_volume, settings.sfx_volume) }
    if effect.preview { audio.hit_sound() }
    if effect.device {
        audio.reopen(settings.audio_device.as_deref());
        audio.set_volume(settings.music_volume, settings.sfx_volume);
        match &audio.device_error {
            Some(e) => jobs.notice(format!("No se pudo abrir esa salida de audio: {e}"), NoticeKind::Error),
            None => if let Some(i) = audio.device_info() { jobs.notice(format!("Salida de audio: {} · {} Hz", i.name, i.sample_rate), NoticeKind::Ok) },
        }
    }
    save(&settings, &mut jobs);
    if effect.calibrate { next.set(Screen::Calibrate) }
}

fn save(s: &Settings, jobs: &mut Jobs) {
    if let Err(e) = s.save(&paths::settings_path()) { jobs.notice(format!("No se pudieron guardar los ajustes: {e}"), NoticeKind::Error) }
}

/// Texto de ayuda de la fila seleccionada (y estado del dispositivo de audio en esa fila).
fn hint_text(f: Field, audio: &Audio) -> String {
    let mut t = hint(f).to_string();
    if f == Device {
        match audio.device_info() {
            Some(i) => t.push_str(&format!("\n\nEn uso: {} · {} Hz · {} ms de latencia", ui::ellipsize(&i.name, 30), i.sample_rate, audio.latency_ms().round())),
            None => t.push_str("\n\nSin dispositivo de audio: se usa un reloj silencioso."),
        }
    }
    t
}

fn update_hint(st: Res<SettingsUi>, nav: Res<MenuNav>, audio: Res<Audio>, mut q: Query<&mut Text, With<HintText>>) {
    let fields = PAGES[st.page].1;
    let t = hint_text(fields[nav.selected.min(fields.len() - 1)], &audio);
    for mut text in &mut q { if **text != t { **text = t.clone() } }
}

fn render(mut commands: Commands, mut st: ResMut<SettingsUi>, settings: Res<Settings>, nav: Res<MenuNav>, fonts: Res<Fonts>,
          audio: Res<Audio>, roots: Query<Entity, With<SettingsRoot>>) {
    if !st.dirty && !roots.is_empty() { return }
    st.dirty = false;
    for r in &roots { commands.entity(r).despawn() }
    let fields = PAGES[st.page].1;
    let sel = nav.selected.min(fields.len() - 1);
    commands.spawn((DespawnOnExit(Screen::Settings), SettingsRoot,
        Node { flex_direction: FlexDirection::Column, padding: UiRect::new(px(96), px(96), px(64), px(40)), row_gap: px(26), ..ui::full_screen_root() }))
        .with_children(|root| {
            root.spawn(ui::spaced(&fonts, "AJUSTES", 38.0, TEXT, Weight::Light, 10.0));
            root.spawn(Node { column_gap: px(36), ..default() }).with_children(|tabs| {
                for (i, (name, _)) in PAGES.iter().enumerate() {
                    let active = i == st.page;
                    tabs.spawn((ui::click(TAB_BASE + i as u32),
                        Node { padding: UiRect::axes(px(4), px(8)), border: UiRect::bottom(px(2)), ..default() },
                        BorderColor::all(if active { WHITE } else { Color::NONE })))
                        .with_children(|t| { t.spawn(ui::text(&fonts, *name, 24.0, if active { WHITE } else { TEXT_DIM }, Weight::Regular)); });
                }
                tabs.spawn(Node { align_self: AlignSelf::Center, margin: UiRect::left(px(24)), ..default() })
                    .with_children(|t| { t.spawn(ui::text(&fonts, "Q / E cambian de sección", 16.0, alpha(TEXT_DIM, 0.7), Weight::Regular)); });
            });
            root.spawn(Node { column_gap: px(80), flex_grow: 1.0, ..default() }).with_children(|body| {
                body.spawn(Node { flex_direction: FlexDirection::Column, width: px(1020), row_gap: px(2), ..default() }).with_children(|list| {
                    for (i, f) in fields.iter().enumerate() {
                        let v = value(&settings, *f, &st);
                        let rebinding = matches!(f, Key(l) if st.rebinding == Some(*l));
                        list.spawn(ui::menu_row(&fonts, i as u32, i, label(*f), &v, if rebinding { WHITE } else { TEXT }, 26.0));
                    }
                });
                body.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(16), width: px(520), padding: UiRect::top(px(10)), ..default() }).with_children(|side| {
                    side.spawn((HintText, ui::text(&fonts, hint_text(fields[sel], &audio), 20.0, TEXT_DIM, Weight::Regular)));
                    for w in &settings.warnings { side.spawn(ui::text(&fonts, format!("Aviso: {w}"), 16.0, DANGER, Weight::Regular)); }
                    if st.page == 0 { field_preview(side, &settings) }
                });
            });
            root.spawn(ui::divider(percent(100)));
            root.spawn(ui::text(&fonts, "↑↓ elegir   ←→ ajustar (Mayús: más rápido)   Intro activar   rueda del ratón ajusta   Esc volver", 16.0, alpha(TEXT_DIM, 0.8), Weight::Regular));
        });
}

/// Vista previa de la pista con el tamaño, la separación y el borde actuales: cuatro círculos
/// fijos y unas notas blancas, a escala 1:1 con la partida (reducida solo si no cabe).
fn field_preview(parent: &mut ChildSpawnerCommands, s: &Settings) {
    let (r, pitch) = (s.note_radius(), s.lane_pitch());
    let total = 3.0 * pitch + 2.0 * r;
    let k = (500.0 / total).min(1.0);
    let (w, h) = (total * k, 330.0 * k.max(0.7));
    let ring_y = h - r * k - 4.0;
    let disc = |x: f32, y: f32, ring: bool| {
        let d = 2.0 * r * k;
        (Node { position_type: PositionType::Absolute, left: px(x - d / 2.0), top: px(y - d / 2.0), width: px(d), height: px(d),
                border: UiRect::all(px(if ring { (s.receptor_border * k).max(1.0) } else { 0.0 })), border_radius: BorderRadius::MAX, ..default() },
         BackgroundColor(if ring { Color::NONE } else { WHITE }),
         BorderColor::all(if ring { alpha(WHITE, 0.7) } else { Color::NONE }))
    };
    parent.spawn(Node { margin: UiRect::top(px(24)), width: px(w), height: px(h), ..default() }).with_children(|f| {
        let x = |lane: usize| r * k + lane as f32 * pitch * k;
        for (lane, y) in [(0usize, 0.30), (2, 0.50), (3, 0.14), (1, 0.70)] { f.spawn(disc(x(lane), ring_y * y, false)); }
        for lane in 0..4 { f.spawn(disc(x(lane), ring_y, true)); }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(s: &mut Settings, f: Field, dir: i8, activate: bool) -> Effect {
        let mut st = SettingsUi { devices: vec!["Altavoces".into(), "Auriculares".into()], ..Default::default() };
        change(s, &mut st, f, dir, false, activate)
    }

    #[test]
    fn every_page_row_has_label_hint_and_value() {
        let s = Settings::default();
        let st = SettingsUi::default();
        for (name, fields) in PAGES {
            assert!(!fields.is_empty(), "{name}");
            for f in fields {
                assert!(!label(*f).is_empty() && !hint(*f).is_empty());
                let _ = value(&s, *f, &st);
            }
        }
    }

    #[test]
    fn sliders_clamp_without_float_noise() {
        let mut s = Settings::default();
        for _ in 0..100 { run(&mut s, ScrollSpeed, 1, false); }
        assert_eq!(s.scroll_speed, 40.0);
        for _ in 0..100 { run(&mut s, ScrollSpeed, -1, false); }
        assert_eq!(s.scroll_speed, 1.0);
        for _ in 0..3 { run(&mut s, NoteScale, 1, false); }
        assert_eq!(s.note_scale, 1.45);
        for _ in 0..100 { run(&mut s, NoteScale, 1, false); }
        assert_eq!(s.note_scale, 2.0);
        for _ in 0..100 { run(&mut s, LaneGap, -1, false); }
        assert_eq!(s.lane_gap, 0.0);
        for _ in 0..100 { run(&mut s, LaneGap, 1, false); }
        assert_eq!(s.lane_gap, 80.0);
        for _ in 0..100 { run(&mut s, ReceptorBorder, -1, false); }
        assert_eq!(s.receptor_border, 1.0);
        run(&mut s, ReceptorBorder, 1, false);
        assert_eq!(s.receptor_border, 1.5);
        for _ in 0..7 { run(&mut s, MusicVolume, 1, false); }
        assert_eq!(s.music_volume, 1.0);
        assert!(s.validate().is_empty(), "todo valor producido por la interfaz es válido");
    }

    #[test]
    fn activating_a_slider_steps_and_wraps() {
        let mut s = Settings::default();
        s.scroll_speed = 40.0;
        run(&mut s, ScrollSpeed, 0, true);
        assert_eq!(s.scroll_speed, 1.0);
        run(&mut s, ScrollSpeed, 0, true);
        assert_eq!(s.scroll_speed, 2.0);
    }

    #[test]
    fn toggles_and_choices() {
        let mut s = Settings::default();
        assert!(!s.no_fail);
        run(&mut s, NoFail, 0, true);
        assert!(s.no_fail);
        run(&mut s, FpsLimit, 1, false);
        assert_eq!(s.fps_limit, 30);
        run(&mut s, FpsLimit, -1, false);
        run(&mut s, FpsLimit, -1, false);
        assert_eq!(s.fps_limit, 360, "da la vuelta");
        assert!(s.validate().is_empty(), "{:?}", s.fps_limit);
    }

    #[test]
    fn minimal_visuals_toggle_overrides_hit_effects_display() {
        let mut s = Settings::default();
        let st = SettingsUi::default();
        assert_eq!(value(&s, ShowEffects, &st), "Activado");
        run(&mut s, MinimalVisuals, 0, true);
        assert!(s.minimal_visuals);
        assert_eq!(value(&s, MinimalVisuals, &st), "Activado");
        assert_eq!(value(&s, ShowEffects, &st), "Desactivado", "el modo minimalista se refleja en la fila de efectos");
        assert!(s.show_hit_effects, "sin tocar el ajuste individual: al volver a los efectos se recupera");
        run(&mut s, MinimalVisuals, 0, true);
        assert_eq!(value(&s, ShowEffects, &st), "Activado");
        assert_eq!(PAGES.iter().filter(|(_, f)| f.contains(&MinimalVisuals)).count(), 2, "aparece en Juego y en Gráficos");
    }

    #[test]
    fn precision_presets_roundtrip_and_stay_valid() {
        let mut s = Settings::default();
        assert_eq!(precision_index(&s.hit_windows), Some(1));
        run(&mut s, Precision, -1, false);
        assert_eq!(precision_index(&s.hit_windows), Some(0));
        assert!(s.hit_windows.is_valid() && s.hit_windows.bad_ms < HitWindows::default().bad_ms);
        run(&mut s, Precision, 1, false);
        run(&mut s, Precision, 1, false);
        run(&mut s, Precision, 1, false);
        assert_eq!(precision_index(&s.hit_windows), Some(2));
        assert!(s.hit_windows.is_valid() && s.hit_windows.bad_ms <= 500.0);
        s.hit_windows.marvelous_ms = 5.0;
        assert_eq!(value(&s, Precision, &SettingsUi::default()), "Personalizada");
    }

    #[test]
    fn offset_steps_by_one_or_five_and_clamps() {
        let mut s = Settings::default();
        let mut st = SettingsUi::default();
        change(&mut s, &mut st, Offset, 1, false, false);
        change(&mut s, &mut st, Offset, 1, true, false);
        assert_eq!(s.global_offset_ms, 6.0);
        for _ in 0..100 { change(&mut s, &mut st, Offset, -1, true, false); }
        assert_eq!(s.global_offset_ms, -300.0);
        assert_eq!(value(&s, Offset, &st), "-300 ms");
    }

    #[test]
    fn profiles_create_cycle_delete() {
        let mut s = Settings::default();
        run(&mut s, NewProfile, 0, true);
        assert_eq!((s.profiles.len(), s.active_profile), (2, 1));
        assert_eq!(s.profiles[1].name, "Perfil 2");
        run(&mut s, Profile, 1, false);
        assert_eq!(s.active_profile, 0, "da la vuelta");
        run(&mut s, DeleteProfile, 0, true);
        assert_eq!(s.profiles.len(), 1);
        run(&mut s, DeleteProfile, 0, true);
        assert_eq!(s.profiles.len(), 1, "siempre queda uno");
        for _ in 0..20 { run(&mut s, NewProfile, 0, true); }
        assert_eq!(s.profiles.len(), MAX_PROFILES);
        assert!(s.validate().is_empty());
    }

    #[test]
    fn rebinding_swaps_duplicates_and_rejects_reserved() {
        let mut s = Settings::default(); // D F J K
        bind_key(&mut s, 0, KeyCode::KeyA).unwrap();
        assert_eq!(s.keys(), [KeyCode::KeyA, KeyCode::KeyF, KeyCode::KeyJ, KeyCode::KeyK]);
        bind_key(&mut s, 0, KeyCode::KeyK).unwrap();
        assert_eq!(s.keys(), [KeyCode::KeyK, KeyCode::KeyF, KeyCode::KeyJ, KeyCode::KeyA], "K y A se intercambian");
        bind_key(&mut s, 1, KeyCode::KeyF).unwrap();
        assert_eq!(s.keys()[1], KeyCode::KeyF, "reasignar la misma tecla no rompe nada");
        assert!(bind_key(&mut s, 2, KeyCode::Escape).is_err());
        assert!(bind_key(&mut s, 2, KeyCode::F11).is_err());
        assert!(duplicate_key(&s.keys()).is_none());
        assert!(s.validate().is_empty());
        let mut st = SettingsUi::default();
        change(&mut s, &mut st, Key(2), 0, false, true);
        assert_eq!(st.rebinding, Some(2));
        assert_eq!(value(&s, Key(2), &st), "Pulsa una tecla…");
    }

    #[test]
    fn audio_device_cycles_through_list_and_default() {
        let mut s = Settings::default();
        let fx = run(&mut s, Device, 1, false);
        assert!(fx.device);
        assert_eq!(s.audio_device.as_deref(), Some("Altavoces"));
        let mut st = SettingsUi { devices: vec!["Altavoces".into(), "Auriculares".into()], ..Default::default() };
        change(&mut s, &mut st, Device, 1, false, false);
        assert_eq!(s.audio_device.as_deref(), Some("Auriculares"));
        change(&mut s, &mut st, Device, 1, false, false);
        assert_eq!(s.audio_device, None, "tras el último vuelve a «Predeterminado»");
        s.audio_device = Some("Desenchufado".into());
        change(&mut s, &mut st, Device, 1, false, false);
        assert_eq!(s.audio_device.as_deref(), Some("Altavoces"), "un dispositivo desaparecido se trata como el predeterminado");
    }

    #[test]
    fn reset_all_needs_confirmation_and_keeps_device() {
        let mut s = Settings::default();
        s.scroll_speed = 33.0;
        s.audio_device = Some("Altavoces".into());
        let mut st = SettingsUi::default();
        change(&mut s, &mut st, ResetAll, 0, false, true);
        assert_eq!(s.scroll_speed, 33.0);
        assert!(st.confirm_reset);
        change(&mut s, &mut st, NoFail, 0, false, true);
        assert!(!st.confirm_reset, "tocar otra fila cancela la confirmación");
        change(&mut s, &mut st, ResetAll, 0, false, true);
        change(&mut s, &mut st, ResetAll, 0, false, true);
        assert_eq!(s.scroll_speed, Settings::default().scroll_speed);
        assert!(!s.no_fail);
        assert_eq!(s.audio_device.as_deref(), Some("Altavoces"));
    }
}
