//! Avisos efímeros (confirmaciones y errores recuperables) sobre cualquier pantalla.
use crate::jobs::{Jobs, NoticeKind};
use crate::theme::*;
use crate::ui;
use bevy::prelude::*;

#[derive(Component)] struct ToastRoot;
#[derive(Component)] struct ToastText;

pub fn plugin(app: &mut App) {
    app.add_systems(Startup, spawn).add_systems(Update, update);
}

fn spawn(mut commands: Commands, fonts: Res<Fonts>) {
    commands.spawn((ToastRoot, GlobalZIndex(100),
        Node { position_type: PositionType::Absolute, bottom: px(36), left: percent(50), margin: UiRect::left(px(-380)), width: px(760), padding: UiRect::axes(px(22), px(14)),
               justify_content: JustifyContent::Center, display: Display::None, border: UiRect::all(px(1)), ..default() },
        BackgroundColor(SURFACE_2), BorderColor::all(BORDER)))
        .with_children(|p| { p.spawn((ToastText, ui::text(&fonts, "", 20.0, TEXT, Weight::Regular))); });
}

fn update(mut jobs: ResMut<Jobs>, mut root: Query<(&mut Node, &mut BorderColor), With<ToastRoot>>, mut text: Query<(&mut Text, &mut TextColor), With<ToastText>>) {
    jobs.notices.retain(|(_, _, at)| at.elapsed().as_secs_f32() < 6.0);
    let Some((msg, kind, _)) = jobs.notices.last().cloned() else {
        for (mut n, _) in &mut root { n.display = Display::None }
        return;
    };
    for (mut n, mut b) in &mut root { n.display = Display::Flex; *b = BorderColor::all(match kind { NoticeKind::Error => DANGER, NoticeKind::Ok => OK, NoticeKind::Info => BORDER }) }
    for (mut t, mut c) in &mut text { if **t != msg { **t = msg.clone() } c.0 = if kind == NoticeKind::Error { DANGER } else { TEXT } }
}
