//! Minimal HUD system: anchored elements drawing coloured rectangles into a [`Canvas`].
//!
//! Everything here is pure (no GPU or window types) so layout and values are unit
//! tested; `render.rs` only uploads the canvas rectangles in a 2D overlay pass.
//! Elements read a [`HudState`] snapshot and never affect gameplay.
// Anchors and accessors beyond the first element are the extension surface for later elements.
#![allow(dead_code)]

use std::fmt::Write;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Screen {
    pub width: f32,
    pub height: f32,
    /// Window scale factor (DPI); multiplied into element sizes.
    pub scale: f32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HAnchor {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VAnchor {
    Top,
    Middle,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    pub horizontal: HAnchor,
    pub vertical: VAnchor,
    /// Distance from the anchored edge in logical pixels (scaled by DPI); positive moves inward.
    pub margin: [f32; 2],
}

impl Anchor {
    /// Top-left pixel position of a box of `size` pixels placed by this anchor.
    pub fn place(&self, screen: &Screen, size: [f32; 2]) -> [f32; 2] {
        let margin = [self.margin[0] * screen.scale, self.margin[1] * screen.scale];
        let x = match self.horizontal {
            HAnchor::Left => margin[0],
            HAnchor::Center => (screen.width - size[0]) * 0.5 + margin[0],
            HAnchor::Right => screen.width - size[0] - margin[0],
        };
        let y = match self.vertical {
            VAnchor::Top => margin[1],
            VAnchor::Middle => (screen.height - size[1]) * 0.5 + margin[1],
            VAnchor::Bottom => screen.height - size[1] - margin[1],
        };
        [x, y]
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: [f32; 4],
}

/// Rectangles to draw this frame. Cleared and refilled each frame, so its
/// allocation is reused.
#[derive(Default)]
pub struct Canvas {
    pub rects: Vec<Rect>,
}

impl Canvas {
    pub fn clear(&mut self) {
        self.rects.clear();
    }

    /// Draws `text` with the built-in 5x7 font; `pixel` is the size of one font pixel.
    pub fn text(&mut self, text: &str, origin: [f32; 2], pixel: f32, color: [f32; 4]) {
        let mut x = origin[0];
        for ch in text.chars() {
            if let Some(rows) = glyph(ch.to_ascii_uppercase()) {
                for (row, bits) in rows.iter().enumerate() {
                    for col in 0..GLYPH_WIDTH {
                        if bits & (1 << (GLYPH_WIDTH - 1 - col)) != 0 {
                            self.rects.push(Rect {
                                x: x + col as f32 * pixel,
                                y: origin[1] + row as f32 * pixel,
                                w: pixel,
                                h: pixel,
                                color,
                            });
                        }
                    }
                }
            }
            x += (GLYPH_WIDTH + 1) as f32 * pixel;
        }
    }

    /// Text with a one-pixel dark drop shadow for legibility over the world.
    pub fn shadowed_text(&mut self, text: &str, origin: [f32; 2], pixel: f32, color: [f32; 4]) {
        let offset = (pixel * 0.5).max(1.0);
        self.text(
            text,
            [origin[0] + offset, origin[1] + offset],
            pixel,
            [0.0, 0.0, 0.0, 0.75],
        );
        self.text(text, origin, pixel, color);
    }
}

const GLYPH_WIDTH: usize = 5;
const GLYPH_HEIGHT: usize = 7;

/// Size in font pixels of `chars` characters (one pixel of spacing between them).
pub fn text_size(chars: usize, pixel: f32) -> [f32; 2] {
    let width = if chars == 0 {
        0
    } else {
        chars * (GLYPH_WIDTH + 1) - 1
    };
    [width as f32 * pixel, GLYPH_HEIGHT as f32 * pixel]
}

fn glyph(ch: char) -> Option<[u8; GLYPH_HEIGHT]> {
    Some(match ch {
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0A],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        ':' => [0x00, 0x04, 0x00, 0x00, 0x00, 0x04, 0x00],
        '/' => [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10],
        '-' => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],
        '!' => [0x04, 0x04, 0x04, 0x04, 0x04, 0x00, 0x04],
        '%' => [0x18, 0x19, 0x02, 0x04, 0x08, 0x13, 0x03],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C],
        _ => return None,
    })
}

/// Snapshot of render-side state for HUD elements.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HudState {
    pub velocity: [f32; 3],
    pub frame_seconds: f32,
}

/// Horizontal speed in units per second, as the legacy `viewState->xyspeed`
/// (`source/client/cgame/cg_view.cpp`) used by `cg_showSpeed`.
pub fn horizontal_speed(velocity: [f32; 3]) -> f32 {
    velocity[0].hypot(velocity[1])
}

pub trait HudElement {
    fn update(&mut self, state: &HudState);
    fn draw(&self, screen: &Screen, canvas: &mut Canvas);
}

/// Units-per-second counter. The shown number refreshes at a fixed rate so it is
/// readable, and its text buffer is reused.
pub struct UpsCounter {
    anchor: Anchor,
    shown: u32,
    since_refresh: f32,
    text: String,
}

impl UpsCounter {
    pub const REFRESH_SECONDS: f32 = 0.1;
    const PIXEL: f32 = 3.0;

    pub fn new() -> Self {
        Self {
            anchor: Anchor {
                horizontal: HAnchor::Center,
                vertical: VAnchor::Bottom,
                margin: [0.0, 48.0],
            },
            shown: 0,
            since_refresh: Self::REFRESH_SECONDS,
            text: String::with_capacity(8),
        }
    }

    pub fn shown(&self) -> u32 {
        self.shown
    }
}

impl Default for UpsCounter {
    fn default() -> Self {
        Self::new()
    }
}

impl HudElement for UpsCounter {
    fn update(&mut self, state: &HudState) {
        self.since_refresh += state.frame_seconds;
        if self.since_refresh < Self::REFRESH_SECONDS {
            return;
        }
        self.since_refresh = 0.0;
        self.shown = horizontal_speed(state.velocity).round() as u32;
        self.text.clear();
        let _ = write!(self.text, "{}", self.shown);
    }

    fn draw(&self, screen: &Screen, canvas: &mut Canvas) {
        let pixel = Self::PIXEL * screen.scale;
        let number = text_size(self.text.chars().count(), pixel);
        let label = text_size(3, pixel * 0.5);
        let gap = pixel;
        let size = [number[0].max(label[0]), number[1] + gap + label[1]];
        let [x, y] = self.anchor.place(screen, size);
        let white = [1.0, 1.0, 1.0, 1.0];
        canvas.shadowed_text(
            &self.text,
            [x + (size[0] - number[0]) * 0.5, y],
            pixel,
            white,
        );
        canvas.shadowed_text(
            "UPS",
            [x + (size[0] - label[0]) * 0.5, y + number[1] + gap],
            pixel * 0.5,
            [0.8, 0.8, 0.8, 1.0],
        );
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

    fn anchor(horizontal: HAnchor, vertical: VAnchor, margin: [f32; 2]) -> Anchor {
        Anchor {
            horizontal,
            vertical,
            margin,
        }
    }

    #[test]
    fn anchors_place_boxes_relative_to_screen() {
        let size = [100.0, 20.0];
        let p = |h, v| anchor(h, v, [10.0, 5.0]).place(&SCREEN, size);
        assert_eq!(p(HAnchor::Left, VAnchor::Top), [10.0, 5.0]);
        assert_eq!(p(HAnchor::Center, VAnchor::Bottom), [460.0, 575.0]);
        assert_eq!(p(HAnchor::Right, VAnchor::Middle), [890.0, 295.0]);
    }

    #[test]
    fn margins_scale_with_dpi() {
        let screen = Screen {
            scale: 2.0,
            ..SCREEN
        };
        let a = anchor(HAnchor::Left, VAnchor::Top, [10.0, 5.0]);
        assert_eq!(a.place(&screen, [1.0, 1.0]), [20.0, 10.0]);
    }

    #[test]
    fn speed_is_horizontal_only() {
        assert_eq!(horizontal_speed([3.0, 4.0, 1000.0]), 5.0);
        assert_eq!(horizontal_speed([0.0, 0.0, -500.0]), 0.0);
    }

    #[test]
    fn text_size_counts_spacing() {
        assert_eq!(text_size(0, 2.0), [0.0, 14.0]);
        assert_eq!(text_size(3, 2.0), [34.0, 14.0]);
    }

    #[test]
    fn counter_refreshes_at_fixed_rate_with_rounding() {
        let mut ups = UpsCounter::new();
        let state = |speed: f32, dt: f32| HudState {
            velocity: [speed, 0.0, 0.0],
            frame_seconds: dt,
        };
        ups.update(&state(320.4, 0.001));
        assert_eq!(ups.shown(), 320);
        ups.update(&state(500.0, 0.05));
        assert_eq!(ups.shown(), 320, "not refreshed before the interval");
        ups.update(&state(500.6, 0.06));
        assert_eq!(ups.shown(), 501);
    }

    #[test]
    fn counter_draws_centered_near_bottom() {
        let mut ups = UpsCounter::new();
        ups.update(&HudState {
            velocity: [320.0, 0.0, 0.0],
            frame_seconds: 1.0,
        });
        let mut canvas = Canvas::default();
        ups.draw(&SCREEN, &mut canvas);
        assert!(!canvas.rects.is_empty());
        let min_x = canvas.rects.iter().map(|r| r.x).fold(f32::MAX, f32::min);
        let max_x = canvas.rects.iter().map(|r| r.x + r.w).fold(0.0, f32::max);
        assert!(((min_x + max_x) * 0.5 - 500.0).abs() < 4.0, "centered");
        let max_y = canvas.rects.iter().map(|r| r.y + r.h).fold(0.0, f32::max);
        assert!(max_y < 600.0 && max_y > 500.0, "bottom area, got {max_y}");
    }
}
