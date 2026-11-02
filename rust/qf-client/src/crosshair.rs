//! Crosshair HUD element: a centred cross, dot or cross + dot drawn from plain rectangles.
//!
//! Placement, scale, colour and visibility come from the element's [`ElementLayout`]
//! (`elements.crosshair`); the shape (style, arm length, thickness, gap) lives in
//! [`CrosshairShape`] (`[crosshair]` in `hud.toml`). Sizes are logical pixels, multiplied by the
//! layout scale and the window DPI scale.

use serde::{Deserialize, Serialize};

use crate::hud::{Canvas, Rect, Screen};
use crate::hud_layout::{self, ElementLayout, HudLayout};

pub const DEFAULT_COLOR: [f32; 4] = [0.3, 1.0, 0.3, 1.0];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrosshairStyle {
    Cross,
    Dot,
    CrossDot,
}

impl CrosshairStyle {
    pub fn next(self) -> Self {
        match self {
            Self::Cross => Self::Dot,
            Self::Dot => Self::CrossDot,
            Self::CrossDot => Self::Cross,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Cross => "CROSS",
            Self::Dot => "DOT",
            Self::CrossDot => "CROSS+DOT",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CrosshairShape {
    pub style: CrosshairStyle,
    /// Length of each arm in logical pixels.
    pub size: f32,
    /// Line (and dot) thickness in logical pixels.
    pub thickness: f32,
    /// Distance from the centre to the inner end of each arm.
    pub gap: f32,
}

impl CrosshairShape {
    pub const MAX_SIZE: f32 = 64.0;
    pub const MAX_THICKNESS: f32 = 16.0;
    pub const MAX_GAP: f32 = 64.0;

    /// Replaces non-finite values and clamps to the supported ranges.
    pub fn sanitized(mut self) -> Self {
        let default = Self::default();
        let fix =
            |v: f32, d: f32, min: f32, max: f32| if v.is_finite() { v } else { d }.clamp(min, max);
        self.size = fix(self.size, default.size, 1.0, Self::MAX_SIZE);
        self.thickness = fix(self.thickness, default.thickness, 1.0, Self::MAX_THICKNESS);
        self.gap = fix(self.gap, default.gap, 0.0, Self::MAX_GAP);
        self
    }
}

impl Default for CrosshairShape {
    fn default() -> Self {
        Self {
            style: CrosshairStyle::Cross,
            size: 8.0,
            thickness: 2.0,
            gap: 3.0,
        }
    }
}

/// Side in pixels of the square box the crosshair occupies (its widest style).
pub fn box_size(shape: &CrosshairShape, layout_scale: f32, screen_scale: f32) -> f32 {
    2.0 * (shape.gap + shape.size).max(shape.thickness) * layout_scale * screen_scale
}

pub struct Crosshair {
    layout: ElementLayout,
    shape: CrosshairShape,
}

impl Crosshair {
    pub fn new() -> Self {
        Self {
            layout: HudLayout::default_for(hud_layout::CROSSHAIR),
            shape: CrosshairShape::default(),
        }
    }

    pub fn set_layout(&mut self, layout: &HudLayout) {
        self.layout = layout.get(hud_layout::CROSSHAIR);
        self.shape = layout.crosshair;
    }

    pub fn draw(&self, screen: &Screen, canvas: &mut Canvas) {
        if !self.layout.enabled {
            return;
        }
        let scale = self.layout.scale * screen.scale;
        let side = box_size(&self.shape, self.layout.scale, screen.scale);
        let [left, top] = self.layout.anchor.place(screen, [side, side]);
        let center = [(left + side * 0.5).round(), (top + side * 0.5).round()];
        let color = self.layout.color.unwrap_or(DEFAULT_COLOR);
        let outline = [0.0, 0.0, 0.0, 0.6 * color[3]];
        let thickness = (self.shape.thickness * scale).round().max(1.0);
        let edge = (scale * 0.5).round().max(1.0);
        let length = (self.shape.size * scale).round().max(1.0);
        let gap = (self.shape.gap * scale).round();
        let half = (thickness * 0.5).floor();

        let mut parts: Vec<[f32; 4]> = Vec::new();
        let cross = matches!(
            self.shape.style,
            CrosshairStyle::Cross | CrosshairStyle::CrossDot
        );
        let dot = matches!(
            self.shape.style,
            CrosshairStyle::Dot | CrosshairStyle::CrossDot
        );
        if cross {
            let near = gap;
            parts.push([center[0] + near, center[1] - half, length, thickness]);
            parts.push([
                center[0] - near - length,
                center[1] - half,
                length,
                thickness,
            ]);
            parts.push([center[0] - half, center[1] + near, thickness, length]);
            parts.push([
                center[0] - half,
                center[1] - near - length,
                thickness,
                length,
            ]);
        }
        if dot {
            parts.push([center[0] - half, center[1] - half, thickness, thickness]);
        }
        for p in &parts {
            canvas.rects.push(Rect {
                x: p[0] - edge,
                y: p[1] - edge,
                w: p[2] + edge * 2.0,
                h: p[3] + edge * 2.0,
                color: outline,
            });
        }
        for p in &parts {
            canvas.rects.push(Rect {
                x: p[0],
                y: p[1],
                w: p[2],
                h: p[3],
                color,
            });
        }
    }
}

impl Default for Crosshair {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::{HAnchor, VAnchor};

    const SCREEN: Screen = Screen {
        width: 1000.0,
        height: 600.0,
        scale: 1.0,
    };

    fn drawn(layout: &HudLayout, screen: &Screen) -> Vec<Rect> {
        let mut crosshair = Crosshair::new();
        crosshair.set_layout(layout);
        let mut canvas = Canvas::default();
        crosshair.draw(screen, &mut canvas);
        canvas.rects
    }

    fn with_style(style: CrosshairStyle) -> HudLayout {
        let mut layout = HudLayout::default();
        layout.crosshair.style = style;
        layout
    }

    /// Bounding box of the coloured (non-outline) rects.
    fn bounds(rects: &[Rect]) -> [f32; 4] {
        let fill: Vec<_> = rects
            .iter()
            .filter(|r| r.color[0] != 0.0 || r.color[1] != 0.0)
            .collect();
        let x0 = fill.iter().map(|r| r.x).fold(f32::MAX, f32::min);
        let y0 = fill.iter().map(|r| r.y).fold(f32::MAX, f32::min);
        let x1 = fill.iter().map(|r| r.x + r.w).fold(f32::MIN, f32::max);
        let y1 = fill.iter().map(|r| r.y + r.h).fold(f32::MIN, f32::max);
        [x0, y0, x1, y1]
    }

    #[test]
    fn default_is_enabled_and_centered() {
        let layout = HudLayout::default();
        let c = layout.get(hud_layout::CROSSHAIR);
        assert!(c.enabled);
        assert_eq!(c.anchor.horizontal, HAnchor::Center);
        assert_eq!(c.anchor.vertical, VAnchor::Middle);
        assert_eq!(c.anchor.margin, [0.0, 0.0]);
        let [x0, y0, x1, y1] = bounds(&drawn(&layout, &SCREEN));
        assert!(((x0 + x1) * 0.5 - 500.0).abs() <= 1.0);
        assert!(((y0 + y1) * 0.5 - 300.0).abs() <= 1.0);
    }

    #[test]
    fn rect_counts_follow_style() {
        let count = |style| drawn(&with_style(style), &SCREEN).len();
        // Each part is drawn as an outline plus a fill.
        assert_eq!(count(CrosshairStyle::Cross), 8);
        assert_eq!(count(CrosshairStyle::Dot), 2);
        assert_eq!(count(CrosshairStyle::CrossDot), 10);
    }

    #[test]
    fn disabled_draws_nothing() {
        let mut layout = HudLayout::default();
        let mut c = layout.get(hud_layout::CROSSHAIR);
        c.enabled = false;
        layout.set(hud_layout::CROSSHAIR, c);
        assert!(drawn(&layout, &SCREEN).is_empty());
    }

    #[test]
    fn gap_and_size_set_the_extent() {
        let mut layout = HudLayout::default();
        layout.crosshair.gap = 4.0;
        layout.crosshair.size = 10.0;
        layout.crosshair.thickness = 2.0;
        let [x0, _, x1, _] = bounds(&drawn(&layout, &SCREEN));
        assert!((x1 - x0 - 28.0).abs() <= 1.0);
    }

    #[test]
    fn scales_with_dpi_and_layout_scale() {
        let base = bounds(&drawn(&HudLayout::default(), &SCREEN));
        let dpi = Screen {
            scale: 2.0,
            ..SCREEN
        };
        let big = bounds(&drawn(&HudLayout::default(), &dpi));
        assert!(((big[2] - big[0]) / (base[2] - base[0]) - 2.0).abs() < 0.2);

        let mut layout = HudLayout::default();
        let mut c = layout.get(hud_layout::CROSSHAIR);
        c.scale = 2.0;
        layout.set(hud_layout::CROSSHAIR, c);
        let scaled = bounds(&drawn(&layout, &SCREEN));
        assert!(((scaled[2] - scaled[0]) / (base[2] - base[0]) - 2.0).abs() < 0.2);
    }

    #[test]
    fn colour_override_is_used() {
        let mut layout = HudLayout::default();
        let mut c = layout.get(hud_layout::CROSSHAIR);
        c.color = Some([1.0, 0.0, 0.0, 1.0]);
        layout.set(hud_layout::CROSSHAIR, c);
        assert!(
            drawn(&layout, &SCREEN)
                .iter()
                .any(|r| r.color == [1.0, 0.0, 0.0, 1.0])
        );
    }

    #[test]
    fn crosshair_is_always_centered_whatever_the_layout_says() {
        let mut layout = HudLayout::default();
        let mut c = layout.get(hud_layout::CROSSHAIR);
        c.anchor.horizontal = HAnchor::Left;
        c.anchor.margin = [10.0, 0.0];
        layout.set(hud_layout::CROSSHAIR, c);
        let [x0, y0, x1, y1] = bounds(&drawn(&layout, &SCREEN));
        assert!(((x0 + x1) * 0.5 - 500.0).abs() <= 1.0);
        assert!(((y0 + y1) * 0.5 - 300.0).abs() <= 1.0);
    }

    #[test]
    fn saved_offcenter_anchor_is_ignored_on_load() {
        let text = "[elements.crosshair.anchor]\nhorizontal = \"Left\"\nvertical = \"Top\"\nmargin = [5.0, 5.0]\n";
        let layout = HudLayout::parse(text).unwrap();
        let anchor = layout.get(hud_layout::CROSSHAIR).anchor;
        assert_eq!(
            (anchor.horizontal, anchor.vertical),
            (HAnchor::Center, VAnchor::Middle)
        );
        assert_eq!(anchor.margin, [0.0, 0.0]);
    }

    #[test]
    fn shape_is_sanitized() {
        let shape = CrosshairShape {
            style: CrosshairStyle::Dot,
            size: f32::NAN,
            thickness: 1000.0,
            gap: -5.0,
        }
        .sanitized();
        assert_eq!(shape.size, CrosshairShape::default().size);
        assert_eq!(shape.thickness, CrosshairShape::MAX_THICKNESS);
        assert_eq!(shape.gap, 0.0);
    }

    #[test]
    fn style_cycles_through_all() {
        let mut style = CrosshairStyle::Cross;
        let mut seen = vec![style];
        for _ in 0..2 {
            style = style.next();
            seen.push(style);
        }
        assert_eq!(style.next(), CrosshairStyle::Cross);
        assert_eq!(seen.len(), 3);
        assert!(seen.windows(2).all(|w| w[0] != w[1]));
    }
}
