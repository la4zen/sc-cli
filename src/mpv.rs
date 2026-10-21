//! mpv по JSON IPC: поток-читатель шлёт property-change в канал приложения.
use crate::config;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const OBSERVE: [&str; 10] = [
    "pause", "playlist", "duration", "volume", "loop-file", "loop-playlist",
    "idle-active", "media-title", "paused-for-cache", "demuxer-cache-time",
];

pub enum MpvEvent {
    Prop(String, Value),
    Closed,
}

type Pending = Arc<Mutex<HashMap<u64, Sender<Value>>>>;

pub struct Mpv {
    w: Option<Mutex<UnixStream>>,
    pending: Pending,
    rid: Mutex<u64>,
    pub pid: Option<u64>,
    pub client: String,
}

impl Mpv {
    pub fn dead() -> Mpv {
        Mpv { w: None, pending: Default::default(), rid: Mutex::new(0), pid: None, client: "sc-mpv".into() }
    }

    pub fn alive(&self) -> bool {
        self.w.is_some()
    }

    /// Подключиться к mpv (или поднять свой). `tx` получает события.
    pub fn connect<F>(spawn: bool, volume: f64, wrap: F) -> Option<Mpv>
    where
        F: Fn(MpvEvent) + Send + 'static,
    {
        let s = match UnixStream::connect(config::sock()) {
            Ok(s) => s,
            Err(_) if spawn => {
                let mut cmd = Command::new("mpv");
                cmd.args([
                    "--no-video", "--no-terminal", "--idle=yes", "--loop-playlist=inf",
                    "--prefetch-playlist=yes", "--ytdl-format=bestaudio/best", "--demuxer-max-bytes=48MiB",
                    "--audio-display=no", "--audio-client-name=sc-mpv",
                ])
                .arg(format!("--volume={volume}"))
                .args(config::ytdl_impersonate().map(|i| format!("--ytdl-raw-options-append=impersonate={i}")))
                .arg(format!("--input-ipc-server={}", config::sock().display()))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
                // своя сессия: UI закрыли — музыка играет
                unsafe {
                    cmd.pre_exec(|| {
                        libc::setsid();
                        Ok(())
                    });
                }
                let mut child = cmd.spawn().ok()?;
                let mut got = None;
                for _ in 0..80 {
                    std::thread::sleep(Duration::from_millis(100));
                    if let Ok(s) = UnixStream::connect(config::sock()) {
                        got = Some(s);
                        break;
                    }
                }
                // зомби не плодим: дождёмся в фоне
                std::thread::spawn(move || child.wait());
                got?
            }
            Err(_) => return None,
        };
        let pending: Pending = Default::default();
        let r = s.try_clone().ok()?;
        let p2 = pending.clone();
        std::thread::spawn(move || {
            let mut rd = BufReader::with_capacity(1 << 16, r);
            let mut line = String::new();
            loop {
                line.clear();
                match rd.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
                if let Some(id) = msg.get("request_id").and_then(Value::as_u64) {
                    if let Some(tx) = p2.lock().unwrap().remove(&id) {
                        let _ = tx.send(msg);
                    }
                } else if msg["event"] == "property-change" {
                    let name = msg["name"].as_str().unwrap_or_default().to_string();
                    wrap(MpvEvent::Prop(name, msg.get("data").cloned().unwrap_or(Value::Null)));
                }
            }
            p2.lock().unwrap().clear();
            wrap(MpvEvent::Closed);
        });
        let mut m = Mpv { w: Some(Mutex::new(s)), pending, rid: Mutex::new(0), pid: None, client: "mpv".into() };
        for (i, name) in OBSERVE.iter().enumerate() {
            m.send(json!(["observe_property", i + 1, name]));
        }
        m.pid = m.get("pid").and_then(|v| v.as_u64());
        m.client = m.get("audio-client-name").and_then(|v| v.as_str().map(String::from)).unwrap_or("mpv".into());
        Some(m)
    }

    fn next_id(&self) -> u64 {
        let mut g = self.rid.lock().unwrap();
        *g += 1;
        *g
    }

    fn write(&self, msg: Value) -> bool {
        let Some(w) = &self.w else { return false };
        w.lock().unwrap().write_all(format!("{msg}\n").as_bytes()).is_ok()
    }

    /// Без ожидания ответа.
    pub fn send(&self, cmd: Value) {
        let id = self.next_id();
        self.write(json!({"command": cmd, "request_id": id}));
    }

    /// С ответом; None — ошибка/таймаут.
    pub fn cmd(&self, cmd: Value) -> Option<Value> {
        let id = self.next_id();
        let (tx, rx) = mpsc::channel();
        self.pending.lock().unwrap().insert(id, tx);
        if !self.write(json!({"command": cmd, "request_id": id})) {
            self.pending.lock().unwrap().remove(&id);
            return None;
        }
        let msg = rx.recv_timeout(Duration::from_secs(5)).ok()?;
        (msg["error"] == "success").then(|| msg.get("data").cloned().unwrap_or(Value::Null))
    }

    pub fn get(&self, prop: &str) -> Option<Value> {
        self.cmd(json!(["get_property", prop]))
    }
}
