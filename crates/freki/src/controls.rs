//! The Controls window's picture: a Game Boy (or Game Boy Color) with each button's
//! binding beside it, and the controller's own button labels and symbols.
//!
//! Controllers are recognised by their USB/Bluetooth IDs, or else by name, so a binding
//! to the right face button shows as Ⓑ on an Xbox pad, ○ on a PlayStation one and A on
//! a Switch Pro Controller.

use eframe::egui::{
    Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, pos2,
    vec2,
};
use freki_core::Button;
use gilrs::Button as Pad;

/// Controller families with their own button labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerKind {
    /// Xbox One, Series X|S, Elite and Adaptive controllers (Menu / View).
    Xbox,
    /// Xbox 360 (Start / Back).
    Xbox360,
    /// DualSense (Options / Create).
    PlayStation5,
    /// DualShock 4 (Options / Share).
    PlayStation4,
    /// Steam Controller and Steam Deck.
    Steam,
    /// Switch Pro Controller: A on the right, B at the bottom.
    Switch,
    /// Anything else: buttons named by position.
    Generic,
}

impl ControllerKind {
    /// Recognises a controller by vendor and product ID, or else by name.
    pub fn detect(name: &str, vendor: Option<u16>, product: Option<u16>) -> Self {
        match (vendor, product) {
            (Some(0x045E), Some(0x028E | 0x028F | 0x0291 | 0x0719)) => return Self::Xbox360,
            (Some(0x045E), _) => return Self::Xbox,
            (Some(0x054C), Some(0x0CE6 | 0x0DF2)) => return Self::PlayStation5,
            (Some(0x054C), Some(0x05C4 | 0x09CC | 0x0BA0)) => return Self::PlayStation4,
            (Some(0x28DE), _) => return Self::Steam,
            (Some(0x057E), Some(0x2009)) => return Self::Switch,
            _ => {}
        }
        let name = name.to_ascii_lowercase();
        let has = |words: &[&str]| words.iter().any(|w| name.contains(w));
        if has(&["xbox 360", "x360"]) {
            Self::Xbox360
        } else if has(&["xbox", "xinput"]) {
            Self::Xbox
        } else if has(&["dualsense", "ps5"]) {
            Self::PlayStation5
        } else if has(&["dualshock", "ps4", "playstation"]) {
            Self::PlayStation4
        } else if has(&["steam"]) {
            Self::Steam
        } else if has(&["pro controller", "switch"]) {
            Self::Switch
        } else {
            Self::Generic
        }
    }
}

/// A small picture of a controller button.
#[derive(Debug, Clone, PartialEq)]
pub enum Glyph {
    /// A lettered face button in its colour (Xbox A/B/X/Y; grey on Steam and Switch).
    Letter(&'static str, Color32),
    PsCross,
    PsCircle,
    PsSquare,
    PsTriangle,
    /// A D-pad with one arm lit.
    DPad(Button),
    /// The four face buttons with one lit: 0 bottom, 1 right, 2 left, 3 top.
    Position(u8),
    Menu,
    View,
    StartArrow,
    BackArrow,
    Options,
    Create,
    Share,
    Plus,
    Minus,
    /// A shoulder button, stick press or home button: its short name in a small badge.
    Badge(&'static str),
    /// No picture: the name says it all.
    Blank,
}

const XBOX_GREEN: Color32 = Color32::from_rgb(0x6C, 0xCF, 0x5F);
const XBOX_RED: Color32 = Color32::from_rgb(0xFF, 0x6B, 0x6B);
const XBOX_BLUE: Color32 = Color32::from_rgb(0x6A, 0xA8, 0xFF);
const XBOX_YELLOW: Color32 = Color32::from_rgb(0xF2, 0xC8, 0x4B);
const PLAIN: Color32 = Color32::from_rgb(0xD0, 0xD0, 0xDA);
const LIT: Color32 = Color32::from_rgb(0x8A, 0xB4, 0xF8);
const DIM: Color32 = Color32::from_rgb(0x55, 0x55, 0x5F);

/// How controller button `pad` is labelled on a `kind` controller: symbol and name.
pub fn pad_label(kind: ControllerKind, pad: Pad) -> (Glyph, String) {
    use ControllerKind::*;
    let xboxish = matches!(kind, Xbox | Xbox360 | Steam);
    let ps = matches!(kind, PlayStation5 | PlayStation4);
    let face = |index: u8| -> (Glyph, String) {
        // index: 0 bottom, 1 right, 2 left, 3 top
        if xboxish {
            let (letter, color) = [
                ("A", XBOX_GREEN),
                ("B", XBOX_RED),
                ("X", XBOX_BLUE),
                ("Y", XBOX_YELLOW),
            ][index as usize];
            let color = if kind == Steam { PLAIN } else { color };
            (Glyph::Letter(letter, color), format!("{letter} button"))
        } else if ps {
            let (glyph, name) = [
                (Glyph::PsCross, "Cross"),
                (Glyph::PsCircle, "Circle"),
                (Glyph::PsSquare, "Square"),
                (Glyph::PsTriangle, "Triangle"),
            ][index as usize]
                .clone();
            (glyph, name.to_string())
        } else if kind == Switch {
            let letter = ["B", "A", "Y", "X"][index as usize];
            (Glyph::Letter(letter, PLAIN), format!("{letter} button"))
        } else {
            let name = ["Bottom face", "Right face", "Left face", "Top face"][index as usize];
            (Glyph::Position(index), name.to_string())
        }
    };
    let pick =
        |xbox: &'static str, ps: &'static str, switch: &'static str, generic: &'static str| {
            match kind {
                Xbox | Xbox360 | Steam => xbox,
                PlayStation5 | PlayStation4 => ps,
                Switch => switch,
                Generic => generic,
            }
        };
    let badge = |name: &'static str| (Glyph::Badge(name), name.to_string());
    match pad {
        Pad::South => face(0),
        Pad::East => face(1),
        Pad::West => face(2),
        Pad::North => face(3),
        Pad::DPadUp => (Glyph::DPad(Button::Up), "D-pad up".into()),
        Pad::DPadDown => (Glyph::DPad(Button::Down), "D-pad down".into()),
        Pad::DPadLeft => (Glyph::DPad(Button::Left), "D-pad left".into()),
        Pad::DPadRight => (Glyph::DPad(Button::Right), "D-pad right".into()),
        Pad::Start => match kind {
            Xbox | Steam => (Glyph::Menu, "Menu".into()),
            Xbox360 => (Glyph::StartArrow, "Start".into()),
            PlayStation5 | PlayStation4 => (Glyph::Options, "Options".into()),
            Switch => (Glyph::Plus, "Plus".into()),
            Generic => (Glyph::Blank, "Start".into()),
        },
        Pad::Select => match kind {
            Xbox | Steam => (Glyph::View, "View".into()),
            Xbox360 => (Glyph::BackArrow, "Back".into()),
            PlayStation5 => (Glyph::Create, "Create".into()),
            PlayStation4 => (Glyph::Share, "Share".into()),
            Switch => (Glyph::Minus, "Minus".into()),
            Generic => (Glyph::Blank, "Select".into()),
        },
        Pad::Mode => badge(match kind {
            Xbox => "Xbox",
            Xbox360 => "Guide",
            PlayStation5 | PlayStation4 => "PS",
            Steam => "Steam",
            Switch => "Home",
            Generic => "Mode",
        }),
        Pad::LeftTrigger => badge(pick("LB", "L1", "L", "L1")),
        Pad::RightTrigger => badge(pick("RB", "R1", "R", "R1")),
        Pad::LeftTrigger2 => badge(pick("LT", "L2", "ZL", "L2")),
        Pad::RightTrigger2 => badge(pick("RT", "R2", "ZR", "R2")),
        Pad::LeftThumb => badge(pick("LS", "L3", "LS", "L3")),
        Pad::RightThumb => badge(pick("RS", "R3", "RS", "R3")),
        _ => badge("?"),
    }
}

/// Draws `glyph` centred in a square around `center`, `size` points across.
pub fn draw_glyph(painter: &Painter, center: Pos2, size: f32, glyph: &Glyph) {
    let r = size / 2.0;
    let ring = |color: Color32| {
        painter.circle(
            center,
            r - 0.5,
            Color32::from_gray(0x1A),
            Stroke::new(1.2, color),
        )
    };
    let thin = |color: Color32| Stroke::new(1.4, color);
    match glyph {
        Glyph::Letter(letter, color) => {
            ring(*color);
            painter.text(
                center,
                Align2::CENTER_CENTER,
                letter,
                FontId::proportional(size * 0.58),
                *color,
            );
        }
        Glyph::PsCross => {
            ring(DIM);
            let d = r * 0.42;
            let c = Color32::from_rgb(0x7A, 0xA7, 0xFF);
            painter.line_segment([center + vec2(-d, -d), center + vec2(d, d)], thin(c));
            painter.line_segment([center + vec2(d, -d), center + vec2(-d, d)], thin(c));
        }
        Glyph::PsCircle => {
            ring(DIM);
            painter.circle_stroke(center, r * 0.45, thin(Color32::from_rgb(0xFF, 0x7A, 0x8A)));
        }
        Glyph::PsSquare => {
            ring(DIM);
            let rect = Rect::from_center_size(center, vec2(r * 0.85, r * 0.85));
            painter.rect_stroke(
                rect,
                0,
                thin(Color32::from_rgb(0xE4, 0x8A, 0xD6)),
                StrokeKind::Middle,
            );
        }
        Glyph::PsTriangle => {
            ring(DIM);
            let d = r * 0.5;
            let points = vec![
                center + vec2(0.0, -d),
                center + vec2(d, d * 0.75),
                center + vec2(-d, d * 0.75),
            ];
            painter.add(Shape::closed_line(
                points,
                thin(Color32::from_rgb(0x5F, 0xD0, 0xB0)),
            ));
        }
        Glyph::DPad(lit) => {
            let arm = r * 0.36;
            let full = Rect::from_center_size(center, vec2(arm * 2.0, r * 2.0));
            painter.rect_filled(full, 1, DIM);
            painter.rect_filled(
                Rect::from_center_size(center, vec2(r * 2.0, arm * 2.0)),
                1,
                DIM,
            );
            let offset = match lit {
                Button::Up => vec2(0.0, -1.0),
                Button::Down => vec2(0.0, 1.0),
                Button::Left => vec2(-1.0, 0.0),
                _ => vec2(1.0, 0.0),
            } * (r - arm);
            painter.rect_filled(
                Rect::from_center_size(center + offset, vec2(arm * 2.0, arm * 2.0)),
                1,
                LIT,
            );
        }
        Glyph::Position(lit) => {
            let d = r * 0.62;
            for (i, offset) in [vec2(0.0, d), vec2(d, 0.0), vec2(-d, 0.0), vec2(0.0, -d)]
                .into_iter()
                .enumerate()
            {
                let color = if i as u8 == *lit { LIT } else { DIM };
                painter.circle_filled(center + offset, r * 0.3, color);
            }
        }
        Glyph::Menu => {
            ring(PLAIN);
            for dy in [-0.32, 0.0, 0.32] {
                let y = center.y + dy * r;
                painter.line_segment(
                    [pos2(center.x - r * 0.42, y), pos2(center.x + r * 0.42, y)],
                    Stroke::new(1.1, PLAIN),
                );
            }
        }
        Glyph::View => {
            ring(PLAIN);
            let s = vec2(r * 0.6, r * 0.45);
            let back = Rect::from_min_size(center + vec2(-r * 0.48, -r * 0.4), s);
            let front = back.translate(vec2(r * 0.35, r * 0.3));
            painter.rect_stroke(back, 0, Stroke::new(1.0, PLAIN), StrokeKind::Middle);
            painter.rect(
                front,
                0,
                Color32::from_gray(0x1A),
                Stroke::new(1.0, PLAIN),
                StrokeKind::Middle,
            );
        }
        Glyph::StartArrow | Glyph::BackArrow => {
            ring(PLAIN);
            let d = r * 0.4;
            let sign = if *glyph == Glyph::StartArrow {
                1.0
            } else {
                -1.0
            };
            let points = vec![
                center + vec2(sign * d, 0.0),
                center + vec2(-sign * d * 0.7, -d),
                center + vec2(-sign * d * 0.7, d),
            ];
            painter.add(Shape::convex_polygon(points, PLAIN, Stroke::NONE));
        }
        Glyph::Options | Glyph::Create | Glyph::Share => {
            let pill = Rect::from_center_size(center, vec2(size, size * 0.56));
            painter.rect(
                pill,
                CornerRadius::same((size * 0.28) as u8),
                Color32::from_gray(0x1A),
                Stroke::new(1.0, PLAIN),
                StrokeKind::Middle,
            );
            let s = Stroke::new(0.9, PLAIN);
            match glyph {
                Glyph::Options => {
                    for dy in [-0.18, 0.0, 0.18] {
                        let y = center.y + dy * r;
                        painter.line_segment(
                            [pos2(center.x - r * 0.35, y), pos2(center.x + r * 0.35, y)],
                            s,
                        );
                    }
                }
                Glyph::Create => {
                    let d = r * 0.25;
                    painter.add(Shape::line(
                        vec![
                            center + vec2(-d, d),
                            center + vec2(0.0, -d),
                            center + vec2(d, d),
                        ],
                        s,
                    ));
                }
                _ => {
                    let d = r * 0.25;
                    painter.add(Shape::line(
                        vec![
                            center + vec2(d, -d),
                            center + vec2(-d * 0.6, 0.0),
                            center + vec2(d, d * 0.2),
                        ],
                        s,
                    ));
                }
            }
        }
        Glyph::Plus | Glyph::Minus => {
            ring(PLAIN);
            let d = r * 0.42;
            let s = Stroke::new(1.5, PLAIN);
            painter.line_segment([center + vec2(-d, 0.0), center + vec2(d, 0.0)], s);
            if *glyph == Glyph::Plus {
                painter.line_segment([center + vec2(0.0, -d), center + vec2(0.0, d)], s);
            }
        }
        Glyph::Blank => {}
        Glyph::Badge(text) => {
            let font = FontId::proportional(size * 0.5);
            let width = painter
                .layout_no_wrap(text.to_string(), font.clone(), PLAIN)
                .size()
                .x
                + 8.0;
            let rect = Rect::from_center_size(center, vec2(width.max(size), size * 0.8));
            painter.rect(
                rect,
                CornerRadius::same(4),
                Color32::from_gray(0x1A),
                Stroke::new(1.0, PLAIN),
                StrokeKind::Middle,
            );
            painter.text(center, Align2::CENTER_CENTER, text, font, PLAIN);
        }
    }
}

/// Width a glyph takes when drawn at `size` (badges are wider than tall).
pub fn glyph_width(painter: &Painter, size: f32, glyph: &Glyph) -> f32 {
    match glyph {
        Glyph::Badge(text) => {
            let font = FontId::proportional(size * 0.5);
            (painter
                .layout_no_wrap(text.to_string(), font, PLAIN)
                .size()
                .x
                + 8.0)
                .max(size)
        }
        Glyph::Blank => 0.0,
        _ => size,
    }
}

/// Where things are in the picture, relative to its top-left corner (points).
pub mod layout {
    use super::*;

    pub const SIZE: [f32; 2] = [800.0, 500.0];
    /// The console's body.
    pub const BODY: Rect = Rect::from_min_max(pos2(290.0, 6.0), pos2(550.0, 426.0));
    pub const CHIP_SIZE: [f32; 2] = [150.0, 28.0];

    /// Centre of `button` on the drawing.
    pub fn button_center(button: Button) -> Pos2 {
        match button {
            Button::Up | Button::Down | Button::Left | Button::Right => pos2(370.0, 286.0),
            Button::B => pos2(468.0, 280.0),
            Button::A => pos2(516.0, 250.0),
            Button::Select => pos2(386.0, 357.0),
            Button::Start => pos2(438.0, 357.0),
        }
    }

    /// The point a binding's leader line ends at.
    pub fn line_target(button: Button) -> Pos2 {
        match button {
            Button::Up => pos2(370.0, 252.0),
            Button::Down => pos2(370.0, 320.0),
            Button::Left => pos2(336.0, 286.0),
            Button::Right => pos2(404.0, 286.0),
            Button::A => pos2(494.0, 250.0),
            Button::B => pos2(468.0, 302.0),
            Button::Select => pos2(386.0, 364.0),
            Button::Start => pos2(438.0, 364.0),
        }
    }

    /// The binding's chip and where its Game Boy button name goes.
    pub fn chip(button: Button) -> (Rect, Pos2, Align2) {
        let [w, h] = CHIP_SIZE;
        let left = |y: f32| {
            let rect = Rect::from_min_size(pos2(228.0 - w, y - h / 2.0), vec2(w, h));
            (rect, pos2(rect.left() - 10.0, y), Align2::RIGHT_CENTER)
        };
        let right = |y: f32| {
            let rect = Rect::from_min_size(pos2(612.0, y - h / 2.0), vec2(w, h));
            (rect, pos2(rect.right() + 10.0, y), Align2::LEFT_CENTER)
        };
        let below = |x: f32| {
            let rect = Rect::from_min_size(pos2(x - w / 2.0, 446.0), vec2(w, h));
            (rect, pos2(x, rect.bottom() + 12.0), Align2::CENTER_CENTER)
        };
        match button {
            Button::Up => left(208.0),
            Button::Left => left(250.0),
            Button::Right => left(292.0),
            Button::Down => left(334.0),
            Button::A => right(218.0),
            Button::B => right(302.0),
            Button::Select => below(320.0),
            Button::Start => below(520.0),
        }
    }

    /// The leader line from a chip to its button: a few points.
    pub fn leader(button: Button) -> Vec<Pos2> {
        let (chip, _, _) = chip(button);
        let target = line_target(button);
        match button {
            Button::A | Button::B => {
                let start = chip.left_center();
                vec![start, pos2(start.x - 40.0, start.y), target]
            }
            Button::Select | Button::Start => {
                let start = chip.center_top();
                vec![start, pos2(start.x, start.y - 18.0), target]
            }
            _ => {
                let start = chip.right_center();
                vec![start, pos2(300.0, start.y), target]
            }
        }
    }
}

/// Draws the console (an original Game Boy if `dmg`, else a Game Boy Color) with its
/// top-left corner at `origin`; `highlight` rings the button being rebound.
pub fn draw_console(painter: &Painter, origin: Pos2, k: f32, dmg: bool, highlight: Option<Button>) {
    let at = |x: f32, y: f32| origin + vec2(x, y) * k;
    let map = |p: Pos2| at(p.x, p.y);
    let rect = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(at(x, y), vec2(w, h) * k);
    let body = Rect::from_min_max(map(layout::BODY.min), map(layout::BODY.max));
    let r = |v: f32| (v * k).round() as u8;
    let corners = |nw: f32, ne: f32, sw: f32, se: f32| CornerRadius {
        nw: r(nw),
        ne: r(ne),
        sw: r(sw),
        se: r(se),
    };
    let font = |size: f32| FontId::proportional(size * k);
    let text = |x: f32, y: f32, s: &str, size: f32, color: Color32| {
        painter.text(at(x, y), Align2::LEFT_TOP, s, font(size), color);
    };
    let (shell, shade, bezel, button_fill, button_text, label) = if dmg {
        (
            Color32::from_rgb(0xC9, 0xC6, 0xC2),
            Color32::from_rgb(0xAE, 0xAA, 0xA5),
            Color32::from_rgb(0x5D, 0x5E, 0x6E),
            Color32::from_rgb(0x9A, 0x22, 0x57),
            Color32::from_rgb(0xF6, 0xD9, 0xE6),
            Color32::from_rgb(0x2B, 0x2F, 0x8F),
        )
    } else {
        (
            Color32::from_rgb(0x6B, 0x4F, 0xA0),
            Color32::from_rgb(0x56, 0x3F, 0x82),
            Color32::from_rgb(0x3B, 0x3B, 0x46),
            Color32::from_rgb(0x2A, 0x2A, 0x33),
            Color32::from_rgb(0xD0, 0xD0, 0xDC),
            Color32::from_rgb(0xE0, 0xD8, 0xF0),
        )
    };
    let body_corners = if dmg {
        corners(10.0, 10.0, 10.0, 72.0)
    } else {
        corners(22.0, 22.0, 46.0, 46.0)
    };
    painter.rect_filled(body.translate(vec2(0.0, 6.0 * k)), body_corners, shade);
    painter.rect_filled(body, body_corners, shell);

    let bezel_rect = rect(310.0, 26.0, 220.0, if dmg { 176.0 } else { 184.0 });
    let bezel_corners = if dmg {
        corners(8.0, 8.0, 8.0, 34.0)
    } else {
        corners(10.0, 10.0, 30.0, 30.0)
    };
    painter.rect_filled(bezel_rect, bezel_corners, bezel);
    let screen_x = if dmg { 350.0 } else { 345.0 };
    painter.rect_filled(
        rect(screen_x, if dmg { 46.0 } else { 44.0 }, 150.0, 135.0),
        r(2.0),
        Color32::from_rgb(0x9B, 0xBC, 0x0F),
    );
    if dmg {
        painter.line_segment(
            [at(322.0, 35.0), at(372.0, 35.0)],
            Stroke::new(2.0 * k, Color32::from_rgb(0x8C, 0x2F, 0x5A)),
        );
        painter.line_segment(
            [at(470.0, 35.0), at(518.0, 35.0)],
            Stroke::new(2.0 * k, Color32::from_rgb(0x2D, 0x3A, 0x8C)),
        );
        painter.text(
            at(421.0, 35.0),
            Align2::CENTER_CENTER,
            "DOT MATRIX WITH STEREO SOUND",
            font(6.0),
            Color32::from_gray(0xD8),
        );
        painter.circle_filled(
            at(325.0, 96.0),
            3.5 * k,
            Color32::from_rgb(0xE2, 0x34, 0x3C),
        );
        text(316.0, 104.0, "BATTERY", 6.0, Color32::from_gray(0xD8));
        text(312.0, 208.0, "Nintendo", 13.0, label);
        text(374.0, 206.0, "GAME BOY", 15.0, label);
        for i in 0..4 {
            let x = 478.0 + i as f32 * 10.0;
            painter.line_segment(
                [at(x, 420.0), at(x + 26.0, 376.0)],
                Stroke::new(4.0 * k, shade),
            );
        }
    } else {
        painter.text(
            at(420.0, 194.0),
            Align2::CENTER_CENTER,
            "GAME BOY COLOR",
            font(10.0),
            Color32::from_gray(0xC9),
        );
    }

    // D-pad, A, B, Select, Start.
    let dpad = map(layout::button_center(Button::Up));
    let dark = Color32::from_rgb(0x23, 0x23, 0x29);
    painter.rect_filled(
        Rect::from_center_size(dpad, vec2(26.0, 84.0) * k),
        r(4.0),
        dark,
    );
    painter.rect_filled(
        Rect::from_center_size(dpad, vec2(84.0, 26.0) * k),
        r(4.0),
        dark,
    );
    for button in [Button::B, Button::A] {
        let center = map(layout::button_center(button));
        painter.circle_filled(center, 22.0 * k, button_fill);
        painter.text(
            center,
            Align2::CENTER_CENTER,
            crate::input::button_name(button),
            font(13.0),
            button_text,
        );
    }
    let pill = if dmg {
        Color32::from_rgb(0x8D, 0x8A, 0x94)
    } else {
        bezel
    };
    for (button, name) in [(Button::Select, "SELECT"), (Button::Start, "START")] {
        let center = map(layout::button_center(button));
        let d = vec2(13.0, -6.0) * k;
        painter.line_segment([center - d, center + d], Stroke::new(11.0 * k, pill));
        painter.circle_filled(center - d, 5.5 * k, pill);
        painter.circle_filled(center + d, 5.5 * k, pill);
        painter.text(
            center + vec2(0.0, 16.0 * k),
            Align2::CENTER_CENTER,
            name,
            font(9.0),
            label,
        );
    }

    if let Some(button) = highlight {
        let amber = Stroke::new(2.0, AMBER);
        let center = map(layout::button_center(button));
        match button {
            Button::A | Button::B => {
                painter.circle_stroke(center, 23.0 * k, amber);
            }
            Button::Start | Button::Select => {
                painter.circle_stroke(center, 20.0 * k, amber);
            }
            _ => {
                let tip = map(layout::line_target(button));
                painter.circle_stroke(center + (tip - center) * 0.8, 15.0 * k, amber);
            }
        }
    }
}

/// Highlight for the binding being changed.
pub const AMBER: Color32 = Color32::from_rgb(0xF2, 0xC8, 0x79);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_controllers_by_ids_and_names() {
        use ControllerKind::*;
        assert_eq!(
            ControllerKind::detect("x", Some(0x045E), Some(0x028E)),
            Xbox360
        );
        assert_eq!(
            ControllerKind::detect("x", Some(0x045E), Some(0x0B13)),
            Xbox
        );
        assert_eq!(
            ControllerKind::detect("x", Some(0x054C), Some(0x0CE6)),
            PlayStation5
        );
        assert_eq!(
            ControllerKind::detect("x", Some(0x054C), Some(0x09CC)),
            PlayStation4
        );
        assert_eq!(
            ControllerKind::detect("x", Some(0x28DE), Some(0x1205)),
            Steam
        );
        assert_eq!(
            ControllerKind::detect("x", Some(0x057E), Some(0x2009)),
            Switch
        );
        assert_eq!(
            ControllerKind::detect("DualSense Wireless Controller", None, None),
            PlayStation5
        );
        assert_eq!(
            ControllerKind::detect("Xbox 360 Controller", None, None),
            Xbox360
        );
        assert_eq!(
            ControllerKind::detect("Logitech Dual Action", None, None),
            Generic
        );
    }

    #[test]
    fn labels_follow_the_controller() {
        use ControllerKind::*;
        assert_eq!(pad_label(Xbox, Pad::East).1, "B button");
        assert_eq!(pad_label(PlayStation5, Pad::East).1, "Circle");
        assert_eq!(pad_label(Switch, Pad::East).1, "A button");
        assert_eq!(pad_label(Generic, Pad::East).1, "Right face");
        assert_eq!(pad_label(PlayStation4, Pad::Select).1, "Share");
        assert_eq!(pad_label(Xbox360, Pad::Start).1, "Start");
        assert_eq!(pad_label(Xbox, Pad::LeftTrigger2).1, "LT");
    }
}
