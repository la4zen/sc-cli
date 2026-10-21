//! Цвета и мелкие помощники.
use ratatui::style::Color;
use unicode_normalization::UnicodeNormalization;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub type Rgb = (u8, u8, u8);

pub const BG: Rgb = (14, 14, 18);
pub const BG2: Rgb = (17, 17, 22);
pub const SURFACE: Rgb = (21, 21, 27);
pub const FG: Rgb = (230, 230, 235);
pub const DIM: Rgb = (110, 110, 122);
pub const GREY: Rgb = (58, 58, 68);
pub const HINT: Rgb = (90, 90, 102);
pub const WHITE: Rgb = (255, 255, 255);
pub const SC_ORANGE: Rgb = (255, 85, 0);
pub const BLOCKS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// готовые акценты в настройках
pub const PALETTE: [&str; 10] = [
    "#ff5500", "#ff2e4d", "#ff4fa3", "#b46cff", "#5b7cff", "#22c3e8", "#1fdba0", "#8fe22e", "#ffc53d", "#e8e8f0",
];

pub fn c(x: Rgb) -> Color {
    Color::Rgb(x.0, x.1, x.2)
}

pub fn hexc(x: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", x.0, x.1, x.2)
}

pub fn parse_hex(s: &str) -> Option<Rgb> {
    let h = s.trim().trim_start_matches('#');
    if !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let h: String = match h.len() {
        3 => h.chars().flat_map(|c| [c, c]).collect(),
        6 => h.to_string(),
        _ => return None,
    };
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some((p(0)?, p(2)?, p(4)?))
}

pub fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    (f(a.0, b.0), f(a.1, b.1), f(a.2, b.2))
}

pub fn rgb_to_hsv(x: Rgb) -> (f32, f32, f32) {
    let (r, g, b) = (x.0 as f32 / 255.0, x.1 as f32 / 255.0, x.2 as f32 / 255.0);
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let d = mx - mn;
    let h = if d == 0.0 {
        0.0
    } else if mx == r {
        ((g - b) / d).rem_euclid(6.0) / 6.0
    } else if mx == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    (h, if mx == 0.0 { 0.0 } else { d / mx }, mx)
}

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Rgb {
    let h = h.rem_euclid(1.0) * 6.0;
    let i = h.floor();
    let f = h - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    let (r, g, b) = match i as i32 % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    ((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

pub fn shift(x: Rgb, dh: f32, ds: f32, dv: f32) -> Rgb {
    let (h, s, v) = rgb_to_hsv(x);
    hsv_to_rgb(h + dh, (s + ds).clamp(0.0, 1.0), (v + dv).clamp(0.0, 1.0))
}

pub fn ease(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

pub fn fmt_t(s: Option<f64>) -> String {
    let Some(s) = s else { return "–:––".into() };
    let s = s.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

pub fn human(n: Option<u64>) -> String {
    let Some(n) = n else { return "–".into() };
    for (div, suf) in [(1e9, "B"), (1e6, "M"), (1e3, "K")] {
        if n as f64 >= div {
            let s = format!("{:.1}", n as f64 / div);
            return s.trim_end_matches('0').trim_end_matches('.').to_string() + suf;
        }
    }
    n.to_string()
}

pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Обрезать строку по ширине в ячейках терминала.
pub fn clip(s: &str, w: usize) -> String {
    if width(s) <= w {
        return s.to_string();
    }
    if w == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut cur = 0;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0);
        if cur + cw > w - 1 {
            break;
        }
        out.push(ch);
        cur += cw;
    }
    out + "…"
}

pub fn nfc(s: &str) -> String {
    // заодно выкидываем управляющие символы — они ломают раскладку терминала
    s.nfc().filter(|c| !c.is_control()).collect()
}
