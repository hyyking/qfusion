//! HUD layout editor: select elements, drag them (snapping to the nearest anchor), toggle,
//! scale and recolour. Pure state and drawing; the client feeds pointer/keys and applies the
//! resulting [`HudLayout`], so nothing here touches gameplay.
// Elements are listed by name; sizes are approximations of each element's on-screen box.
#![allow(dead_code)]

use crate::crosshair::{self, CrosshairShape};
use crate::hud::{Anchor, Canvas, HAnchor, Rect, Screen, VAnchor, text_size};
use crate::hud_layout::{self, ElementLayout, HudLayout};
use crate::menu::Bounds;
use crate::weapon_hud::WeaponHud;

pub const ELEMENTS: [&str; 6] = [
    hud_layout::UPS,
    hud_layout::FPS,
    hud_layout::RECORD,
    hud_layout::WEAPONS,
    hud_layout::AMMO,
    hud_layout::CROSSHAIR,
];

pub const SCALE_STEP: f32 = 0.25;
/// Step in logical pixels for crosshair size, thickness and gap.
pub const SHAPE_STEP: f32 = 1.0;

/// Colour overrides cycled with the colour keys; the first entry is "element default".
pub const COLORS: [Option<[f32; 4]>; 8] = [
    None,
    Some([1.0, 1.0, 1.0, 1.0]),
    Some([1.0, 0.25, 0.2, 1.0]),
    Some([0.3, 1.0, 0.3, 1.0]),
    Some([0.35, 0.55, 1.0, 1.0]),
    Some([1.0, 0.9, 0.2, 1.0]),
    Some([0.2, 0.9, 1.0, 1.0]),
    Some([1.0, 0.6, 0.15, 1.0]),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditorKey {
    Next,
    Prev,
    Toggle,
    ScaleUp,
    ScaleDown,
    NextColor,
    PrevColor,
    Reset,
    ResetAll,
    /// Crosshair shape keys; only apply while the crosshair is selected.
    Style,
    SizeUp,
    SizeDown,
    ThicknessUp,
    ThicknessDown,
    GapUp,
    GapDown,
    Save,
    Cancel,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EditorOutcome {
    None,
    /// The layout changed and should be previewed.
    Changed,
    /// Save this layout and leave the editor.
    Save(HudLayout),
    Cancel,
}

pub fn label(name: &str) -> &'static str {
    match name {
        hud_layout::UPS => "UPS",
        hud_layout::FPS => "FPS",
        hud_layout::RECORD => "REC",
        hud_layout::WEAPONS => "WEAPONS",
        hud_layout::AMMO => "AMMO",
        hud_layout::CROSSHAIR => "XHAIR",
        _ => "?",
    }
}

/// Approximate size in pixels of an element's box.
pub fn element_size(
    name: &str,
    layout_scale: f32,
    screen_scale: f32,
    crosshair: &CrosshairShape,
) -> [f32; 2] {
    let pixel = 3.0 * layout_scale * screen_scale;
    match name {
        hud_layout::UPS | hud_layout::FPS => {
            let number = text_size(4, pixel);
            let label = text_size(3, pixel * 0.5);
            [number[0].max(label[0]), number[1] + pixel + label[1]]
        }
        hud_layout::RECORD => {
            let text = text_size(8, pixel);
            [text[0] + text[1] + pixel * 2.0, text[1]]
        }
        hud_layout::WEAPONS => WeaponHud::bar_size(6, layout_scale, screen_scale),
        hud_layout::AMMO => WeaponHud::ammo_size(layout_scale, screen_scale),
        hud_layout::CROSSHAIR => {
            let side = crosshair::box_size(crosshair, layout_scale, screen_scale);
            [side, side]
        }
        _ => [pixel * 10.0, pixel * 7.0],
    }
}

/// Anchor and margin (logical pixels) that put a box of `size` with top-left `top_left` at that
/// spot, snapping to the nearest screen third. Inverse of [`Anchor::place`].
pub fn snap_anchor(screen: &Screen, size: [f32; 2], top_left: [f32; 2]) -> Anchor {
    let scale = screen.scale.max(0.01);
    let cx = top_left[0] + size[0] * 0.5;
    let cy = top_left[1] + size[1] * 0.5;
    let horizontal = if cx < screen.width / 3.0 {
        HAnchor::Left
    } else if cx > screen.width * 2.0 / 3.0 {
        HAnchor::Right
    } else {
        HAnchor::Center
    };
    let vertical = if cy < screen.height / 3.0 {
        VAnchor::Top
    } else if cy > screen.height * 2.0 / 3.0 {
        VAnchor::Bottom
    } else {
        VAnchor::Middle
    };
    let mx = match horizontal {
        HAnchor::Left => top_left[0].max(0.0),
        HAnchor::Center => top_left[0] - (screen.width - size[0]) * 0.5,
        HAnchor::Right => (screen.width - size[0] - top_left[0]).max(0.0),
    };
    let my = match vertical {
        VAnchor::Top => top_left[1].max(0.0),
        VAnchor::Middle => top_left[1] - (screen.height - size[1]) * 0.5,
        VAnchor::Bottom => (screen.height - size[1] - top_left[1]).max(0.0),
    };
    Anchor {
        horizontal,
        vertical,
        margin: [mx / scale, my / scale],
    }
}

pub struct HudEditor {
    original: HudLayout,
    working: HudLayout,
    selected: usize,
    /// Pointer offset from the dragged element's top-left corner.
    drag: Option<[f32; 2]>,
}

impl HudEditor {
    pub fn new(layout: &HudLayout) -> Self {
        Self {
            original: layout.clone(),
            working: layout.clone(),
            selected: 0,
            drag: None,
        }
    }

    pub fn layout(&self) -> &HudLayout {
        &self.working
    }

    pub fn original(&self) -> &HudLayout {
        &self.original
    }

    pub fn selected(&self) -> &'static str {
        ELEMENTS[self.selected]
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub fn bounds(&self, name: &str, screen: &Screen) -> Bounds {
        let layout = self.working.get(name);
        let size = element_size(name, layout.scale, screen.scale, &self.working.crosshair);
        let [x, y] = layout.anchor.place(screen, size);
        Bounds {
            x,
            y,
            w: size[0],
            h: size[1],
        }
    }

    /// Topmost (last drawn) element under `cursor`.
    pub fn element_at(&self, screen: &Screen, cursor: [f32; 2]) -> Option<usize> {
        (0..ELEMENTS.len())
            .rev()
            .find(|&i| self.bounds(ELEMENTS[i], screen).contains(cursor))
    }

    pub fn pointer_down(&mut self, screen: &Screen, cursor: [f32; 2]) {
        if let Some(i) = self.element_at(screen, cursor) {
            self.selected = i;
            let b = self.bounds(ELEMENTS[i], screen);
            self.drag = Some([cursor[0] - b.x, cursor[1] - b.y]);
        }
    }

    pub fn pointer_move(&mut self, screen: &Screen, cursor: [f32; 2]) -> EditorOutcome {
        let Some(offset) = self.drag else {
            return EditorOutcome::None;
        };
        let name = self.selected();
        let mut layout = self.working.get(name);
        let size = element_size(name, layout.scale, screen.scale, &self.working.crosshair);
        layout.anchor = snap_anchor(screen, size, [cursor[0] - offset[0], cursor[1] - offset[1]]);
        self.working.set(name, layout);
        EditorOutcome::Changed
    }

    pub fn pointer_up(&mut self) {
        self.drag = None;
    }

    fn edit(&mut self, f: impl FnOnce(&mut ElementLayout)) -> EditorOutcome {
        let name = self.selected();
        let mut layout = self.working.get(name);
        f(&mut layout);
        self.working.set(name, layout);
        EditorOutcome::Changed
    }

    fn edit_shape(&mut self, key: EditorKey) -> EditorOutcome {
        if self.selected() != hud_layout::CROSSHAIR {
            return EditorOutcome::None;
        }
        let shape = &mut self.working.crosshair;
        match key {
            EditorKey::Style => shape.style = shape.style.next(),
            EditorKey::SizeUp => shape.size += SHAPE_STEP,
            EditorKey::SizeDown => shape.size -= SHAPE_STEP,
            EditorKey::ThicknessUp => shape.thickness += SHAPE_STEP,
            EditorKey::ThicknessDown => shape.thickness -= SHAPE_STEP,
            EditorKey::GapUp => shape.gap += SHAPE_STEP,
            EditorKey::GapDown => shape.gap -= SHAPE_STEP,
            _ => return EditorOutcome::None,
        }
        *shape = shape.sanitized();
        EditorOutcome::Changed
    }

    pub fn key(&mut self, key: EditorKey) -> EditorOutcome {
        let count = ELEMENTS.len();
        match key {
            EditorKey::Next => {
                self.selected = (self.selected + 1) % count;
                EditorOutcome::None
            }
            EditorKey::Prev => {
                self.selected = (self.selected + count - 1) % count;
                EditorOutcome::None
            }
            EditorKey::Toggle => self.edit(|l| l.enabled = !l.enabled),
            EditorKey::ScaleUp => self.edit(|l| {
                l.scale = (l.scale + SCALE_STEP).min(ElementLayout::MAX_SCALE);
            }),
            EditorKey::ScaleDown => self.edit(|l| {
                l.scale = (l.scale - SCALE_STEP).max(ElementLayout::MIN_SCALE);
            }),
            EditorKey::NextColor => self.edit(|l| l.color = cycle_color(l.color, 1)),
            EditorKey::PrevColor => self.edit(|l| l.color = cycle_color(l.color, -1)),
            EditorKey::Reset => {
                let name = self.selected();
                self.working.set(name, HudLayout::default_for(name));
                if name == hud_layout::CROSSHAIR {
                    self.working.crosshair = CrosshairShape::default();
                }
                EditorOutcome::Changed
            }
            EditorKey::ResetAll => {
                self.working.reset();
                EditorOutcome::Changed
            }
            EditorKey::Style
            | EditorKey::SizeUp
            | EditorKey::SizeDown
            | EditorKey::ThicknessUp
            | EditorKey::ThicknessDown
            | EditorKey::GapUp
            | EditorKey::GapDown => self.edit_shape(key),
            EditorKey::Save => EditorOutcome::Save(self.working.clone()),
            EditorKey::Cancel => EditorOutcome::Cancel,
        }
    }

    pub fn draw(&self, screen: &Screen, canvas: &mut Canvas) {
        let pixel = 3.0 * screen.scale;
        canvas.rects.push(Rect {
            x: 0.0,
            y: 0.0,
            w: screen.width,
            h: screen.height,
            color: [0.0, 0.0, 0.0, 0.4],
        });
        for (i, name) in ELEMENTS.iter().enumerate() {
            let layout = self.working.get(name);
            let b = self.bounds(name, screen);
            let selected = i == self.selected;
            let edge = (screen.scale * 2.0).max(1.0);
            let color = match (selected, layout.enabled) {
                (true, _) => [1.0, 0.8, 0.2, 1.0],
                (false, true) => [0.7, 0.8, 1.0, 0.9],
                (false, false) => [0.5, 0.5, 0.5, 0.6],
            };
            canvas.rects.push(Rect {
                x: b.x,
                y: b.y,
                w: b.w,
                h: b.h,
                color: [color[0], color[1], color[2], 0.18],
            });
            for edge_rect in [
                (b.x, b.y, b.w, edge),
                (b.x, b.y + b.h - edge, b.w, edge),
                (b.x, b.y, edge, b.h),
                (b.x + b.w - edge, b.y, edge, b.h),
            ] {
                canvas.rects.push(Rect {
                    x: edge_rect.0,
                    y: edge_rect.1,
                    w: edge_rect.2,
                    h: edge_rect.3,
                    color,
                });
            }
            let mut text = label(name).to_owned();
            if *name == hud_layout::CROSSHAIR {
                text.push(' ');
                text.push_str(self.working.crosshair.style.name());
            }
            if !layout.enabled {
                text.push_str(" OFF");
            }
            let text_color = layout.color.unwrap_or(color);
            canvas.shadowed_text(
                &text,
                [b.x + edge * 2.0, b.y + edge * 2.0],
                pixel * 0.5,
                text_color,
            );
        }
        let help = [
            "HUD LAYOUT  DRAG TO MOVE  TAB SELECT",
            "E TOGGLE  +/- SCALE  C COLOUR  R RESET  SHIFT R ALL",
            "XHAIR: X STYLE  [ ] SIZE  , . THICKNESS  ; ' GAP",
            "ENTER SAVE  ESC CANCEL",
        ];
        for (i, line) in help.iter().enumerate() {
            canvas.shadowed_text(
                line,
                [pixel * 2.0, pixel * 2.0 + i as f32 * pixel * 3.0],
                pixel * 0.5,
                [1.0, 1.0, 1.0, 1.0],
            );
        }
    }
}

fn cycle_color(current: Option<[f32; 4]>, step: i32) -> Option<[f32; 4]> {
    let index = COLORS.iter().position(|c| *c == current).unwrap_or(0) as i32;
    COLORS[(index + step).rem_euclid(COLORS.len() as i32) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Screen = Screen {
        width: 1000.0,
        height: 600.0,
        scale: 1.0,
    };

    #[test]
    fn snap_is_inverse_of_place() {
        let size = [100.0, 40.0];
        for tl in [[10.0, 5.0], [450.0, 280.0], [880.0, 550.0], [20.0, 300.0]] {
            let anchor = snap_anchor(&SCREEN, size, tl);
            let placed = anchor.place(&SCREEN, size);
            assert!((placed[0] - tl[0]).abs() < 0.01 && (placed[1] - tl[1]).abs() < 0.01);
        }
    }

    #[test]
    fn snap_picks_nearest_thirds_and_scales_margin() {
        let a = snap_anchor(&SCREEN, [10.0, 10.0], [900.0, 20.0]);
        assert_eq!((a.horizontal, a.vertical), (HAnchor::Right, VAnchor::Top));
        let hi = Screen {
            scale: 2.0,
            ..SCREEN
        };
        let b = snap_anchor(&hi, [10.0, 10.0], [32.0, 32.0]);
        assert_eq!(b.margin, [16.0, 16.0]);
        let c = snap_anchor(&SCREEN, [10.0, 10.0], [-50.0, 700.0]);
        assert_eq!((c.horizontal, c.vertical), (HAnchor::Left, VAnchor::Bottom));
        assert_eq!(c.margin, [0.0, 0.0]);
    }

    #[test]
    fn dragging_moves_the_selected_element() {
        let mut editor = HudEditor::new(&HudLayout::default());
        let b = editor.bounds(hud_layout::FPS, &SCREEN);
        let grab = [b.x + 2.0, b.y + 2.0];
        editor.pointer_down(&SCREEN, grab);
        assert!(editor.is_dragging());
        assert_eq!(editor.selected(), hud_layout::FPS);
        assert_eq!(
            editor.pointer_move(&SCREEN, [grab[0] - 800.0, grab[1] + 400.0]),
            EditorOutcome::Changed
        );
        editor.pointer_up();
        assert!(!editor.is_dragging());
        let fps = editor.layout().get(hud_layout::FPS);
        assert_eq!(fps.anchor.horizontal, HAnchor::Left);
        assert_eq!(fps.anchor.vertical, VAnchor::Bottom);
        // Other elements are untouched and the original is kept for cancel.
        assert_eq!(
            editor.layout().get(hud_layout::UPS),
            HudLayout::default_for(hud_layout::UPS)
        );
        assert_eq!(editor.original(), &HudLayout::default());
    }

    #[test]
    fn click_on_empty_space_selects_nothing() {
        let mut editor = HudEditor::new(&HudLayout::default());
        assert_eq!(editor.element_at(&SCREEN, [300.0, 200.0]), None);
        editor.pointer_down(&SCREEN, [300.0, 200.0]);
        assert!(!editor.is_dragging());
    }

    #[test]
    fn keys_edit_the_selected_element() {
        let mut editor = HudEditor::new(&HudLayout::default());
        editor.key(EditorKey::Next);
        assert_eq!(editor.selected(), hud_layout::FPS);
        editor.key(EditorKey::Toggle);
        assert!(!editor.layout().get(hud_layout::FPS).enabled);
        for _ in 0..100 {
            editor.key(EditorKey::ScaleUp);
        }
        assert_eq!(
            editor.layout().get(hud_layout::FPS).scale,
            ElementLayout::MAX_SCALE
        );
        for _ in 0..100 {
            editor.key(EditorKey::ScaleDown);
        }
        assert_eq!(
            editor.layout().get(hud_layout::FPS).scale,
            ElementLayout::MIN_SCALE
        );
        editor.key(EditorKey::NextColor);
        assert_eq!(editor.layout().get(hud_layout::FPS).color, COLORS[1]);
        editor.key(EditorKey::PrevColor);
        editor.key(EditorKey::PrevColor);
        assert_eq!(
            editor.layout().get(hud_layout::FPS).color,
            COLORS[COLORS.len() - 1]
        );
        editor.key(EditorKey::Reset);
        assert_eq!(
            editor.layout().get(hud_layout::FPS),
            HudLayout::default_for(hud_layout::FPS)
        );
    }

    #[test]
    fn save_returns_layout_and_cancel_keeps_original() {
        let mut editor = HudEditor::new(&HudLayout::default());
        editor.key(EditorKey::Toggle);
        let EditorOutcome::Save(saved) = editor.key(EditorKey::Save) else {
            panic!("expected save");
        };
        assert!(!saved.get(hud_layout::UPS).enabled);
        assert_eq!(editor.key(EditorKey::Cancel), EditorOutcome::Cancel);
        assert!(editor.original().get(hud_layout::UPS).enabled);
        editor.key(EditorKey::ResetAll);
        assert!(editor.layout().get(hud_layout::UPS).enabled);
    }

    #[test]
    fn crosshair_shape_keys_only_apply_when_selected() {
        let mut editor = HudEditor::new(&HudLayout::default());
        assert_eq!(editor.key(EditorKey::Style), EditorOutcome::None);
        while editor.selected() != hud_layout::CROSSHAIR {
            editor.key(EditorKey::Next);
        }
        let before = editor.layout().crosshair;
        assert_eq!(editor.key(EditorKey::Style), EditorOutcome::Changed);
        assert_ne!(editor.layout().crosshair.style, before.style);
        editor.key(EditorKey::SizeUp);
        editor.key(EditorKey::GapDown);
        editor.key(EditorKey::ThicknessUp);
        let shape = editor.layout().crosshair;
        assert_eq!(shape.size, before.size + SHAPE_STEP);
        assert_eq!(shape.gap, before.gap - SHAPE_STEP);
        assert_eq!(shape.thickness, before.thickness + SHAPE_STEP);
        for _ in 0..200 {
            editor.key(EditorKey::GapDown);
        }
        assert_eq!(editor.layout().crosshair.gap, 0.0);
        editor.key(EditorKey::Reset);
        assert_eq!(editor.layout().crosshair, CrosshairShape::default());
    }

    #[test]
    fn draw_stays_inside_the_screen() {
        let editor = HudEditor::new(&HudLayout::default());
        let mut canvas = Canvas::default();
        editor.draw(&SCREEN, &mut canvas);
        for r in &canvas.rects {
            assert!(r.x >= -0.01 && r.y >= -0.01, "{r:?}");
            assert!(
                r.x + r.w <= SCREEN.width + 0.01 && r.y + r.h <= SCREEN.height + 0.01,
                "{r:?}"
            );
        }
    }
}
