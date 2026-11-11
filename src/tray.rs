//! The tray icon: a coloured disc whose colour is the day's state, and a small menu.

use tray_icon::menu::{Menu, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// What the tray shows at a glance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayState {
    Idle,
    Running,
    Partial,
    Blocked,
}

impl TrayState {
    fn colour(self) -> [u8; 3] {
        match self {
            TrayState::Idle => [46, 160, 67],
            TrayState::Running => [56, 132, 244],
            TrayState::Partial => [222, 160, 30],
            TrayState::Blocked => [215, 58, 73],
        }
    }

    pub fn tooltip(self) -> &'static str {
        match self {
            TrayState::Idle => "Activity Journal: idle",
            TrayState::Running => "Activity Journal: running",
            TrayState::Partial => "Activity Journal: last run partial",
            TrayState::Blocked => "Activity Journal: needs attention",
        }
    }
}

/// Menu item ids, compared against incoming menu events.
pub struct TrayMenu {
    pub open: MenuId,
    pub checkpoint: MenuId,
    pub final_run: MenuId,
    pub quit: MenuId,
}

pub fn build(state: TrayState) -> Result<(TrayIcon, TrayMenu), String> {
    let open = MenuItem::new("Open", true, None);
    let checkpoint = MenuItem::new("Run checkpoint now", true, None);
    let final_run = MenuItem::new("Run final journal now", true, None);
    let quit = MenuItem::new("Quit", true, None);
    let menu = Menu::with_items(&[
        &open,
        &PredefinedMenuItem::separator(),
        &checkpoint,
        &final_run,
        &PredefinedMenuItem::separator(),
        &quit,
    ])
    .map_err(|e| e.to_string())?;
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip(state.tooltip())
        .with_icon(icon(state))
        .build()
        .map_err(|e| e.to_string())?;
    let ids = TrayMenu {
        open: open.id().clone(),
        checkpoint: checkpoint.id().clone(),
        final_run: final_run.id().clone(),
        quit: quit.id().clone(),
    };
    Ok((tray, ids))
}

pub fn show(tray: &TrayIcon, state: TrayState) {
    let _ = tray.set_icon(Some(icon(state)));
    let _ = tray.set_tooltip(Some(state.tooltip()));
}

/// A 32×32 anti-aliased disc in the state's colour.
pub fn disc_rgba(state: TrayState, size: u32) -> Vec<u8> {
    let [r, g, b] = state.colour();
    let centre = size as f32 / 2.0;
    let radius = centre - 2.0;
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - centre;
            let dy = y as f32 + 0.5 - centre;
            let coverage = (radius - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
            pixels.extend_from_slice(&[r, g, b, (coverage * 255.0) as u8]);
        }
    }
    pixels
}

fn icon(state: TrayState) -> Icon {
    Icon::from_rgba(disc_rgba(state, 32), 32, 32).expect("a 32x32 RGBA buffer is a valid icon")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disc_is_opaque_in_the_middle_and_clear_at_the_corner() {
        let pixels = disc_rgba(TrayState::Blocked, 32);
        assert_eq!(pixels.len(), 32 * 32 * 4);
        let alpha = |x: usize, y: usize| pixels[(y * 32 + x) * 4 + 3];
        assert_eq!(alpha(16, 16), 255);
        assert_eq!(alpha(0, 0), 0);
    }

    #[test]
    fn every_state_has_its_own_colour() {
        let colours: std::collections::HashSet<_> = [
            TrayState::Idle,
            TrayState::Running,
            TrayState::Partial,
            TrayState::Blocked,
        ]
        .map(TrayState::colour)
        .into();
        assert_eq!(colours.len(), 4);
    }
}
