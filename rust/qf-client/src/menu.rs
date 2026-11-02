//! Immediate-mode menu toolkit drawn on the HUD [`Canvas`].
//!
//! Pure layout, hit-testing and input handling (no window or GPU types). A menu never touches
//! gameplay state: it returns [`UiAction`]s that the client applies at a frame boundary.
// Screens added by later PRs build on the full toolkit surface.
#![allow(dead_code)]

use crate::hud::{Canvas, Rect, Screen, text_size};

/// Where keyboard and mouse input is routed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputContext {
    Game,
    Menu,
}

/// Something a menu asks the client to do.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiAction {
    Resume,
    Quit,
    /// Join a team or spectate.
    SetTeam(TeamChoice),
    /// A gametype command (for example `primary` / `secondary`).
    Command {
        name: String,
        args: String,
    },
}

/// Where the local player plays. Spectator is a free-flying noclip view outside the match.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TeamChoice {
    Alpha,
    Beta,
    Spectator,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuKey {
    Up,
    Down,
    Activate,
    Back,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MenuItem {
    pub label: String,
    pub action: UiAction,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Bounds {
    pub fn contains(&self, point: [f32; 2]) -> bool {
        point[0] >= self.x
            && point[0] < self.x + self.w
            && point[1] >= self.y
            && point[1] < self.y + self.h
    }

    fn rect(&self, color: [f32; 4]) -> Rect {
        Rect {
            x: self.x,
            y: self.y,
            w: self.w,
            h: self.h,
            color,
        }
    }
}

const PIXEL: f32 = 3.0;
const PAD: f32 = 4.0;
const MIN_CHARS: usize = 16;

pub struct Menu {
    open: bool,
    title: String,
    items: Vec<MenuItem>,
    selected: usize,
}

impl Menu {
    pub fn new(title: &str, items: Vec<MenuItem>) -> Self {
        Self {
            open: false,
            title: title.to_owned(),
            items,
            selected: 0,
        }
    }

    /// The in-game pause menu.
    pub fn pause() -> Self {
        Self::new(
            "MENU",
            vec![
                MenuItem {
                    label: "RESUME".into(),
                    action: UiAction::Resume,
                },
                MenuItem {
                    label: "QUIT".into(),
                    action: UiAction::Quit,
                },
            ],
        )
    }

    /// Team join screen; the current choice is marked with `>`.
    pub fn team(current: TeamChoice) -> Self {
        let item = |label: &str, choice: TeamChoice| MenuItem {
            label: if choice == current {
                format!("> {label}")
            } else {
                label.to_owned()
            },
            action: UiAction::SetTeam(choice),
        };
        Self::new(
            "JOIN TEAM",
            vec![
                item("ALPHA", TeamChoice::Alpha),
                item("BETA", TeamChoice::Beta),
                item("SPECTATE", TeamChoice::Spectator),
            ],
        )
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn context(&self) -> InputContext {
        if self.open {
            InputContext::Menu
        } else {
            InputContext::Game
        }
    }

    pub fn open(&mut self) {
        self.open = true;
        self.selected = 0;
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn toggle(&mut self) {
        if self.open {
            self.close();
        } else {
            self.open();
        }
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn items(&self) -> &[MenuItem] {
        &self.items
    }

    /// Panel bounds and one bounds per item, in screen pixels.
    pub fn layout(&self, screen: &Screen) -> (Bounds, Vec<Bounds>) {
        let pixel = PIXEL * screen.scale;
        let pad = PAD * screen.scale;
        let chars = self
            .items
            .iter()
            .map(|i| i.label.chars().count())
            .chain([self.title.chars().count(), MIN_CHARS])
            .max()
            .unwrap_or(MIN_CHARS);
        let text = text_size(chars, pixel);
        let row = text[1] + pad * 2.0;
        let width = text[0] + pad * 4.0;
        let height = row * (self.items.len() as f32 + 1.0) + pad * 2.0;
        let panel = Bounds {
            x: (screen.width - width) * 0.5,
            y: (screen.height - height) * 0.5,
            w: width,
            h: height,
        };
        let items = (0..self.items.len())
            .map(|i| Bounds {
                x: panel.x + pad,
                y: panel.y + pad + row * (i as f32 + 1.0),
                w: width - pad * 2.0,
                h: row,
            })
            .collect();
        (panel, items)
    }

    /// Selects the item under the cursor, if any.
    pub fn hover(&mut self, screen: &Screen, cursor: [f32; 2]) {
        if !self.open {
            return;
        }
        let (_, items) = self.layout(screen);
        if let Some(i) = items.iter().position(|b| b.contains(cursor)) {
            self.selected = i;
        }
    }

    /// Activates the item under the cursor; clicks elsewhere do nothing.
    pub fn click(&mut self, screen: &Screen, cursor: [f32; 2]) -> Option<UiAction> {
        if !self.open {
            return None;
        }
        let (_, items) = self.layout(screen);
        let i = items.iter().position(|b| b.contains(cursor))?;
        self.selected = i;
        Some(self.items[i].action.clone())
    }

    pub fn key(&mut self, key: MenuKey) -> Option<UiAction> {
        if !self.open || self.items.is_empty() {
            return None;
        }
        let count = self.items.len();
        match key {
            MenuKey::Up => self.selected = (self.selected + count - 1) % count,
            MenuKey::Down => self.selected = (self.selected + 1) % count,
            MenuKey::Activate => return Some(self.items[self.selected].action.clone()),
            MenuKey::Back => return Some(UiAction::Resume),
        }
        None
    }

    pub fn draw(&self, screen: &Screen, cursor: Option<[f32; 2]>, canvas: &mut Canvas) {
        if !self.open {
            return;
        }
        let pixel = PIXEL * screen.scale;
        let pad = PAD * screen.scale;
        canvas.rects.push(
            Bounds {
                x: 0.0,
                y: 0.0,
                w: screen.width,
                h: screen.height,
            }
            .rect([0.0, 0.0, 0.0, 0.55]),
        );
        let (panel, items) = self.layout(screen);
        canvas.rects.push(panel.rect([0.08, 0.08, 0.1, 0.92]));
        let title = text_size(self.title.chars().count(), pixel);
        canvas.shadowed_text(
            &self.title,
            [panel.x + (panel.w - title[0]) * 0.5, panel.y + pad * 2.0],
            pixel,
            [1.0, 0.8, 0.2, 1.0],
        );
        for (i, (bounds, item)) in items.iter().zip(&self.items).enumerate() {
            let selected = i == self.selected;
            if selected {
                canvas.rects.push(bounds.rect([0.25, 0.3, 0.45, 0.9]));
            }
            let label = text_size(item.label.chars().count(), pixel);
            let color = if selected {
                [1.0, 1.0, 1.0, 1.0]
            } else {
                [0.75, 0.75, 0.75, 1.0]
            };
            canvas.shadowed_text(
                &item.label,
                [
                    bounds.x + (bounds.w - label[0]) * 0.5,
                    bounds.y + (bounds.h - label[1]) * 0.5,
                ],
                pixel,
                color,
            );
        }
        if let Some([x, y]) = cursor {
            let size = pixel * 3.0;
            canvas.rects.push(Rect {
                x,
                y,
                w: size,
                h: size,
                color: [1.0, 1.0, 1.0, 1.0],
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Screen = Screen {
        width: 1000.0,
        height: 600.0,
        scale: 1.0,
    };

    fn open_menu() -> Menu {
        let mut menu = Menu::pause();
        menu.open();
        menu
    }

    #[test]
    fn closed_menu_routes_to_game_and_ignores_input() {
        let mut menu = Menu::pause();
        assert_eq!(menu.context(), InputContext::Game);
        assert_eq!(menu.key(MenuKey::Activate), None);
        assert_eq!(menu.click(&SCREEN, [500.0, 300.0]), None);
        let mut canvas = Canvas::default();
        menu.draw(&SCREEN, None, &mut canvas);
        assert!(canvas.rects.is_empty());
    }

    #[test]
    fn team_menu_marks_current_and_emits_choices() {
        let mut menu = Menu::team(TeamChoice::Beta);
        menu.open();
        let labels: Vec<_> = menu.items().iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, ["ALPHA", "> BETA", "SPECTATE"]);
        assert_eq!(
            menu.key(MenuKey::Activate),
            Some(UiAction::SetTeam(TeamChoice::Alpha))
        );
        menu.key(MenuKey::Up);
        assert_eq!(
            menu.key(MenuKey::Activate),
            Some(UiAction::SetTeam(TeamChoice::Spectator))
        );
    }

    #[test]
    fn toggle_switches_context() {
        let mut menu = Menu::pause();
        menu.toggle();
        assert_eq!(menu.context(), InputContext::Menu);
        menu.toggle();
        assert_eq!(menu.context(), InputContext::Game);
    }

    #[test]
    fn keys_wrap_and_activate() {
        let mut menu = open_menu();
        assert_eq!(menu.key(MenuKey::Up), None);
        assert_eq!(menu.selected(), 1);
        assert_eq!(menu.key(MenuKey::Activate), Some(UiAction::Quit));
        menu.key(MenuKey::Down);
        assert_eq!(menu.selected(), 0);
        assert_eq!(menu.key(MenuKey::Activate), Some(UiAction::Resume));
        assert_eq!(menu.key(MenuKey::Back), Some(UiAction::Resume));
    }

    #[test]
    fn layout_is_centered_and_items_inside_panel() {
        let menu = open_menu();
        let (panel, items) = menu.layout(&SCREEN);
        assert!((panel.x + panel.w * 0.5 - 500.0).abs() < 0.01);
        assert!((panel.y + panel.h * 0.5 - 300.0).abs() < 0.01);
        assert_eq!(items.len(), 2);
        for b in &items {
            assert!(panel.contains([b.x, b.y]));
            assert!(panel.contains([b.x + b.w - 1.0, b.y + b.h - 1.0]));
        }
        assert!(items[0].y + items[0].h <= items[1].y);
    }

    #[test]
    fn hover_and_click_hit_test_items() {
        let mut menu = open_menu();
        let (_, items) = menu.layout(&SCREEN);
        let center = |b: &Bounds| [b.x + b.w * 0.5, b.y + b.h * 0.5];
        menu.hover(&SCREEN, center(&items[1]));
        assert_eq!(menu.selected(), 1);
        assert_eq!(
            menu.click(&SCREEN, center(&items[0])),
            Some(UiAction::Resume)
        );
        assert_eq!(menu.click(&SCREEN, [1.0, 1.0]), None);
    }

    #[test]
    fn layout_scales_with_dpi() {
        let menu = open_menu();
        let (a, _) = menu.layout(&SCREEN);
        let (b, _) = menu.layout(&Screen {
            scale: 2.0,
            ..SCREEN
        });
        assert!((b.w - a.w * 2.0).abs() < 0.01);
    }

    #[test]
    fn draw_stays_inside_screen() {
        let menu = open_menu();
        let mut canvas = Canvas::default();
        menu.draw(&SCREEN, Some([10.0, 10.0]), &mut canvas);
        assert!(canvas.rects.len() > 4);
        for r in &canvas.rects {
            assert!(r.x >= 0.0 && r.y >= 0.0);
            assert!(r.x + r.w <= SCREEN.width + 0.01 && r.y + r.h <= SCREEN.height + 0.01);
        }
    }
}
