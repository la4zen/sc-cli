//! Состояние плеера, события и действия. Рисует ui.rs.
use crate::config::State;
use crate::i18n::{self, t};
use crate::mpv::{Mpv, MpvEvent};
use crate::soundcloud::{accent_from, write_m3u, Kind, Row, SoundCloud};
use crate::spectrum::{self, Shared};
use crate::util::*;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use image::RgbImage;
use ratatui::layout::Rect;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const TABS: [&str; 6] = ["queue", "search", "feed", "likes", "tracks", "sets"];
const LIKES: usize = 3;
pub const STYLES: [&str; 3] = ["bars", "thin", "mirror"];
pub const FPS: [u32; 4] = [15, 20, 30, 60];
pub const SET_ROWS: [&str; 5] = ["lang", "accent", "vis", "style", "fps"];

#[derive(Clone, Copy, PartialEq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

pub enum Ev {
    Term(Event),
    Mpv(MpvEvent),
    List { seq: u64, kind: String, arg: String, title: String, rows: Result<Vec<Row>, String> },
    Meta(String, Value),
    MetaFail(String),
    Media { url: String, img: Option<RgbImage>, acc: Option<Rgb>, wave: Option<Vec<f32>> },
    Enqueue(Vec<Row>, &'static str),
    Toast(String, Level),
}

pub struct Toast {
    pub text: String,
    pub title: Option<String>,
    pub level: Level,
    pub until: Instant,
}

/// Однострочное поле ввода.
#[derive(Default)]
pub struct LineEdit {
    pub s: String,
    pub cur: usize, // в символах
}

impl LineEdit {
    fn byte(&self, i: usize) -> usize {
        self.s.char_indices().nth(i).map(|(b, _)| b).unwrap_or(self.s.len())
    }
    pub fn set(&mut self, v: &str) {
        self.s = v.to_string();
        self.cur = v.chars().count();
    }
    /// true — текст поменялся
    pub fn key(&mut self, k: &KeyEvent, max: usize) -> bool {
        let n = self.s.chars().count();
        match k.code {
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                if n < max {
                    let b = self.byte(self.cur);
                    self.s.insert(b, c);
                    self.cur += 1;
                    return true;
                }
            }
            KeyCode::Char('u') => {
                self.s.clear();
                self.cur = 0;
                return true;
            }
            KeyCode::Char('w') => {
                // ctrl+w — слово назад
                let chars: Vec<char> = self.s.chars().collect();
                let mut i = self.cur;
                while i > 0 && chars[i - 1] == ' ' {
                    i -= 1;
                }
                while i > 0 && chars[i - 1] != ' ' {
                    i -= 1;
                }
                let (a, b) = (self.byte(i), self.byte(self.cur));
                self.s.replace_range(a..b, "");
                self.cur = i;
                return true;
            }
            KeyCode::Backspace if self.cur > 0 => {
                self.cur -= 1;
                let b = self.byte(self.cur);
                self.s.remove(b);
                return true;
            }
            KeyCode::Delete if self.cur < n => {
                let b = self.byte(self.cur);
                self.s.remove(b);
                return true;
            }
            KeyCode::Left => self.cur = self.cur.saturating_sub(1),
            KeyCode::Right => self.cur = (self.cur + 1).min(n),
            KeyCode::Home => self.cur = 0,
            KeyCode::End => self.cur = n,
            _ => {}
        }
        false
    }
}

pub struct SettingsState {
    pub row: usize,
    pub hex: Option<LineEdit>,
}

pub enum Modal {
    Help,
    Settings(SettingsState),
}

#[derive(PartialEq, Clone, Copy)]
pub enum Focus {
    Table,
    Search,
}

pub type Lines = Rc<Vec<Vec<(Rgb, Rgb)>>>;

/// Обложка полублоками ▀; при смене — проявляется сканлайном сверху вниз.
#[derive(Default)]
pub struct Cover {
    pub img: Option<RgbImage>,
    pub seq: u64,
    pub cache: HashMap<(u64, u16, u16), Lines>,
    pub last: Option<Lines>,
    pub old: Option<Lines>,
    pub reveal: Option<Instant>,
}

impl Cover {
    fn set(&mut self, img: Option<RgbImage>) {
        self.old = self.last.clone();
        self.img = img;
        self.seq += 1;
        self.cache.clear();
        self.reveal = Some(Instant::now());
    }
    pub fn progress(&self) -> f32 {
        self.reveal.map_or(1.0, |s| (s.elapsed().as_secs_f32() / 0.5).min(1.0))
    }
}

pub struct Marquee {
    pub text: String,
    pub ofs: usize,
    hold: u32,
    last: Instant,
}

impl Marquee {
    pub fn set(&mut self, s: &str) {
        if s != self.text {
            self.text = s.to_string();
            self.ofs = 0;
            self.hold = 10;
        }
    }
    fn tick(&mut self, w: usize) -> bool {
        if self.last.elapsed() < Duration::from_millis(220) || width(&self.text) <= w {
            return false;
        }
        self.last = Instant::now();
        if self.hold > 0 {
            self.hold -= 1;
            return false;
        }
        self.ofs = (self.ofs + 1) % (self.text.chars().count() + 5);
        if self.ofs == 0 {
            self.hold = 10;
        }
        true
    }
}

#[derive(Default)]
pub struct Vis {
    pub cur: Vec<f32>,
    pub peak: Vec<f32>,
    hold: Vec<f32>,
    pub running: bool,
}

#[derive(Default)]
pub struct Hits {
    pub tabs: Vec<(u16, u16, u16)>, // x0, x1, y
    pub table: Rect,
    pub wave: Rect,
    pub search: Rect,
    pub vis_w: u16,
    pub title_w: u16,
}

pub struct App {
    pub st: State,
    pub sc: Arc<SoundCloud>,
    pub mpv: Mpv,
    pub spec: Arc<Shared>,
    tx: Sender<Ev>,
    pub props: HashMap<String, Value>,
    pub pos: Option<f64>,
    force_poll: bool,
    pub queue: Vec<Row>,
    pub qpos: usize,
    lists: HashMap<String, Vec<Row>>,
    pub set_open: Option<(String, Vec<Row>)>,
    pub view: usize,
    pub rows: Vec<Row>,
    row_index: HashMap<String, usize>,
    pub cursor: usize,
    pub scroll: usize,
    last_page: usize,
    pub cur_url: Option<String>,
    pub accent: Rgb,
    acc_from: Rgb,
    acc_to: Rgb,
    acc_start: Option<Instant>,
    pub hover: Option<f32>,
    pub hover_x: Option<u16>,
    cover_acc: Option<Rgb>,
    pub loading: Option<String>,
    list_gen: u64,
    pub title: Marquee,
    pub artist: String,
    pub meta: Option<Value>,
    pub cover: Cover,
    pub wave: Option<Vec<f32>>,
    pub wave_busy: bool,
    pub vis: Vis,
    pub modal: Option<Modal>,
    pub focus: Focus,
    pub query: LineEdit,
    pub toasts: Vec<Toast>,
    pub quit: bool,
    pub dirty: bool,
    pub hits: Hits,
    /// подчёркивание вкладки едет к новой
    pub tab_anim: (f32, f32, Instant),
    next_poll: Instant,
    next_frame: Instant,
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s != "no",
        _ => true,
    }
}

impl App {
    pub fn new(tx: Sender<Ev>) -> App {
        let st = State::load();
        i18n::set_lang(st.lang.as_deref());
        let spec = spectrum::start(48, st.fps);
        let now = Instant::now();
        let mut a = App {
            sc: Arc::new(SoundCloud::new()),
            mpv: Mpv::dead(),
            spec,
            tx,
            props: HashMap::new(),
            pos: None,
            force_poll: true,
            queue: vec![],
            qpos: 0,
            lists: HashMap::new(),
            set_open: None,
            view: usize::MAX,
            rows: vec![],
            row_index: HashMap::new(),
            cursor: 0,
            scroll: 0,
            last_page: 0,
            cur_url: None,
            accent: SC_ORANGE,
            acc_from: SC_ORANGE,
            acc_to: SC_ORANGE,
            acc_start: None,
            hover: None,
            hover_x: None,
            cover_acc: None,
            loading: None,
            list_gen: 0,
            title: Marquee { text: String::new(), ofs: 0, hold: 10, last: now },
            artist: String::new(),
            meta: None,
            cover: Cover::default(),
            wave: None,
            wave_busy: false,
            vis: Vis::default(),
            modal: None,
            focus: Focus::Table,
            query: LineEdit::default(),
            toasts: vec![],
            quit: false,
            dirty: true,
            hits: Hits::default(),
            tab_anim: (0.0, 0.0, now),
            next_poll: now,
            next_frame: now,
            st,
        };
        a.accent = a.base_accent();
        a.acc_to = a.accent;
        a
    }

    pub fn start(&mut self, initial: (Option<&str>, Option<String>)) {
        self.connect(false);
        self.idle_panel();
        match initial {
            (Some("url"), Some(u)) => {
                self.load_list("url", &u, "");
                self.show_view(0);
            }
            (Some("query"), Some(q)) => {
                self.query.set(&q);
                self.show_view(1);
                self.load_list("search", &q, "");
            }
            (Some("tab"), Some(tab)) => self.show_view(TABS.iter().position(|x| *x == tab).unwrap_or(LIKES)),
            _ => {
                let has = self.props.get("playlist").and_then(|v| v.as_array()).is_some_and(|a| !a.is_empty());
                self.show_view(if self.mpv.alive() && has { 0 } else { LIKES });
            }
        }
    }

    pub fn shutdown(&mut self) {
        self.spec.stop();
        self.save();
    }

    fn save(&mut self) {
        if let Err(e) = self.st.save() {
            self.notify(e.to_string(), Level::Error);
        }
    }

    pub fn playing(&self) -> bool {
        self.mpv.alive() && !truthy(self.props.get("pause")) && !truthy(self.props.get("idle-active"))
    }

    pub fn prop_str(&self, k: &str) -> &str {
        self.props.get(k).and_then(|v| v.as_str()).unwrap_or_default()
    }
    pub fn prop_f(&self, k: &str) -> Option<f64> {
        self.props.get(k).and_then(|v| v.as_f64())
    }
    pub fn prop_b(&self, k: &str) -> bool {
        truthy(self.props.get(k))
    }

    pub fn notify(&mut self, text: impl Into<String>, level: Level) {
        self.toast(text.into(), None, level, 2.5);
    }

    fn toast(&mut self, text: String, title: Option<String>, level: Level, secs: f32) {
        let secs = if level == Level::Error { secs.max(5.0) } else { secs };
        self.toasts.push(Toast { text, title, level, until: Instant::now() + Duration::from_secs_f32(secs) });
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
        self.dirty = true;
    }

    // ----- mpv -----
    fn connect(&mut self, spawn: bool) -> bool {
        if self.mpv.alive() {
            return true;
        }
        let tx = self.tx.clone();
        let Some(m) = Mpv::connect(spawn, self.st.volume, move |e| {
            let _ = tx.send(Ev::Mpv(e));
        }) else {
            return false;
        };
        self.mpv = m;
        *self.spec.pid.lock().unwrap() = self.mpv.pid;
        *self.spec.client.lock().unwrap() = self.mpv.client.clone();
        self.ensure_mpris();
        true
    }

    /// MPRIS-сайдкар: кнопки наушников и медиаклавиши. Второй экземпляр сам выйдет — имя на шине занято.
    fn ensure_mpris(&self) {
        use std::os::unix::process::CommandExt;
        if !self.st.mpris {
            return;
        }
        let Ok(exe) = std::env::current_exe() else { return };
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("mpris-daemon")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        if let Ok(mut ch) = cmd.spawn() {
            std::thread::spawn(move || ch.wait());
        }
    }

    fn on_prop(&mut self, name: String, data: Value) {
        self.force_poll = true;
        self.dirty = true;
        match name.as_str() {
            "playlist" => self.on_playlist(data.as_array().cloned().unwrap_or_default()),
            "volume" => {
                if let Some(v) = data.as_f64() {
                    self.st.volume = v.round();
                }
            }
            "media-title" => {
                if let (Some(s), Some(_)) = (data.as_str(), &self.cur_url) {
                    if self.title.text.is_empty() {
                        self.title.set(&nfc(s));
                    }
                }
            }
            _ => {}
        }
        let is_idle = name == "idle-active" && truthy(Some(&data));
        let is_pause = name == "pause" || name == "idle-active";
        if name != "playlist" {
            self.props.insert(name, data);
        }
        if is_pause {
            self.vis_state();
            if is_idle {
                self.idle_panel();
            }
        }
    }

    fn on_closed(&mut self) {
        self.mpv = Mpv::dead();
        self.props.clear();
        self.queue.clear();
        self.cur_url = None;
        self.idle_panel();
        self.vis_state();
        if self.view == 0 {
            self.fill(vec![], false);
        }
        self.dirty = true;
    }

    fn on_playlist(&mut self, pl: Vec<Value>) {
        let old: Vec<String> = self.queue.iter().map(|r| r.url.clone()).collect();
        let mut cur = None;
        let cache = self.sc.cache.lock().unwrap().clone();
        self.queue = pl
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let url = e["filename"].as_str().unwrap_or_default().to_string();
                let mut title = nfc(e["title"].as_str().unwrap_or(url.rsplit('/').next().unwrap_or_default()));
                let mut artist = String::new();
                if let Some((a, t)) = title.split_once(" — ") {
                    (artist, title) = (a.to_string(), t.to_string());
                }
                if e["current"].as_bool() == Some(true) {
                    cur = Some(url.clone());
                    self.qpos = i;
                }
                let duration = cache.get(&url).and_then(|m| m["duration"].as_f64()).map(|d| d / 1000.0);
                Row { url, kind: Kind::Track, duration, title, artist, count: None }
            })
            .collect();
        self.props.insert("playlist".into(), Value::Array(pl));
        if self.view == 0 && self.queue.iter().map(|r| &r.url).ne(old.iter()) {
            self.fill(self.queue.clone(), true);
        }
        if let Some(c) = cur {
            if Some(&c) != self.cur_url.as_ref() {
                self.track_changed(c);
            }
        }
    }

    fn track_changed(&mut self, url: String) {
        self.cur_url = Some(url.clone());
        self.pos = Some(0.0);
        let (title, artist) = self
            .queue
            .iter()
            .find(|r| r.url == url)
            .map(|r| (r.title.clone(), r.artist.clone()))
            .unwrap_or_default();
        self.title.set(&title);
        self.artist = artist;
        self.meta = None;
        self.wave = None;
        self.wave_busy = true;
        self.cover_acc = None;
        self.fetch_meta(url);
        self.vis_state();
    }

    fn fetch_meta(&self, url: String) {
        let (sc, tx) = (self.sc.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let m = match sc.track(&url) {
                Ok(m) => m,
                Err(_) => {
                    let _ = tx.send(Ev::MetaFail(url));
                    return;
                }
            };
            let _ = tx.send(Ev::Meta(url.clone(), m.clone()));
            let art = m["artwork_url"].as_str().or(m["user"]["avatar_url"].as_str());
            let img = art
                .and_then(|a| sc.get(&a.replace("-large", "-t500x500"), false).ok())
                .and_then(|b| image::load_from_memory(&b).ok())
                .map(|i| i.to_rgb8());
            let acc = img.as_ref().map(accent_from);
            let wave = m["waveform_url"]
                .as_str()
                .and_then(|w| sc.get(&w.replace(".png", ".json"), false).ok())
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                .and_then(|v| v["samples"].as_array().map(|a| a.iter().filter_map(|x| x.as_f64()).map(|x| x as f32).collect()));
            let _ = tx.send(Ev::Media { url, img, acc, wave });
        });
    }

    fn apply_meta(&mut self, url: &str, m: Value) {
        if Some(url) != self.cur_url.as_deref() {
            return;
        }
        if let Some(t) = m["title"].as_str() {
            self.title.set(&nfc(t));
        }
        if let Some(a) = m["user"]["username"].as_str() {
            self.artist = nfc(a);
        }
        self.meta = Some(m);
    }

    fn idle_panel(&mut self) {
        if self.prop_b("idle-active") || !self.mpv.alive() {
            self.cur_url = None;
            self.cover.set(None);
            self.title.set(t("idle.title"));
            self.artist = t("idle.sub").into();
            self.meta = None;
            self.wave = None;
            self.wave_busy = false;
            self.pos = None;
            self.fade_accent(self.base_accent(), false);
        }
    }

    pub fn vis_state(&mut self) {
        let on = self.playing() && self.st.vis;
        if on {
            *self.spec.pid.lock().unwrap() = self.mpv.pid;
            *self.spec.client.lock().unwrap() = self.mpv.client.clone();
        }
        self.spec.set_active(on);
        // при выключении не стопаем сразу — дадим столбикам красиво упасть
        if self.st.vis {
            self.vis.running = true;
        }
    }

    // ----- цвет -----
    pub fn base_accent(&self) -> Rgb {
        parse_hex(&self.st.accent).unwrap_or(SC_ORANGE)
    }

    fn target_accent(&self) -> Rgb {
        if self.st.accent == "auto" && self.cur_url.is_some() {
            return self.cover_acc.unwrap_or(SC_ORANGE);
        }
        self.base_accent()
    }

    pub fn fade_accent(&mut self, target: Rgb, force: bool) {
        if target == self.acc_to && !force {
            return;
        }
        self.acc_from = self.accent;
        self.acc_to = target;
        self.acc_start = Some(Instant::now());
    }

    pub fn apply_setting(&mut self, key: &str) {
        match key {
            "lang" => {
                i18n::set_lang(self.st.lang.as_deref());
                if self.cur_url.is_none() {
                    self.idle_panel();
                }
            }
            "accent" => self.fade_accent(self.target_accent(), true),
            "vis" => self.vis_state(),
            "fps" => self.spec.rate.store(self.st.fps.clamp(10, 60), Ordering::Relaxed),
            _ => {}
        }
        self.dirty = true;
    }

    // ----- таймеры -----
    pub fn animating(&self) -> bool {
        self.acc_start.is_some()
            || self.cover.reveal.is_some()
            || (self.wave_busy && self.wave.is_none())
            || self.tab_anim.2.elapsed() < Duration::from_millis(250)
    }

    pub fn deadline(&self) -> Instant {
        let mut d = self.next_poll;
        if self.vis.running || self.animating() {
            d = d.min(self.next_frame);
        }
        if width(&self.title.text) > self.hits.title_w as usize {
            d = d.min(self.title.last + Duration::from_millis(220));
        }
        if let Some(t) = self.toasts.iter().map(|t| t.until).min() {
            d = d.min(t);
        }
        d
    }

    pub fn tick(&mut self) {
        let now = Instant::now();
        if now >= self.next_poll {
            self.next_poll = now + Duration::from_millis(250);
            self.poll_time();
        }
        if self.title.tick(self.hits.title_w as usize) {
            self.dirty = true;
        }
        let n = self.toasts.len();
        self.toasts.retain(|t| t.until > now);
        if self.toasts.len() != n {
            self.dirty = true;
        }
        if now >= self.next_frame {
            let fps = if self.vis.running { self.st.fps.clamp(5, 60) } else { 30 };
            self.next_frame = (self.next_frame + Duration::from_secs_f64(1.0 / fps as f64)).max(now);
            if let Some(s) = self.acc_start {
                let k = s.elapsed().as_secs_f32() / 0.667;
                if k >= 1.0 {
                    self.accent = self.acc_to;
                    self.acc_start = None;
                } else {
                    self.accent = mix(self.acc_from, self.acc_to, ease(k));
                }
                self.dirty = true;
            }
            if self.cover.reveal.is_some() {
                if self.cover.progress() >= 1.0 {
                    self.cover.reveal = None;
                    self.cover.old = None;
                }
                self.dirty = true;
            }
            if self.animating() {
                self.dirty = true;
            }
            if self.vis.running {
                self.vis_tick();
            }
        }
    }

    fn poll_time(&mut self) {
        if !self.mpv.alive() || self.prop_b("idle-active") {
            return;
        }
        if self.prop_b("pause") && self.pos.is_some() && !self.force_poll {
            return;
        }
        self.force_poll = false;
        if let Some(p) = self.mpv.get("time-pos").and_then(|v| v.as_f64()) {
            self.pos = Some(p);
            self.dirty = true;
        }
    }

    pub fn geom(&self) -> (usize, usize) {
        match self.st.vis_style.as_str() {
            "thin" => (1, 1),
            _ => (2, 1),
        }
    }

    fn vis_tick(&mut self) {
        if !self.st.vis {
            self.vis.running = false;
            return;
        }
        let (bw, gap) = self.geom();
        let n = ((self.hits.vis_w as usize + gap) / (bw + gap)).max(8);
        self.spec.bands.store(n, Ordering::Relaxed);
        let playing = self.playing();
        let mut raw = if playing { self.spec.raw.lock().unwrap().clone() } else { vec![0.0; n] };
        if raw.len() != n {
            // интерполяция на новое число полос
            let src = if raw.len() > 1 { raw.clone() } else { vec![0.0; 2] };
            raw = (0..n)
                .map(|i| {
                    let x = i as f32 / (n - 1).max(1) as f32 * (src.len() - 1) as f32;
                    let (a, f) = (x.floor() as usize, x.fract());
                    let b = (a + 1).min(src.len() - 1);
                    src[a] * (1.0 - f) + src[b] * f
                })
                .collect();
        }
        let v = &mut self.vis;
        if v.cur.len() != n {
            v.cur = vec![0.0; n];
            v.peak = vec![0.0; n];
            v.hold = vec![0.0; n];
        }
        // скорости заданы под 20 fps — пересчитываем, чтобы на 30/60 не дёргалось вдвое быстрее
        let k = 20.0 / self.st.fps.clamp(5, 60) as f32;
        let (rise, fall) = (1.0 - 0.4f32.powf(k), 0.84f32.powf(k));
        for i in 0..n {
            let r = raw[i];
            v.cur[i] = if r > v.cur[i] { v.cur[i] + (r - v.cur[i]) * rise } else { v.cur[i] * fall };
            if v.cur[i] > v.peak[i] {
                v.peak[i] = v.cur[i];
                v.hold[i] = 8.0 / k;
            } else {
                v.hold[i] -= 1.0;
            }
            if v.hold[i] <= 0.0 {
                v.peak[i] = (v.peak[i] - 0.025 * k).max(0.0);
            }
        }
        if !playing && v.peak.iter().cloned().fold(0.0, f32::max) < 0.01 && v.cur.iter().cloned().fold(0.0, f32::max) < 0.01 {
            v.running = false;
        }
        self.dirty = true;
    }

    // ----- события -----
    pub fn handle(&mut self, ev: Ev) {
        match ev {
            Ev::Term(Event::Key(k)) => self.on_key(k),
            Ev::Term(Event::Mouse(m)) => self.on_mouse(m),
            Ev::Term(Event::Resize(..)) => self.dirty = true,
            Ev::Term(_) => {}
            Ev::Mpv(MpvEvent::Prop(n, d)) => self.on_prop(n, d),
            Ev::Mpv(MpvEvent::Closed) => self.on_closed(),
            Ev::List { seq, kind, arg, title, rows } => self.list_loaded(seq, kind, arg, title, rows),
            Ev::Meta(url, m) => {
                self.apply_meta(&url, m);
                self.dirty = true;
            }
            Ev::MetaFail(url) => {
                if Some(&url) == self.cur_url.as_ref() {
                    self.wave_busy = false;
                    self.dirty = true;
                }
            }
            Ev::Media { url, img, acc, wave } => {
                if Some(&url) == self.cur_url.as_ref() {
                    self.cover.set(img);
                    self.wave = wave;
                    self.wave_busy = false;
                    self.cover_acc = acc;
                    self.fade_accent(self.target_accent(), false);
                    self.dirty = true;
                }
            }
            Ev::Enqueue(rows, mode) => self.play(rows, 0, mode),
            Ev::Toast(s, l) => self.notify(s, l),
        }
    }

    fn on_key(&mut self, k: KeyEvent) {
        if k.kind == KeyEventKind::Release {
            return;
        }
        self.dirty = true;
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(k.code, KeyCode::Char('c') | KeyCode::Char('q')) {
            self.quit = true;
            return;
        }
        match &mut self.modal {
            Some(Modal::Help) => {
                if matches!(k.code, KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') | KeyCode::Enter) {
                    self.modal = None;
                }
                return;
            }
            Some(Modal::Settings(_)) => {
                self.settings_key(k);
                return;
            }
            None => {}
        }
        if self.focus == Focus::Search {
            match k.code {
                KeyCode::Esc | KeyCode::Tab | KeyCode::Down => self.focus = Focus::Table,
                KeyCode::Enter => {
                    let q = self.query.s.trim().to_string();
                    if !q.is_empty() {
                        self.load_list("search", &q, "");
                    }
                }
                _ => {
                    self.query.key(&k, 200);
                }
            }
            return;
        }
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        match k.code {
            KeyCode::Char(' ') => self.mpv_send(json!(["cycle", "pause"])),
            KeyCode::Char('n') => self.mpv_send(json!(["playlist-next", "force"])),
            KeyCode::Char('p') => self.prev(),
            KeyCode::Left => self.seek(if shift { -30 } else { -5 }),
            KeyCode::Right => self.seek(if shift { 30 } else { 5 }),
            KeyCode::Char('+') | KeyCode::Char('=') => self.vol(5),
            KeyCode::Char('-') => self.vol(-5),
            KeyCode::Char('r') if ctrl => self.reload(),
            KeyCode::Char('r') => self.repeat(),
            KeyCode::Char('s') => self.shuffle(),
            KeyCode::Char('v') => {
                self.st.vis = !self.st.vis;
                self.apply_setting("vis");
            }
            KeyCode::Char('a') => self.enqueue("append-play"),
            KeyCode::Char('e') => self.enqueue("insert-next-play"),
            KeyCode::Char('d') | KeyCode::Delete => self.remove(),
            KeyCode::Char('/') => {
                self.switch_tab(1);
                self.focus = Focus::Search;
            }
            KeyCode::Backspace | KeyCode::Esc => self.back(),
            KeyCode::Char(c @ '1'..='6') => self.switch_tab(c as usize - '1' as usize),
            KeyCode::Tab => self.switch_tab((self.view + 1) % TABS.len()),
            KeyCode::BackTab => self.switch_tab((self.view + TABS.len() - 1) % TABS.len()),
            KeyCode::Char('?') => self.modal = Some(Modal::Help),
            KeyCode::Char('c') | KeyCode::Char(',') => {
                self.modal = Some(Modal::Settings(SettingsState { row: 0, hex: None }))
            }
            KeyCode::Home => self.move_cursor(isize::MIN),
            KeyCode::End => self.move_cursor(isize::MAX),
            KeyCode::Up | KeyCode::Char('k') => self.move_cursor(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_cursor(1),
            KeyCode::PageUp => self.move_cursor(-(self.page() as isize)),
            KeyCode::PageDown => self.move_cursor(self.page() as isize),
            KeyCode::Enter => self.select(self.cursor),
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('Q') => {
                self.mpv.cmd(json!(["quit"]));
                self.quit = true;
            }
            _ => {}
        }
    }

    fn settings_key(&mut self, k: KeyEvent) {
        let Some(Modal::Settings(s)) = &mut self.modal else { return };
        if let Some(hex) = &mut s.hex {
            match k.code {
                KeyCode::Esc => {
                    s.hex = None;
                    self.apply_setting("accent"); // откатить превью
                }
                KeyCode::Enter => {
                    let v = hex.s.clone();
                    match parse_hex(&v) {
                        Some(c) => {
                            s.hex = None;
                            self.st.custom = hexc(c);
                            self.st.accent = hexc(c);
                            self.apply_setting("accent");
                        }
                        None => self.toast(format!("{}{v}", t("n.bad_hex")), None, Level::Warn, 2.0),
                    }
                }
                _ => {
                    if hex.key(&k, 7) {
                        // живое превью, пока печатаешь
                        if let Some(c) = parse_hex(&hex.s) {
                            self.fade_accent(c, false);
                        }
                    }
                }
            }
            return;
        }
        let n = SET_ROWS.len();
        match k.code {
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => s.row = (s.row + n - 1) % n,
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => s.row = (s.row + 1) % n,
            KeyCode::Left | KeyCode::Char('h') => self.settings_change(-1),
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(' ') => self.settings_change(1),
            KeyCode::Enter => {
                if SET_ROWS[s.row] == "accent" {
                    let mut e = LineEdit::default();
                    e.set(&self.st.custom);
                    s.hex = Some(e);
                } else {
                    self.settings_change(1);
                }
            }
            KeyCode::Esc | KeyCode::Char('c') | KeyCode::Char('q') | KeyCode::Char(',') => {
                self.modal = None;
                self.save();
            }
            _ => {}
        }
    }

    pub fn accents() -> Vec<&'static str> {
        let mut v = vec!["auto"];
        v.extend(PALETTE);
        v.push("custom");
        v
    }

    pub fn accent_index(&self) -> usize {
        let a = &self.st.accent;
        if a == "auto" {
            0
        } else if let Some(i) = PALETTE.iter().position(|p| p == a) {
            1 + i
        } else {
            PALETTE.len() + 1
        }
    }

    fn settings_change(&mut self, d: isize) {
        let Some(Modal::Settings(s)) = &self.modal else { return };
        let key = SET_ROWS[s.row];
        let cyc = |i: usize, len: usize| ((i as isize + d).rem_euclid(len as isize)) as usize;
        match key {
            "lang" => {
                let i = i18n::LANGS.iter().position(|(c, _)| *c == i18n::lang()).unwrap_or(0);
                self.st.lang = Some(i18n::LANGS[cyc(i, i18n::LANGS.len())].0.into());
            }
            "vis" => self.st.vis = !self.st.vis,
            "style" => {
                let i = STYLES.iter().position(|x| *x == self.st.vis_style).unwrap_or(0);
                self.st.vis_style = STYLES[cyc(i, STYLES.len())].into();
            }
            "fps" => {
                let i = FPS.iter().position(|x| *x == self.st.fps).unwrap_or(2);
                self.st.fps = FPS[cyc(i, FPS.len())];
            }
            "accent" => {
                let acc = Self::accents();
                let a = acc[cyc(self.accent_index(), acc.len())];
                self.st.accent = if a == "custom" { self.st.custom.clone() } else { a.into() };
                // свой цвет, совпавший с палитрой, иначе зациклится на нём
                if a == "custom" && PALETTE.contains(&self.st.accent.as_str()) {
                    self.st.accent = "#ff5500".into();
                }
            }
            _ => {}
        }
        self.apply_setting(key);
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        let (x, y) = (m.column, m.row);
        let inside = |r: Rect| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
        match m.kind {
            MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                let w = self.hits.wave;
                let h = if inside(w) { Some(x - w.x) } else { None };
                if h != self.hover_x {
                    self.hover_x = h;
                    self.hover = h.map(|h| h as f32 / w.width.max(1) as f32);
                    self.dirty = true;
                }
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp if self.modal.is_none() => {
                if inside(self.hits.table) {
                    let d = if m.kind == MouseEventKind::ScrollDown { 3 } else { -3isize };
                    let max = self.rows.len().saturating_sub(self.page());
                    self.scroll = (self.scroll as isize + d).clamp(0, max as isize) as usize;
                    self.dirty = true;
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.dirty = true;
                match self.modal {
                    Some(Modal::Help) => {
                        self.modal = None;
                        return;
                    }
                    Some(_) => return,
                    None => {}
                }
                if let Some(i) = self.hits.tabs.iter().position(|&(x0, x1, ty)| y == ty && x >= x0 && x < x1) {
                    self.switch_tab(i);
                } else if inside(self.hits.table) {
                    self.focus = Focus::Table;
                    let i = self.scroll + (y - self.hits.table.y) as usize;
                    if i < self.rows.len() {
                        // как в Textual: клик по выделенной строке = выбрать
                        if i == self.cursor {
                            self.select(i);
                        } else {
                            self.cursor = i;
                        }
                    }
                } else if inside(self.hits.wave) && self.mpv.alive() {
                    let f = (x - self.hits.wave.x) as f64 / self.hits.wave.width.max(1) as f64;
                    self.mpv_send(json!(["seek", (f * 10000.0).round() / 100.0, "absolute-percent"]));
                    self.force_poll = true;
                    self.next_poll = Instant::now();
                } else if inside(self.hits.search) {
                    self.focus = Focus::Search;
                }
            }
            _ => {}
        }
    }

    // ----- списки -----
    pub fn page(&self) -> usize {
        (self.hits.table.height as usize).max(1)
    }

    fn move_cursor(&mut self, d: isize) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() as isize - 1;
        self.cursor = (self.cursor as isize).saturating_add(d).clamp(0, last) as usize;
        self.follow();
    }

    /// Прокрутка за курсором.
    pub fn follow(&mut self) {
        let page = self.page();
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + page {
            self.scroll = self.cursor + 1 - page;
        }
    }

    /// После отрисовки: высота таблицы стала известна или поменялась (первый кадр,
    /// ресайз) — докрутить к курсору. Только тогда: иначе прокрутка колесом,
    /// если курсор ушёл за экран, отщёлкивала бы назад. true — надо перерисовать.
    pub fn relayout(&mut self) -> bool {
        let page = self.page();
        if page == self.last_page {
            return false;
        }
        self.last_page = page;
        let s = self.scroll;
        self.follow();
        self.scroll != s
    }

    fn fill(&mut self, rows: Vec<Row>, keep: bool) {
        let mut cur = if keep { self.cursor } else { 0 };
        self.row_index.clear();
        for (i, r) in rows.iter().enumerate() {
            self.row_index.entry(r.url.clone()).or_insert(i);
        }
        if !keep && self.view == 0 && self.cur_url.is_some() {
            cur = self.qpos;
        }
        self.rows = rows;
        if !keep {
            self.scroll = 0;
        }
        self.cursor = cur.min(self.rows.len().saturating_sub(1));
        self.follow();
        self.dirty = true;
    }

    pub fn marker_row(&self) -> Option<usize> {
        let url = self.cur_url.as_ref()?;
        if self.view == 0 { Some(self.qpos) } else { self.row_index.get(url).copied() }
    }

    fn switch_tab(&mut self, i: usize) {
        if i != self.view {
            self.show_view(i);
        }
    }

    fn show_view(&mut self, v: usize) {
        if self.view < TABS.len() {
            self.tab_anim = (self.tab_anim.1, v as f32, Instant::now());
        } else {
            self.tab_anim = (v as f32, v as f32, Instant::now() - Duration::from_secs(1));
        }
        self.view = v;
        self.focus = Focus::Table;
        let name = TABS[v];
        match name {
            "queue" => self.fill(self.queue.clone(), false),
            "search" => {
                let l = self.lists.get("search").cloned().unwrap_or_default();
                if l.is_empty() {
                    self.focus = Focus::Search;
                }
                self.fill(l, false);
            }
            "sets" if self.set_open.is_some() => {
                let l = self.set_open.as_ref().unwrap().1.clone();
                self.fill(l, false);
            }
            _ => match self.lists.get(name).cloned() {
                Some(l) => self.fill(l, false),
                None => {
                    self.fill(vec![], false);
                    self.load_list(name, "", "");
                }
            },
        }
    }

    fn load_list(&mut self, kind: &str, arg: &str, title: &str) {
        self.list_gen += 1;
        self.loading = Some(kind.to_string());
        self.dirty = true;
        let (sc, tx, seq) = (self.sc.clone(), self.tx.clone(), self.list_gen);
        let (kind, arg, title) = (kind.to_string(), arg.to_string(), title.to_string());
        std::thread::spawn(move || {
            let k = if ["search", "feed", "likes", "tracks", "sets"].contains(&kind.as_str()) { kind.as_str() } else { "set" };
            let rows = sc.listing_any(k, &arg).map_err(|e| e.to_string());
            let _ = tx.send(Ev::List { seq, kind, arg, title, rows });
        });
    }

    fn list_loaded(&mut self, seq: u64, kind: String, arg: String, title: String, rows: Result<Vec<Row>, String>) {
        self.dirty = true;
        if seq == self.list_gen {
            self.loading = None;
        }
        let rows = match rows {
            Ok(r) => r,
            Err(e) => {
                let line: String = e.lines().next().unwrap_or_default().chars().take(200).collect();
                self.toast(line, Some(t("n.load_fail").trim_end_matches([':', ' ']).into()), Level::Error, 5.0);
                return;
            }
        };
        match kind.as_str() {
            "url" => self.play(rows, 0, "replace"),
            "set" => {
                self.set_open = Some((title, rows));
                if self.view == 4 {
                    self.show_view(4);
                }
            }
            _ => {
                let empty = rows.is_empty();
                if self.view < TABS.len() && TABS[self.view] == kind {
                    self.fill(rows.clone(), false);
                    self.focus = Focus::Table;
                }
                self.lists.insert(kind, rows);
                let _ = arg;
                if empty {
                    self.notify(t("n.empty"), Level::Info);
                }
            }
        }
    }

    fn select(&mut self, i: usize) {
        let Some(r) = self.rows.get(i).cloned() else { return };
        if self.view == 0 {
            self.mpv_send(json!(["playlist-play-index", i]));
            self.mpv_send(json!(["set_property", "pause", false]));
        } else if r.kind == Kind::Set {
            self.load_list("set", &r.url, &r.title);
        } else {
            self.play(self.rows.clone(), i, "replace");
        }
    }

    fn play(&mut self, tracks: Vec<Row>, start: usize, mode: &str) {
        if mode != "replace" {
            return self.play_tracks(tracks, start, mode);
        }
        // старт считаем по исходному списку, плейлисты из него выкидываем
        let skip = tracks[..start.min(tracks.len())].iter().filter(|r| r.kind == Kind::Set).count();
        let only: Vec<Row> = tracks.into_iter().filter(|r| r.kind == Kind::Track).collect();
        self.play_tracks(only, start.saturating_sub(skip), mode)
    }

    fn play_tracks(&mut self, tracks: Vec<Row>, start: usize, mode: &str) {
        if tracks.is_empty() {
            return;
        }
        let path = match write_m3u(&tracks) {
            Ok(p) => p,
            Err(e) => return self.notify(e.to_string(), Level::Error),
        };
        if !self.connect(true) {
            let _ = std::fs::remove_file(&path);
            return self.notify(t("n.mpv_fail"), Level::Error);
        }
        let p = path.to_string_lossy().to_string();
        if mode == "replace" {
            self.mpv.cmd(json!(["set_property", "pause", true]));
            self.mpv.cmd(json!(["loadlist", p, "replace"]));
            self.mpv.cmd(json!(["playlist-play-index", start]));
            self.mpv.cmd(json!(["set_property", "pause", false]));
        } else {
            self.mpv.cmd(json!(["loadlist", p, mode]));
        }
        let _ = std::fs::remove_file(&path);
        let msg = if mode == "replace" {
            let q = if tracks.len() > 1 { i18n::queued(tracks.len()) } else { String::new() };
            format!("▶ {}{q}", tracks[start.min(tracks.len() - 1)].title)
        } else {
            let what = if tracks.len() == 1 { tracks[0].title.clone() } else { i18n::count_tracks(tracks.len()) };
            format!("{}{what}", t(if mode == "append-play" { "n.append" } else { "n.insert" }))
        };
        self.notify(msg, Level::Info);
    }

    // ----- действия -----
    fn mpv_send(&self, cmd: Value) {
        self.mpv.send(cmd);
    }

    fn prev(&mut self) {
        // как в нормальных плеерах: сначала в начало трека
        if self.pos.unwrap_or(0.0) > 5.0 {
            self.mpv_send(json!(["seek", 0, "absolute"]));
        } else {
            self.mpv_send(json!(["playlist-prev", "force"]));
        }
    }

    fn seek(&mut self, s: i32) {
        self.mpv_send(json!(["seek", s]));
        self.force_poll = true;
        self.next_poll = Instant::now() + Duration::from_millis(30);
    }

    fn vol(&mut self, d: i32) {
        let v = self.prop_f("volume").unwrap_or(self.st.volume) as i32;
        self.mpv_send(json!(["set_property", "volume", (v + d).clamp(0, 100)]));
    }

    fn repeat(&mut self) {
        if self.prop_str("loop-file") == "inf" {
            self.mpv_send(json!(["set_property", "loop-file", "no"]));
            self.mpv_send(json!(["set_property", "loop-playlist", "no"]));
        } else if self.prop_str("loop-playlist") == "inf" {
            self.mpv_send(json!(["set_property", "loop-file", "inf"]));
        } else {
            self.mpv_send(json!(["set_property", "loop-playlist", "inf"]));
        }
    }

    fn shuffle(&mut self) {
        if self.mpv.alive() {
            self.mpv_send(json!(["playlist-shuffle"]));
            self.toast(t("n.shuffled").into(), None, Level::Info, 1.5);
        }
    }

    fn enqueue(&mut self, mode: &'static str) {
        if self.view == 0 {
            return;
        }
        let Some(r) = self.rows.get(self.cursor).cloned() else { return };
        if r.kind == Kind::Set {
            let (sc, tx) = (self.sc.clone(), self.tx.clone());
            std::thread::spawn(move || match sc.listing_any("set", &r.url) {
                Ok(rows) => {
                    let _ = tx.send(Ev::Enqueue(rows, mode));
                }
                Err(e) => {
                    let _ = tx.send(Ev::Toast(e.to_string().chars().take(200).collect(), Level::Error));
                }
            });
        } else {
            self.play_tracks(vec![r], 0, mode);
        }
    }

    fn remove(&mut self) {
        if self.view == 0 && self.cursor < self.rows.len() {
            self.mpv_send(json!(["playlist-remove", self.cursor]));
        }
    }

    fn back(&mut self) {
        if self.view == 4 && self.set_open.is_some() {
            self.set_open = None;
            self.show_view(4);
        }
    }

    fn reload(&mut self) {
        if (2..=4).contains(&self.view) && self.set_open.is_none() {
            self.lists.remove(TABS[self.view]);
            self.show_view(self.view);
        }
    }
}
