//! Отрисовка: всё рисуем прямо в буфер ratatui — без виджетов-обёрток, так дешевле.
use crate::app::*;
use crate::i18n::{self, t};
use crate::soundcloud::Kind;
use crate::util::*;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::Frame;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::Instant;

type Seg = (String, Style);

const LINE: Rgb = (38, 38, 47);
const PANEL: Rgb = (27, 27, 34);

fn s(fg: Rgb, bg: Rgb) -> Style {
    Style::new().fg(c(fg)).bg(c(bg))
}

fn b(st: Style) -> Style {
    st.add_modifier(Modifier::BOLD)
}

fn it(st: Style) -> Style {
    st.add_modifier(Modifier::ITALIC)
}

fn seg(text: impl Into<String>, st: Style) -> Seg {
    (text.into(), st)
}

/// Строка в буфер с обрезкой по ширине и по краю экрана. Возвращает x после текста.
fn put(buf: &mut Buffer, x: u16, y: u16, text: &str, maxw: u16, st: Style) -> u16 {
    let a = buf.area;
    if y >= a.bottom() || x >= a.right() || maxw == 0 {
        return x;
    }
    let w = maxw.min(a.right() - x);
    buf.set_stringn(x, y, text, w as usize, st).0
}

fn put_segs(buf: &mut Buffer, mut x: u16, y: u16, segs: &[Seg], maxw: u16) -> u16 {
    let end = x.saturating_add(maxw);
    for (text, st) in segs {
        if x >= end {
            break;
        }
        x = put(buf, x, y, text, end - x, *st);
    }
    x
}

fn segs_width(segs: &[Seg]) -> usize {
    segs.iter().map(|(t, _)| width(t)).sum()
}

fn fill(buf: &mut Buffer, r: Rect, bg: Rgb) {
    let r = r.intersection(buf.area);
    for y in r.top()..r.bottom() {
        for x in r.left()..r.right() {
            if let Some(cl) = buf.cell_mut((x, y)) {
                cl.reset();
                cl.set_bg(c(bg));
            }
        }
    }
}

fn cell(buf: &mut Buffer, x: u16, y: u16, ch: &str, fg: Rgb, bg: Rgb) {
    if let Some(cl) = buf.cell_mut((x, y)) {
        cl.set_symbol(ch).set_fg(c(fg)).set_bg(c(bg));
    }
}

fn to_rgb(c: Color, dflt: Rgb) -> Rgb {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => dflt,
    }
}

fn since_start() -> f32 {
    static T: OnceLock<Instant> = OnceLock::new();
    T.get_or_init(Instant::now).elapsed().as_secs_f32()
}

pub fn draw(f: &mut Frame, a: &mut App) {
    let area = f.area();
    let buf = f.buffer_mut();
    fill(buf, area, BG);
    let (w, h) = (area.width, area.height);
    if w < 64 || h < 18 {
        let msg = t("small");
        let x = w.saturating_sub(width(msg) as u16) / 2;
        put(buf, x, h / 2, msg, w, s(DIM, BG));
        return;
    }
    let vis = a.st.vis;
    let wave_y = h - 5;
    let vis_y = wave_y - if vis { 8 } else { 0 };
    let top_h = if vis { vis_y - 1 } else { wave_y };
    let np_w = 48.min(w / 2);
    draw_left(buf, a, Rect::new(0, 0, w - np_w, top_h));
    draw_np(buf, a, Rect::new(w - np_w, 0, np_w, top_h));
    a.hits.vis_w = w - 2;
    if vis {
        draw_vis(buf, a, Rect::new(1, vis_y, w - 2, 8));
    }
    draw_wavebar(buf, a, Rect::new(0, wave_y, w, 4));
    draw_status(buf, a, Rect::new(0, h - 1, w, 1));
    draw_toasts(buf, a, area);
    if a.modal.is_some() {
        // фон под модалкой притушен
        for y in 0..h {
            for x in 0..w {
                if let Some(cl) = buf.cell_mut((x, y)) {
                    let (fg, bg) = (to_rgb(cl.fg, FG), to_rgb(cl.bg, BG));
                    cl.set_fg(c(mix(fg, BG, 0.7))).set_bg(c(mix(bg, BG, 0.7)));
                }
            }
        }
        match &a.modal {
            Some(Modal::Help) => draw_help(buf, a, area),
            Some(Modal::Settings(_)) => draw_settings(buf, a, area),
            None => {}
        }
    }
}

// ----- левая колонка: вкладки, поиск, крошки, список -----
fn draw_left(buf: &mut Buffer, a: &mut App, r: Rect) {
    let acc = a.accent;
    let x0 = r.x + 1;
    let w = r.width.saturating_sub(2);
    // вкладки
    let mut pos = vec![];
    let mut x = x0 + 1;
    a.hits.tabs.clear();
    for (i, k) in TABS.iter().enumerate() {
        let label = format!(" {} ", t(&format!("tab.{k}")));
        let on = i == a.view;
        let st = if on { b(s(FG, BG)) } else { s(DIM, BG) };
        let nx = put(buf, x, 0, &label, x0 + w - x.min(x0 + w), st);
        pos.push((x, nx));
        a.hits.tabs.push((x, nx, 0));
        x = nx + 1;
    }
    for xx in x0..x0 + w {
        cell(buf, xx, 1, "━", (30, 30, 38), BG);
    }
    // подчёркивание едет к выбранной вкладке
    let (from, to, start) = a.tab_anim;
    let k = ease((start.elapsed().as_secs_f32() / 0.25).min(1.0));
    if a.view < TABS.len() {
        let (fi, ti) = (pos[from as usize], pos[to as usize]);
        let ux0 = fi.0 as f32 + (ti.0 as f32 - fi.0 as f32) * k;
        let ux1 = fi.1 as f32 + (ti.1 as f32 - fi.1 as f32) * k;
        for xx in ux0.round() as u16..ux1.round() as u16 {
            cell(buf, xx, 1, "━", acc, BG);
        }
    }

    let mut y = 3;
    a.hits.search = Rect::default();
    if a.view == 1 {
        let focused = a.focus == Focus::Search && a.modal.is_none();
        draw_input(buf, Rect::new(x0, y, w, 3), &a.query, t("search.ph"), focused, acc);
        a.hits.search = Rect::new(x0, y, w, 3);
        y += 4;
    }
    // крошки
    let crumb = crumb(a);
    put_segs(buf, x0 + 1, y, &crumb, w.saturating_sub(1));
    y += 2;
    let tr = Rect::new(x0, y, w, r.bottom().saturating_sub(y));
    draw_table(buf, a, tr);
}

fn draw_input(buf: &mut Buffer, r: Rect, e: &LineEdit, ph: &str, focused: bool, acc: Rgb) {
    let bc = if focused { acc } else { LINE };
    for x in r.x..r.right() {
        cell(buf, x, r.y, "▔", bc, BG);
        cell(buf, x, r.y + 2, "▁", bc, BG);
    }
    fill(buf, Rect::new(r.x, r.y + 1, r.width, 1), SURFACE);
    cell(buf, r.x, r.y + 1, "▊", bc, SURFACE);
    cell(buf, r.right() - 1, r.y + 1, "▎", bc, SURFACE);
    let iw = r.width.saturating_sub(4) as usize;
    let y = r.y + 1;
    if e.s.is_empty() {
        if focused {
            cell(buf, r.x + 2, y, " ", BG, FG);
            put(buf, r.x + 3, y, ph, iw as u16 - 1, s(GREY, SURFACE));
        } else {
            put(buf, r.x + 2, y, ph, iw as u16, s(GREY, SURFACE));
        }
        return;
    }
    // окно по тексту, чтобы курсор всегда был виден
    let chars: Vec<char> = e.s.chars().collect();
    let mut start = 0;
    while start < e.cur && width(&chars[start..e.cur].iter().collect::<String>()) >= iw {
        start += 1;
    }
    let mut x = r.x + 2;
    for (i, ch) in chars.iter().enumerate().skip(start) {
        if x >= r.x + 2 + iw as u16 {
            break;
        }
        let st = if focused && i == e.cur { s(BG, FG) } else { s(FG, SURFACE) };
        x = put(buf, x, y, &ch.to_string(), r.x + 2 + iw as u16 - x, st);
    }
    if focused && e.cur == chars.len() && x < r.x + 2 + iw as u16 {
        cell(buf, x, y, " ", BG, FG);
    }
}

fn crumb(a: &App) -> Vec<Seg> {
    let (dim, grey) = (s(DIM, BG), s(GREY, BG));
    let mut out = vec![];
    let v = TABS.get(a.view).copied().unwrap_or("");
    let loading = a.loading.as_deref().is_some_and(|l| l == v || (l == "set" && v == "sets") || (l == "url" && v == "queue"));
    if loading {
        let dots = ".".repeat((since_start() * 3.0) as usize % 4);
        out.push(seg(format!("{}{dots}", t("loading")), it(s(a.accent, BG))));
    } else if v == "search" && a.rows.is_empty() {
        out.push(seg(t("empty_search"), grey));
    } else if v == "queue" && a.rows.is_empty() {
        out.push(seg(t("empty_queue"), grey));
    } else if !a.rows.is_empty() {
        let sep = "  ·  ";
        if let (Some((name, _)), "sets") = (&a.set_open, v) {
            out.push(seg("▤ ", s(a.accent, BG)));
            out.push(seg(name.clone(), b(s(FG, BG))));
            out.push(seg(sep, grey));
        }
        let sets = a.rows.iter().filter(|r| r.kind == Kind::Set).count();
        let mut parts = vec![];
        if a.rows.len() > sets {
            parts.push(i18n::count_tracks(a.rows.len() - sets));
        }
        if sets > 0 {
            parts.push(i18n::count_sets(sets));
        }
        let total: f64 = a.rows.iter().filter(|r| r.kind == Kind::Track).filter_map(|r| r.duration).sum();
        if total > 0.0 {
            parts.push(i18n::total_time(total));
        }
        out.push(seg(parts.join(sep), dim));
        if a.set_open.is_some() && v == "sets" {
            out.push(seg(format!("      {}", t("back")), grey));
        }
    }
    out
}

fn draw_table(buf: &mut Buffer, a: &mut App, r: Rect) {
    if r.height < 2 {
        return;
    }
    let acc = a.accent;
    let aw = 22u16;
    let cw = r.width.saturating_sub(1); // последний столбец — скроллбар
    let tw = cw.saturating_sub(48).max(12);
    let cols = [4u16, 4, tw, aw, 8];
    let mut xs = vec![];
    let mut x = r.x;
    for w in cols {
        xs.push(x + 1);
        x += w + 2;
    }
    let hdr = b(s(DIM, BG));
    for (i, name) in ["", "#", t("col.title"), t("col.artist"), t("col.time")].iter().enumerate() {
        put(buf, xs[i], r.y, name, cols[i].min((r.x + cw).saturating_sub(xs[i])), hdr);
    }
    let body = Rect::new(r.x, r.y + 1, cw, r.height - 1);
    a.hits.table = body;
    a.scroll = a.scroll.min(a.rows.len().saturating_sub(body.height as usize));
    let mark_row = a.marker_row();
    let mark = if a.prop_b("pause") { " ❚❚" } else { " ▶" };
    let focused = a.focus == Focus::Table && a.modal.is_none();
    for vi in 0..body.height {
        let i = a.scroll + vi as usize;
        let Some(row) = a.rows.get(i) else { break };
        let y = body.y + vi;
        let sel = i == a.cursor;
        let bg = if sel {
            mix(BG, acc, if focused { 0.40 } else { 0.28 })
        } else if i % 2 == 1 {
            BG2
        } else {
            BG
        };
        fill(buf, Rect::new(r.x, y, cw, 1), bg);
        let lim = |k: usize| cols[k].min((r.x + cw).saturating_sub(xs[k]));
        if mark_row == Some(i) {
            put(buf, xs[0], y, mark, lim(0), b(s(if sel { WHITE } else { acc }, bg)));
        }
        put(buf, xs[1], y, &(i + 1).to_string(), lim(1), s(if sel { mix(GREY, WHITE, 0.5) } else { GREY }, bg));
        let title = if row.kind == Kind::Set { format!("▤  {}", row.title) } else { row.title.clone() };
        let tst = if sel { b(s(WHITE, bg)) } else { s(FG, bg) };
        put(buf, xs[2], y, &clip(&title, tw as usize), lim(2), tst);
        let dst = s(if sel { mix(DIM, WHITE, 0.5) } else { DIM }, bg);
        put(buf, xs[3], y, &clip(&row.artist, aw as usize), lim(3), dst);
        let dur = match (row.kind, row.duration, row.count) {
            (Kind::Set, _, Some(n)) => format!("{n} ♫"),
            (_, Some(d), _) => fmt_t(Some(d)),
            _ => String::new(),
        };
        put(buf, xs[4], y, &dur, lim(4), dst);
    }
    // скроллбар
    let (n, vh) = (a.rows.len(), body.height as usize);
    if n > vh {
        let sx = r.x + cw;
        let size = (vh * vh / n).max(1);
        let top = a.scroll * (vh - size) / (n - vh).max(1);
        for vi in 0..vh {
            let on = vi >= top && vi < top + size;
            cell(buf, sx, body.y + vi as u16, " ", BG, if on { mix(BG, acc, 0.5) } else { SURFACE });
        }
    }
}

// ----- правая панель: обложка, название, статистика, далее -----
fn cover_lines(a: &mut App, cw: u16, rows: u16) -> Lines {
    if let Some(img) = &a.cover.img {
        let key = (a.cover.seq, cw, rows);
        if let Some(l) = a.cover.cache.get(&key) {
            return l.clone();
        }
        let px = image::imageops::resize(img, cw as u32, rows as u32 * 2, image::imageops::FilterType::Lanczos3);
        let p = |x: u32, y: u32| {
            let q = px.get_pixel(x, y);
            (q[0], q[1], q[2])
        };
        let lines: Lines = Rc::new(
            (0..rows as u32).map(|y| (0..cw as u32).map(|x| (p(x, 2 * y), p(x, 2 * y + 1))).collect()).collect(),
        );
        a.cover.cache.insert(key, lines.clone());
        return lines;
    }
    // заглушка: градиент от акцента с бликом
    let acc = a.accent;
    let ca = mix(acc, BG, 0.35);
    let cb = mix(shift(acc, 0.12, 0.0, -0.5), BG, 0.5);
    let ph = rows as f32 * 2.0;
    let px = |x: u16, y: f32| -> Rgb {
        let fx = x as f32 / (cw.max(2) - 1) as f32;
        let fy = y / (ph - 1.0).max(1.0);
        let k = fx * 0.6 + fy * 0.4;
        let glow = (-(((fx - 0.3).powi(2) + (fy - 0.25).powi(2)) / 0.08)).exp() * 60.0;
        let ch = |p: u8, q: u8| (p as f32 * (1.0 - k) + q as f32 * k + glow).clamp(0.0, 255.0) as u8;
        (ch(ca.0, cb.0), ch(ca.1, cb.1), ch(ca.2, cb.2))
    };
    Rc::new(
        (0..rows)
            .map(|y| (0..cw).map(|x| (px(x, 2.0 * y as f32), px(x, 2.0 * y as f32 + 1.0))).collect())
            .collect(),
    )
}

fn draw_np(buf: &mut Buffer, a: &mut App, r: Rect) {
    fill(buf, r, BG2);
    let acc = a.accent;
    let x0 = r.x + 2;
    let w = r.width.saturating_sub(4);
    let ch = (r.height as i32 - 1 - 10).clamp(6, 21) as u16;
    let rows = ch.min(w / 2).max(1);
    let cw = rows * 2;
    let new = cover_lines(a, cw, rows);
    let mut lines: Vec<&Vec<(Rgb, Rgb)>> = new.iter().collect();
    let p = a.cover.progress();
    let scan: Vec<(Rgb, Rgb)> = vec![(mix(acc, WHITE, 0.55), mix(acc, BG, 0.3)); cw as usize];
    let old = a.cover.old.clone();
    if p < 1.0 {
        if let Some(old) = old.as_ref().filter(|o| o.len() == rows as usize && o[0].len() == cw as usize) {
            let k = ((rows as f32 * ease(p)) as usize).min(rows as usize - 1);
            lines = new[..k].iter().chain(std::iter::once(&scan)).chain(old[k + 1..].iter()).collect();
        }
    }
    let pad = (w - cw) / 2;
    for (y, line) in lines.iter().enumerate() {
        for (x, (top, bot)) in line.iter().enumerate() {
            cell(buf, x0 + pad + x as u16, r.y + 1 + y as u16, "▀", *top, *bot);
        }
    }
    a.cover.last = Some(new);

    let mut y = r.y + 1 + ch + 1;
    // название: не влезает — бегущая строка
    a.hits.title_w = w;
    let tst = b(s(mix(acc, WHITE, 0.25), BG2));
    let text = &a.title.text;
    if width(text) <= w as usize {
        put(buf, x0, y, text, w, tst);
    } else {
        let lp = format!("{text}  •  {text}");
        let shown: String = lp.chars().skip(a.title.ofs).collect();
        put(buf, x0, y, &shown, w, tst);
    }
    y += 1;
    put(buf, x0, y, &a.artist, w, s(acc, BG2));
    y += 2;
    if let Some(m) = &a.meta {
        let mut segs = vec![];
        for (icon, k) in [("▶ ", "playback_count"), ("♥ ", "likes_count"), ("↻ ", "reposts_count")] {
            segs.push(seg(icon, s(acc, BG2)));
            segs.push(seg(format!("{}   ", human(m[k].as_u64())), s(DIM, BG2)));
        }
        if let Some(g) = m["genre"].as_str().filter(|g| !g.is_empty()) {
            segs.push(seg(format!("#{}", nfc(g).to_lowercase().replace(' ', "")), it(s(DIM, BG2))));
        }
        put_segs(buf, x0, y, &segs, w);
    }
    y += 2;
    // далее
    if a.cur_url.is_some() && a.queue.len() > 1 {
        let mut i = a.qpos + 1;
        if i >= a.queue.len() && a.prop_str("loop-playlist") == "inf" {
            i = 0;
        }
        if let Some(tr) = a.queue.get(i) {
            let lab = t("next");
            let x = put(buf, x0, y, &format!("{lab}  "), w, s(GREY, BG2));
            put(buf, x, y, &clip(&tr.title, 36), x0 + w - x, s(DIM, BG2));
            if !tr.artist.is_empty() {
                put(buf, x, y + 1, &clip(&tr.artist, 36), x0 + w - x, s(GREY, BG2));
            }
        }
    }
}

// ----- спектр -----
fn draw_vis(buf: &mut Buffer, a: &App, r: Rect) {
    let acc = a.accent;
    let (bw, gap) = a.geom();
    let n = a.vis.cur.len();
    if n == 0 {
        return;
    }
    let total = n * (bw + gap) - gap;
    let pad = (r.width as usize).saturating_sub(total) / 2;
    let hot = shift(acc, 0.07, -0.3, 0.3);
    let bx = |i: usize| r.x + (pad + i * (bw + gap)) as u16;
    let bar = |buf: &mut Buffer, i: usize, y: u16, ch: &str, col: Rgb| {
        for k in 0..bw {
            cell(buf, bx(i) + k as u16, y, ch, col, BG);
        }
    };
    let blk = |f: f32| BLOCKS[f.clamp(0.0, 8.0) as usize].to_string();
    let hgt = r.height as usize;
    if a.st.vis_style == "mirror" {
        // симметрично от центральной линии: вверх яркое, вниз — темнеющее эхо
        let up = (hgt / 2).max(1);
        let dn = hgt - up;
        for row in 0..up {
            let rr = up - 1 - row;
            let col = mix(acc, hot, rr as f32 / (up - 1).max(1) as f32);
            for i in 0..n {
                let h8 = a.vis.cur[i] * up as f32 * 8.0;
                bar(buf, i, r.y + row as u16, &blk(h8 - rr as f32 * 8.0), col);
            }
        }
        for rr in 0..dn {
            let col = mix(acc, BG, 0.35 + 0.5 * rr as f32 / dn.max(1) as f32);
            for i in 0..n {
                let f = (a.vis.cur[i] * dn as f32 * 8.0 - rr as f32 * 8.0).clamp(0.0, 8.0);
                let ch = if f >= 7.0 { "█" } else if f >= 3.0 { "▀" } else if f >= 1.0 { "▔" } else { " " };
                bar(buf, i, r.y + (up + rr) as u16, ch, col);
            }
        }
        return;
    }
    let rows = hgt.saturating_sub(2).max(1);
    let refl = 2.min(hgt - 1);
    let cap = mix(hot, WHITE, 0.6);
    for row in 0..rows {
        let rr = rows - 1 - row;
        let col = mix(acc, hot, rr as f32 / (rows - 1).max(1) as f32);
        for i in 0..n {
            let h8 = a.vis.cur[i] * rows as f32 * 8.0;
            let fill = (h8 - rr as f32 * 8.0).clamp(0.0, 8.0) as usize;
            let p8 = (a.vis.peak[i] * rows as f32 * 8.0) as usize;
            if fill == 0 && p8 / 8 == rr && a.vis.peak[i] > 0.04 && p8 as f32 > h8 + 2.0 {
                bar(buf, i, r.y + row as u16, "▁", cap);
            } else if fill > 0 {
                bar(buf, i, r.y + row as u16, &BLOCKS[fill].to_string(), col);
            }
        }
    }
    let base = mix(acc, BG, 0.68);
    for j in 0..refl {
        let col = if j == 0 { base } else { mix(base, BG, 0.55) };
        for i in 0..n {
            let hb = a.vis.cur[i] * rows as f32;
            let ch = match j {
                0 if hb > 0.5 => "▀",
                0 if hb > 0.15 => "▔",
                1 if hb > 1.5 => "▔",
                _ => continue,
            };
            bar(buf, i, r.y + (rows + j) as u16, ch, col);
        }
    }
}

// ----- waveform и время -----
fn draw_wavebar(buf: &mut Buffer, a: &mut App, r: Rect) {
    let acc = a.accent;
    let wr = Rect::new(r.x + 9, r.y, r.width.saturating_sub(18), 4);
    a.hits.wave = wr;
    let dur = a.prop_f("duration").filter(|d| *d > 0.0);
    let center = |buf: &mut Buffer, x: u16, text: &str, st: Style| {
        let tw = width(text) as u16;
        put(buf, x + 8u16.saturating_sub(tw) / 2, r.y + 1, text, 8, st);
    };
    if a.cur_url.is_some() {
        match (a.hover, dur) {
            (Some(f), Some(d)) => center(buf, r.x + 1, &fmt_t(Some(d * f as f64)), b(s(acc, BG))),
            _ if a.pos.is_some() => center(buf, r.x + 1, &fmt_t(a.pos), s(FG, BG)),
            _ => {}
        }
        if let (Some(d), Some(p)) = (dur, a.pos) {
            center(buf, r.right() - 9, &format!("-{}", fmt_t(Some(d - p))), s(DIM, BG));
        }
    }
    let (w, h) = (wr.width as usize, wr.height as usize);
    if w == 0 {
        return;
    }
    let up = (h - 1).max(1);
    let Some(samples) = a.wave.as_ref().filter(|s| !s.is_empty()) else {
        let y = wr.y + (up / 2) as u16;
        if a.wave_busy {
            // бегущий блик, пока грузится
            let cpos = (since_start() * 45.0) % (w as f32 + 30.0) - 15.0;
            for x in 0..w {
                let k = (1.0 - (x as f32 - cpos).abs() / 14.0).max(0.0);
                cell(buf, wr.x + x as u16, y, "━", mix(GREY, acc, k), BG);
            }
        } else {
            for x in 0..w {
                cell(buf, wr.x + x as u16, y, "─", GREY, BG);
            }
        }
        return;
    };
    // как np.array_split: куски почти равные, берём максимум
    let n = samples.len();
    let mut cols: Vec<f32> = (0..w)
        .map(|i| {
            let (s0, s1) = (i * n / w, ((i + 1) * n / w).max(i * n / w + 1).min(n));
            samples[s0.min(n - 1)..s1.max(s0.min(n - 1) + 1)].iter().cloned().fold(0.0, f32::max)
        })
        .collect();
    let mut sorted = cols.clone();
    sorted.sort_by(|p, q| p.total_cmp(q));
    let lo = sorted[((sorted.len() - 1) as f32 * 0.03) as usize];
    let hi = sorted.last().cloned().unwrap_or(1.0).max(1.0);
    for v in cols.iter_mut() {
        // SC-волны пережаты — растягиваем
        *v = ((*v - lo * 0.85) / (hi - lo * 0.85).max(1.0)).clamp(0.06, 1.0).powf(1.6);
    }
    let (played, cache) = match (dur, a.pos) {
        (Some(d), p) => {
            let pl = (w as f64 * p.unwrap_or(0.0) / d) as isize;
            (pl, pl + (w as f64 * a.prop_f("demuxer-cache-time").unwrap_or(0.0) / d) as isize)
        }
        _ => (-1, 0),
    };
    let acc2 = shift(acc, -0.05, 0.0, 0.1);
    let colors: Vec<Rgb> = (0..w as isize)
        .map(|x| {
            if x == played {
                WHITE
            } else if x < played {
                mix(acc, acc2, (x as f32 / w as f32 * 10.0).round() / 10.0)
            } else if a.hover_x.is_some_and(|hx| x <= hx as isize) {
                mix(acc, GREY, 0.55)
            } else if x < cache {
                (88, 88, 100)
            } else {
                GREY
            }
        })
        .collect();
    for row in 0..up {
        let rr = (up - 1 - row) as f32;
        for x in 0..w {
            let f = (cols[x] * up as f32 * 8.0 - rr * 8.0).clamp(0.0, 8.0) as usize;
            if f > 0 {
                cell(buf, wr.x + x as u16, wr.y + row as u16, &BLOCKS[f].to_string(), colors[x], BG);
            }
        }
    }
    for x in 0..w {
        let ch = if cols[x] > 0.3 { "▀" } else if cols[x] > 0.08 { "▔" } else { continue };
        cell(buf, wr.x + x as u16, wr.y + up as u16, ch, mix(colors[x], BG, 0.62), BG);
    }
}

// ----- статус -----
fn draw_status(buf: &mut Buffer, a: &App, r: Rect) {
    fill(buf, r, SURFACE);
    let acc = a.accent;
    let sf = |fg| s(fg, SURFACE);
    let mut segs = vec![];
    if !a.mpv.alive() || a.prop_b("idle-active") {
        segs.push(seg("■ ", sf(DIM)));
    } else if a.prop_b("paused-for-cache") {
        segs.push(seg("◌ ", sf(acc)));
    } else if a.prop_b("pause") {
        segs.push(seg("❚❚", b(sf(acc))));
    } else {
        segs.push(seg("▶ ", b(sf(acc))));
    }
    let mode = if a.prop_str("loop-file") == "inf" {
        t("mode.track")
    } else if a.prop_str("loop-playlist") == "inf" {
        t("mode.queue")
    } else {
        t("mode.off")
    };
    segs.push(seg(format!("  {mode}   "), sf(FG)));
    let vol = a.prop_f("volume").unwrap_or(a.st.volume).round() as i32;
    let filled = ((vol as f32 / 10.0).round() as usize).min(10);
    segs.push(seg(format!("{} ", t("vol")), sf(DIM)));
    segs.push(seg("━".repeat(filled), sf(acc)));
    segs.push(seg("━".repeat(10 - filled), sf(GREY)));
    segs.push(seg(format!(" {vol}"), sf(DIM)));
    let x = put_segs(buf, r.x + 1, r.y, &segs, r.width.saturating_sub(2));
    for key in ["hint", "hint.short"] {
        let hint = t(key);
        let hw = width(hint) as u16;
        if (r.right() - 1).saturating_sub(x) > hw + 2 {
            put(buf, r.right() - 1 - hw, r.y, hint, hw, sf(HINT));
            break;
        }
    }
}

// ----- уведомления -----
fn draw_toasts(buf: &mut Buffer, a: &App, area: Rect) {
    let mut bottom = area.bottom().saturating_sub(2);
    let maxw = (area.width / 2).max(30);
    for tst in a.toasts.iter().rev() {
        let col = match tst.level {
            Level::Info => a.accent,
            Level::Warn => (255, 197, 61),
            Level::Error => (255, 84, 84),
        };
        let tw = width(&tst.text).max(tst.title.as_deref().map_or(0, width)) as u16;
        let w = (tw + 4).min(maxw);
        let h = 3 + tst.title.is_some() as u16;
        if bottom < h + 1 {
            break;
        }
        let top = bottom + 1 - h;
        let x = area.right().saturating_sub(w + 1);
        fill(buf, Rect::new(x, top, w, h), PANEL);
        for y in top..top + h {
            cell(buf, x, y, "▌", col, PANEL);
        }
        let mut y = top + 1;
        if let Some(ti) = &tst.title {
            put(buf, x + 2, y, ti, w - 3, b(s(col, PANEL)));
            y += 1;
        }
        put(buf, x + 2, y, &tst.text, w - 3, s(FG, PANEL));
        bottom = top.saturating_sub(1);
    }
}

// ----- модалки -----
fn draw_box(buf: &mut Buffer, area: Rect, lines: &[Vec<Seg>], border: Rgb) -> Rect {
    let iw = lines.iter().map(|l| segs_width(l)).max().unwrap_or(0) as u16;
    let (w, h) = ((iw + 8).min(area.width), (lines.len() as u16 + 4).min(area.height));
    let r = Rect::new((area.width - w) / 2, (area.height - h) / 2, w, h);
    fill(buf, r, SURFACE);
    let bs = s(border, SURFACE);
    for x in r.x + 1..r.right() - 1 {
        put(buf, x, r.y, "─", 1, bs);
        put(buf, x, r.bottom() - 1, "─", 1, bs);
    }
    for y in r.y + 1..r.bottom() - 1 {
        put(buf, r.x, y, "│", 1, bs);
        put(buf, r.right() - 1, y, "│", 1, bs);
    }
    put(buf, r.x, r.y, "╭", 1, bs);
    put(buf, r.right() - 1, r.y, "╮", 1, bs);
    put(buf, r.x, r.bottom() - 1, "╰", 1, bs);
    put(buf, r.right() - 1, r.bottom() - 1, "╯", 1, bs);
    for (i, l) in lines.iter().enumerate() {
        let y = r.y + 2 + i as u16;
        if y >= r.bottom() - 1 {
            break;
        }
        put_segs(buf, r.x + 4, y, l, w.saturating_sub(8));
    }
    r
}

fn draw_help(buf: &mut Buffer, a: &App, area: Rect) {
    let acc = a.accent;
    let keys = [
        ("Enter", "k.enter"), ("a / e", "k.ae"), ("space", "k.space"), ("n / p", "k.np"),
        ("← →", "k.seek"), ("+ / -", "k.vol"), ("r", "k.repeat"), ("s", "k.shuffle"),
        ("d", "k.remove"), ("/", "k.search"), ("1…6", "k.tabs"), ("Backspace", "k.back"),
        ("Home / End", "k.home"), ("ctrl+r", "k.reload"), ("v", "k.vis"), ("c", "k.settings"),
        ("🖱", "k.mouse"), ("q", "k.q"), ("Q", "k.Q"),
    ];
    let mut lines = vec![vec![seg(t("help.title"), b(s(acc, SURFACE)))], vec![]];
    for (k, d) in keys {
        let pad = 11usize.saturating_sub(width(k));
        lines.push(vec![
            seg(format!("{}{k}  ", " ".repeat(pad)), b(s(mix(acc, WHITE, 0.3), SURFACE))),
            seg(t(d), s(FG, SURFACE)),
        ]);
    }
    draw_box(buf, area, &lines, (51, 51, 61));
}

fn choice(items: &[(bool, String)], sel: bool, hi: Rgb) -> Vec<Seg> {
    let mut out = vec![];
    for (on, lab) in items {
        if *on {
            out.push(seg(if sel { "‹ " } else { "  " }, s(hi, SURFACE)));
            out.push(seg(lab.clone(), b(s(if sel { hi } else { FG }, SURFACE))));
            out.push(seg(if sel { " ›  " } else { "    " }, s(hi, SURFACE)));
        } else {
            out.push(seg(format!("  {lab}    "), s(GREY, SURFACE)));
        }
    }
    out
}

fn draw_settings(buf: &mut Buffer, a: &App, area: Rect) {
    let Some(Modal::Settings(ss)) = &a.modal else { return };
    let acc = a.accent;
    let hi = mix(acc, WHITE, 0.35);
    let st = &a.st;
    let mut lines = vec![vec![seg(t("set.title"), b(s(acc, SURFACE)))], vec![]];
    for (i, key) in SET_ROWS.iter().enumerate() {
        let sel = i == ss.row;
        let label = t(&format!("set.{key}"));
        let pad = 14usize.saturating_sub(width(label));
        let mut l = vec![
            seg(if sel { " › " } else { "   " }, b(s(acc, SURFACE))),
            seg(format!("{label}{}", " ".repeat(pad)), if sel { b(s(FG, SURFACE)) } else { s(DIM, SURFACE) }),
        ];
        match *key {
            "lang" => l.extend(choice(
                &i18n::LANGS.iter().map(|(c, n)| (*c == i18n::lang(), n.to_string())).collect::<Vec<_>>(),
                sel,
                hi,
            )),
            "vis" => l.extend(choice(&[(st.vis, t("set.on").into()), (!st.vis, t("set.off").into())], sel, hi)),
            "style" => l.extend(choice(
                &STYLES.iter().map(|x| (*x == st.vis_style, t(&format!("style.{x}")).to_string())).collect::<Vec<_>>(),
                sel,
                hi,
            )),
            "fps" => l.extend(choice(&FPS.iter().map(|f| (*f == st.fps, f.to_string())).collect::<Vec<_>>(), sel, hi)),
            _ => {
                // цвет: [из обложки]  ██ ██ ██ …  ◆ свой #hex
                let cur = a.accent_index();
                let br = b(s(if sel { WHITE } else { hi }, SURFACE));
                for (j, item) in App::accents().iter().enumerate() {
                    let on = j == cur;
                    let fgc = if on { FG } else { DIM };
                    let lab = if on { b(s(fgc, SURFACE)) } else { s(fgc, SURFACE) };
                    match *item {
                        "auto" => {
                            l.push(seg(if on { "[" } else { " " }, br));
                            l.push((format!("◐ {}", t("set.auto")), lab));
                            l.push(seg(if on { "] " } else { "  " }, br));
                        }
                        "custom" => {
                            let cc = parse_hex(&st.custom).unwrap_or(acc);
                            l.push(seg(if on { " [" } else { "  " }, br));
                            l.push(seg("██", s(cc, SURFACE)));
                            l.push((format!(" {} {}", t("set.custom"), hexc(cc)), lab));
                            l.push(seg(if on { "]" } else { " " }, br));
                        }
                        hx => {
                            l.push(seg(if on { "[" } else { " " }, br));
                            l.push(seg("██", s(parse_hex(hx).unwrap_or(acc), SURFACE)));
                            l.push(seg(if on { "]" } else { " " }, br));
                        }
                    }
                }
            }
        }
        lines.push(l);
        if *key == "fps" && sel {
            lines.push(vec![seg(format!("{}{}", " ".repeat(17), t("set.fps_note")), it(s(GREY, SURFACE)))]);
        }
    }
    lines.push(vec![]);
    if ss.hex.is_some() {
        lines.push(vec![]);
        lines.push(vec![]);
        lines.push(vec![]);
    }
    lines.push(vec![seg(t("set.hint"), s(GREY, SURFACE))]);
    let r = draw_box(buf, area, &lines, mix(SURFACE, acc, 0.6));
    if let Some(hex) = &ss.hex {
        let y = r.bottom() - 2 - 4;
        draw_input(buf, Rect::new(r.x + 4, y, 28, 3), hex, t("set.hex_ph"), true, acc);
    }
}
