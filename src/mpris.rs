//! MPRIS-сайдкар: mpv ↔ D-Bus. Кнопки наушников/клавиатуры, виджет плеера KDE/GNOME, lock screen.
//!
//! Живёт отдельным процессом рядом с mpv (UI можно закрыть), выходит, когда mpv завершается.
//! Один на сокет: если имя на шине уже занято — сразу выходит.
use crate::config;
use crate::soundcloud::SoundCloud;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex, OnceLock};
use zbus::interface;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Value as ZV};

const BUS: &str = "org.mpris.MediaPlayer2.sc";
const PATH: &str = "/org/mpris/MediaPlayer2";
const OBSERVE: [&str; 10] = [
    "pause", "idle-active", "loop-file", "loop-playlist", "volume", "media-title", "path",
    "duration", "playlist-pos", "playlist-count",
];

#[derive(Default)]
struct Inner {
    p: HashMap<String, Value>,
    /// url → file:// путь обложки
    art: HashMap<String, String>,
    /// url, title, artist из api — точнее, чем media-title
    meta: Option<(String, String, String)>,
}

type St = Arc<Mutex<Inner>>;

struct Mpv(Mutex<UnixStream>);

impl Mpv {
    fn send(&self, cmd: Value) {
        let _ = self.0.lock().unwrap().write_all(format!("{}\n", json!({ "command": cmd })).as_bytes());
    }
}

fn time_pos() -> f64 {
    config::ctl(json!(["get_property", "time-pos"])).and_then(|v| v.as_f64()).unwrap_or(0.0)
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s != "no",
        _ => true,
    }
}

fn ov(v: ZV<'_>) -> OwnedValue {
    v.try_to_owned().expect("plain value")
}

impl Inner {
    fn status(&self) -> String {
        if truthy(self.p.get("idle-active")) {
            "Stopped".into()
        } else if truthy(self.p.get("pause")) {
            "Paused".into()
        } else {
            "Playing".into()
        }
    }
    fn loop_status(&self) -> String {
        if truthy(self.p.get("loop-file")) {
            "Track".into()
        } else if truthy(self.p.get("loop-playlist")) {
            "Playlist".into()
        } else {
            "None".into()
        }
    }
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        let s = |k: &str| self.p.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let url = s("path");
        let mut title = s("media-title");
        if title.is_empty() {
            title = url.rsplit('/').next().unwrap_or_default().to_string();
        }
        let mut artist = String::new();
        match &self.meta {
            Some((u, t, a)) if *u == url && !url.is_empty() => {
                if !t.is_empty() {
                    title = t.clone();
                }
                artist = a.clone();
            }
            _ => {
                if let Some((a, t)) = title.split_once(" — ") {
                    (artist, title) = (a.to_string(), t.to_string());
                }
            }
        }
        let pos = self.p.get("playlist-pos").and_then(|v| v.as_i64()).unwrap_or(0).max(0);
        let tid = format!("/org/mpris/MediaPlayer2/sc/track/{pos}");
        let mut md = HashMap::new();
        md.insert("mpris:trackid".into(), ov(ZV::from(ObjectPath::try_from(tid.as_str()).unwrap())));
        md.insert("xesam:title".into(), ov(ZV::from(crate::util::nfc(&title))));
        md.insert("xesam:artist".into(), ov(ZV::from(if artist.is_empty() { vec![] } else { vec![crate::util::nfc(&artist)] })));
        md.insert("xesam:url".into(), ov(ZV::from(url.clone())));
        if let Some(d) = self.p.get("duration").and_then(|v| v.as_f64()) {
            md.insert("mpris:length".into(), ov(ZV::from((d * 1e6) as i64)));
        }
        if let Some(a) = self.art.get(&url) {
            md.insert("mpris:artUrl".into(), ov(ZV::from(a.clone())));
        }
        md
    }
}

struct Root(Arc<Mpv>);

#[interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {}
    fn quit(&self) {
        self.0.send(json!(["quit"]));
    }
    #[zbus(property)]
    fn can_quit(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_raise(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn identity(&self) -> &str {
        "SoundCloud (sc)"
    }
    #[zbus(property)]
    fn desktop_entry(&self) -> &str {
        "sc"
    }
    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        vec!["https".into()]
    }
    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        vec![]
    }
}

struct Player {
    st: St,
    mpv: Arc<Mpv>,
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    fn next(&self) {
        self.mpv.send(json!(["playlist-next", "force"]));
    }
    fn previous(&self) {
        if time_pos() > 5.0 {
            self.mpv.send(json!(["seek", 0, "absolute"]));
        } else {
            self.mpv.send(json!(["playlist-prev", "force"]));
        }
    }
    fn pause(&self) {
        self.mpv.send(json!(["set_property", "pause", true]));
    }
    fn play_pause(&self) {
        self.mpv.send(json!(["cycle", "pause"]));
    }
    /// Stop у гарнитур — не повод сносить очередь
    fn stop(&self) {
        self.pause();
    }
    fn play(&self) {
        self.mpv.send(json!(["set_property", "pause", false]));
    }
    fn seek(&self, offset: i64) {
        self.mpv.send(json!(["seek", offset as f64 / 1e6, "relative"]));
    }
    fn set_position(&self, _track: ObjectPath<'_>, pos: i64) {
        self.mpv.send(json!(["seek", pos as f64 / 1e6, "absolute"]));
    }
    fn open_uri(&self, uri: &str) {
        self.mpv.send(json!(["loadfile", uri, "replace"]));
    }

    #[zbus(signal)]
    async fn seeked(emitter: &SignalEmitter<'_>, pos: i64) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> String {
        self.st.lock().unwrap().status()
    }
    #[zbus(property)]
    fn loop_status(&self) -> String {
        self.st.lock().unwrap().loop_status()
    }
    #[zbus(property)]
    fn set_loop_status(&mut self, v: String) {
        self.mpv.send(json!(["set_property", "loop-file", if v == "Track" { "inf" } else { "no" }]));
        let pl = if v == "Track" || v == "Playlist" { "inf" } else { "no" };
        self.mpv.send(json!(["set_property", "loop-playlist", pl]));
    }
    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn set_rate(&mut self, _v: f64) {}
    #[zbus(property)]
    fn shuffle(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn set_shuffle(&mut self, v: bool) {
        if v {
            self.mpv.send(json!(["playlist-shuffle"]));
        }
    }
    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        self.st.lock().unwrap().metadata()
    }
    #[zbus(property)]
    fn volume(&self) -> f64 {
        self.st.lock().unwrap().p.get("volume").and_then(|v| v.as_f64()).unwrap_or(0.0) / 100.0
    }
    #[zbus(property)]
    fn set_volume(&mut self, v: f64) {
        self.mpv.send(json!(["set_property", "volume", (v * 100.0).clamp(0.0, 100.0)]));
    }
    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        (time_pos() * 1e6) as i64
    }
    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_play(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_pause(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_seek(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
}

enum Ev {
    Prop(String, Value),
    Seeked,
}

/// Обложку тянем сами: UI может быть закрыт.
fn fetch_art(st: &St, url: &str) -> Option<()> {
    static SC: OnceLock<SoundCloud> = OnceLock::new();
    if !url.starts_with("http") || st.lock().unwrap().art.contains_key(url) {
        return None;
    }
    let sc = SC.get_or_init(SoundCloud::new);
    let m = sc.track(url).ok()?;
    let art = m["artwork_url"].as_str().or(m["user"]["avatar_url"].as_str())?;
    let dir = config::run_dir().join("sc-art");
    std::fs::create_dir_all(&dir).ok()?;
    let id = m["id"].as_u64().map(|i| i.to_string()).unwrap_or_else(|| format!("{:x}", fxhash(url)));
    let path = dir.join(format!("{id}.jpg"));
    if !path.exists() {
        let data = sc.get(&art.replace("-large", "-t500x500"), false).ok()?;
        let tmp = path.with_extension("jpg.tmp");
        std::fs::write(&tmp, data).ok()?;
        std::fs::rename(&tmp, &path).ok()?;
    }
    // держим 8 последних
    let mut old: Vec<_> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jpg"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    old.sort();
    for (_, p) in old.iter().rev().skip(8) {
        let _ = std::fs::remove_file(p);
    }
    let mut g = st.lock().unwrap();
    g.art.insert(url.to_string(), format!("file://{}", path.display()));
    let s = |v: &Value| v.as_str().unwrap_or_default().to_string();
    g.meta = Some((url.to_string(), s(&m["title"]), s(&m["user"]["username"])));
    Some(())
}

/// Свой сокет ($SC_SOCK) — свой плеер на шине, чтобы не драться с основным.
fn bus_name() -> String {
    let sock = config::sock();
    if sock == config::run_dir().join("sc-mpv.sock") {
        BUS.into()
    } else {
        format!("{BUS}.i{:x}", fxhash(&sock.to_string_lossy()))
    }
}

fn fxhash(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

pub fn main() -> anyhow::Result<()> {
    let sock = UnixStream::connect(config::sock()).map_err(|_| anyhow::anyhow!("mpris: mpv is not running"))?;
    let mpv = Arc::new(Mpv(Mutex::new(sock.try_clone()?)));
    let st: St = Default::default();
    // имя занято — уже есть сайдкар, выходим тихо
    let Ok(conn) = zbus::blocking::connection::Builder::session()?
        .allow_name_replacements(false)
        .replace_existing_names(false)
        .name(bus_name())?
        .serve_at(PATH, Root(mpv.clone()))?
        .serve_at(PATH, Player { st: st.clone(), mpv: mpv.clone() })?
        .build()
    else {
        return Ok(());
    };

    let (tx, rx) = std::sync::mpsc::channel::<Ev>();
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut rd = BufReader::new(sock);
            let mut line = String::new();
            while matches!(rd.read_line(&mut line), Ok(n) if n > 0) {
                if let Ok(msg) = serde_json::from_str::<Value>(&line) {
                    if msg["event"] == "property-change" {
                        let name = msg["name"].as_str().unwrap_or_default().to_string();
                        let _ = tx.send(Ev::Prop(name, msg.get("data").cloned().unwrap_or(Value::Null)));
                    } else if msg["event"] == "playback-restart" {
                        let _ = tx.send(Ev::Seeked); // после перемотки
                    }
                }
                line.clear();
            }
            std::process::exit(0); // mpv закрылся — нам тоже пора
        });
    }
    for (i, name) in OBSERVE.iter().enumerate() {
        mpv.send(json!(["observe_property", i + 1, name]));
    }

    let iface = conn.object_server().interface::<_, Player>(PATH)?;
    let em = iface.signal_emitter().clone();
    for ev in rx {
        let (name, data) = match ev {
            Ev::Seeked => {
                let _ = zbus::block_on(Player::seeked(&em, (time_pos() * 1e6) as i64));
                continue;
            }
            Ev::Prop(n, d) => (n, d),
        };
        if name == "path" {
            if let Some(url) = data.as_str().map(String::from) {
                let (st2, tx2) = (st.clone(), tx.clone());
                std::thread::spawn(move || {
                    if fetch_art(&st2, &url).is_some() {
                        let _ = tx2.send(Ev::Prop("__art__".into(), Value::Null));
                    }
                });
            }
        }
        if name != "__art__" {
            st.lock().unwrap().p.insert(name.clone(), data);
        }
        let g = iface.get();
        let _ = zbus::block_on(async {
            match name.as_str() {
                "pause" | "idle-active" => g.playback_status_changed(&em).await,
                "loop-file" | "loop-playlist" => g.loop_status_changed(&em).await,
                "volume" => g.volume_changed(&em).await,
                "media-title" | "path" | "duration" | "__art__" => g.metadata_changed(&em).await,
                "playlist-pos" | "playlist-count" => {
                    g.can_go_next_changed(&em).await?;
                    g.can_go_previous_changed(&em).await
                }
                _ => Ok(()),
            }
        });
    }
    Ok(())
}
