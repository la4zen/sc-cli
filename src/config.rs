//! Пути, state.json, синхронный пульт к mpv, поиск профиля Firefox.
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

fn home() -> PathBuf {
    PathBuf::from(env("HOME").unwrap_or_else(|| "/tmp".into()))
}

pub fn run_dir() -> PathBuf {
    PathBuf::from(env("XDG_RUNTIME_DIR").unwrap_or_else(|| "/tmp".into()))
}

pub fn sock() -> PathBuf {
    env("SC_SOCK").map(PathBuf::from).unwrap_or_else(|| run_dir().join("sc-mpv.sock"))
}

pub fn sc_user() -> Option<String> {
    env("SC_USER")
}

pub fn state_file() -> PathBuf {
    env("SC_STATE").map(PathBuf::from).unwrap_or_else(|| home().join(".config/sc/state.json"))
}

pub fn cid_cache() -> PathBuf {
    home().join(".cache/yt-dlp/soundcloud/client_id.json")
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct State {
    pub volume: f64,
    pub vis: bool,
    pub lang: Option<String>,
    /// "auto" — из обложки, иначе "#rrggbb"
    pub accent: String,
    pub custom: String,
    /// bars | thin | mirror
    pub vis_style: String,
    pub fps: u32,
    pub mpris: bool,
}

impl Default for State {
    fn default() -> Self {
        State {
            volume: 100.0,
            vis: true,
            lang: None,
            accent: "auto".into(),
            custom: "#ff5500".into(),
            vis_style: "bars".into(),
            fps: 30,
            mpris: true,
        }
    }
}

impl State {
    pub fn load() -> State {
        std::fs::read_to_string(state_file())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let p = state_file();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap_or_default())?;
        std::fs::rename(tmp, p)
    }
}

/// Синхронная команда в mpv с ответом. None — плеер не запущен или ошибка.
pub fn ctl(cmd: serde_json::Value) -> Option<serde_json::Value> {
    let mut s = UnixStream::connect(sock()).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let msg = serde_json::json!({"command": cmd, "request_id": 1});
    s.write_all(format!("{msg}\n").as_bytes()).ok()?;
    let mut r = BufReader::new(s);
    let mut line = String::new();
    loop {
        line.clear();
        if r.read_line(&mut line).ok()? == 0 {
            return None;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
            if v.get("request_id").and_then(|x| x.as_u64()) == Some(1) {
                return if v["error"] == "success" { Some(v["data"].clone()) } else { None };
            }
        }
    }
}

/// Профиль Firefox с куками: $SC_FIREFOX_PROFILE или тот, что открывается по умолчанию.
pub fn ff_profile() -> Option<PathBuf> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    P.get_or_init(|| {
        if let Some(p) = env("SC_FIREFOX_PROFILE") {
            return Some(PathBuf::from(p));
        }
        let bases = [
            ".config/mozilla/firefox",
            ".mozilla/firefox",
            "snap/firefox/common/.mozilla/firefox",
            ".var/app/org.mozilla.firefox/.mozilla/firefox",
        ];
        for b in bases {
            let base = home().join(b);
            let Ok(ini) = std::fs::read_to_string(base.join("profiles.ini")) else { continue };
            // порядок: [Install…] Default= → профиль с Default=1 → остальные
            let (mut first, mut def, mut rest) = (vec![], vec![], vec![]);
            let mut sec = String::new();
            let (mut path, mut is_def) = (None::<String>, false);
            let mut flush = |sec: &str, path: &mut Option<String>, is_def: &mut bool| {
                if sec.starts_with("Profile") {
                    if let Some(p) = path.take() {
                        if *is_def { def.push(p) } else { rest.push(p) }
                    }
                }
                *path = None;
                *is_def = false;
            };
            for line in ini.lines().map(str::trim) {
                if line.starts_with('[') {
                    flush(&sec, &mut path, &mut is_def);
                    sec = line.trim_matches(|c| c == '[' || c == ']').to_string();
                } else if let Some((k, v)) = line.split_once('=') {
                    match (k, sec.starts_with("Install")) {
                        ("Default", true) => first.push(v.to_string()),
                        ("Path", false) => path = Some(v.to_string()),
                        ("Default", false) => is_def = v == "1",
                        _ => {}
                    }
                }
            }
            flush(&sec, &mut path, &mut is_def);
            for p in first.into_iter().chain(def).chain(rest) {
                let full = base.join(&p); // абсолютный путь join не испортит
                if full.join("cookies.sqlite").exists() {
                    return Some(full);
                }
            }
        }
        None
    })
    .clone()
}

/// Есть ли у yt-dlp Chrome-impersonate (нужен curl_cffi). CloudFront перед
/// api-v2 режет питоновский TLS-отпечаток yt-dlp и отдаёт 403 — с ним проходит.
pub fn ytdl_impersonate() -> Option<&'static str> {
    static I: OnceLock<bool> = OnceLock::new();
    let ok = *I.get_or_init(|| {
        std::process::Command::new("yt-dlp")
            .args(["--ignore-config", "--list-impersonate-targets"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .is_ok_and(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .any(|l| l.starts_with("Chrome") && !l.contains("unavailable"))
            })
    });
    ok.then_some("chrome")
}
