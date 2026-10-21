//! SoundCloud: метаданные и списки через api-v2, фоллбэк — yt-dlp.
use crate::config;
use crate::i18n::t;
use crate::util::{nfc, Rgb, SC_ORANGE};
use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

const UA: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:140.0) Gecko/20100101 Firefox/140.0";
const API: &str = "https://api-v2.soundcloud.com/";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Track,
    Set,
}

#[derive(Clone, Debug)]
pub struct Row {
    pub url: String,
    pub kind: Kind,
    pub duration: Option<f64>,
    pub title: String,
    pub artist: String,
    pub count: Option<u64>,
}

pub struct SoundCloud {
    token: Option<String>,
    cid: Mutex<Option<String>>,
    pub cache: Mutex<HashMap<String, Value>>,
    me: Mutex<Option<Value>>,
    agent: ureq::Agent,
}

fn percent(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

fn track_id(url: &str) -> Option<u64> {
    for pat in ["tracks%3A", "tracks:", "tracks/"] {
        if let Some(i) = url.find(pat) {
            let digits: String = url[i + pat.len()..].chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = digits.parse() {
                return Some(n);
            }
        }
    }
    None
}

impl SoundCloud {
    pub fn new() -> SoundCloud {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(15)))
            .build()
            .into();
        SoundCloud {
            token: read_token(),
            cid: Mutex::new(None),
            cache: Mutex::new(HashMap::new()),
            me: Mutex::new(None),
            agent,
        }
    }

    fn cid(&self, refresh: bool) -> Option<String> {
        if refresh {
            // yt-dlp сам перевыпускает client_id, когда старый протух
            let mut cmd = Command::new("yt-dlp");
            cmd.args(["--ignore-config", "-q", "--no-warnings", "--flat-playlist", "-J", "scsearch1:a"]);
            if let Some(i) = config::ytdl_impersonate() {
                cmd.args(["--impersonate", i]);
            }
            let _ = cmd
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let mut g = self.cid.lock().unwrap();
        if refresh || g.is_none() {
            *g = std::fs::read_to_string(config::cid_cache())
                .ok()
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .and_then(|v| v["data"].as_str().map(String::from));
        }
        g.clone()
    }

    pub fn get(&self, url: &str, auth: bool) -> Result<Vec<u8>> {
        let mut req = self.agent.get(url).header("User-Agent", UA);
        if auth {
            if let Some(tok) = &self.token {
                req = req.header("Authorization", &format!("OAuth {tok}"));
            }
        }
        let mut resp = req.call()?;
        Ok(resp.body_mut().with_config().limit(64 << 20).read_to_vec()?)
    }

    /// GET к api-v2; на 401/403 — свежий client_id и ещё раз.
    fn api_url(&self, url: &str) -> Result<Value> {
        for attempt in 0..2 {
            let cid = self.cid(attempt > 0).unwrap_or_default();
            let sep = if url.contains('?') { '&' } else { '?' };
            let full = if url.contains("client_id=") { url.to_string() } else { format!("{url}{sep}client_id={cid}") };
            match self.get(&full, true) {
                Ok(b) => return Ok(serde_json::from_slice(&b)?),
                Err(e) => {
                    if attempt == 0 {
                        if let Some(ureq::Error::StatusCode(401 | 403)) = e.downcast_ref::<ureq::Error>() {
                            continue;
                        }
                    }
                    return Err(e);
                }
            }
        }
        unreachable!()
    }

    pub fn api(&self, path: &str, q: &[(&str, &str)]) -> Result<Value> {
        let qs: Vec<String> = q.iter().map(|(k, v)| format!("{k}={}", percent(v))).collect();
        self.api_url(&format!("{API}{path}?{}", qs.join("&")))
    }

    /// Пагинация api-v2 по next_href.
    fn pages(&self, path: &str, cap: usize, q: &[(&str, &str)]) -> Result<Vec<Value>> {
        let mut q = q.to_vec();
        if !q.iter().any(|(k, _)| *k == "limit") {
            q.push(("limit", "200"));
        }
        q.push(("linked_partitioning", "1"));
        let mut d = self.api(path, &q)?;
        let mut items: Vec<Value> = d["collection"].as_array().cloned().unwrap_or_default();
        while let Some(next) = d["next_href"].as_str().map(String::from) {
            if items.len() >= cap {
                break;
            }
            d = self.api_url(&next)?;
            items.extend(d["collection"].as_array().cloned().unwrap_or_default());
        }
        Ok(items)
    }

    fn row(&self, o: &Value) -> Row {
        let o = if o.get("track").is_some_and(|v| v.is_object()) {
            &o["track"]
        } else if o.get("playlist").is_some_and(|v| v.is_object()) {
            &o["playlist"]
        } else {
            o
        };
        let url = o["permalink_url"].as_str().unwrap_or_default().to_string();
        let dur = o["duration"].as_f64().map(|d| d / 1000.0);
        let artist = nfc(o["user"]["username"].as_str().unwrap_or_default());
        let title = nfc(o["title"].as_str().unwrap_or_default());
        if o["kind"] == "playlist" {
            return Row { url, kind: Kind::Set, duration: dur, title, artist, count: o["track_count"].as_u64() };
        }
        if !title.is_empty() {
            self.cache.lock().unwrap().insert(url.clone(), o.clone());
        }
        Row { url, kind: Kind::Track, duration: dur, title, artist, count: None }
    }

    /// Чей профиль: $SC_USER, иначе владелец куки.
    fn me(&self) -> Result<Value> {
        if let Some(m) = self.me.lock().unwrap().clone() {
            return Ok(m);
        }
        let m = if let Some(u) = config::sc_user() {
            self.api("resolve", &[("url", &format!("https://soundcloud.com/{u}"))])?
        } else if self.token.is_some() {
            self.api("me", &[])?
        } else {
            bail!(t("n.no_login"));
        };
        *self.me.lock().unwrap() = Some(m.clone());
        Ok(m)
    }

    pub fn permalink(&self) -> String {
        self.me()
            .ok()
            .and_then(|m| m["permalink"].as_str().map(String::from))
            .or_else(config::sc_user)
            .unwrap_or_else(|| "you".into())
    }

    pub fn listing(&self, kind: &str, arg: &str) -> Result<Vec<Row>> {
        let uid = || -> Result<String> { Ok(self.me()?["id"].as_u64().ok_or_else(|| anyhow!("no id"))?.to_string()) };
        let items = match kind {
            "search" => self.pages("search/tracks", 100, &[("q", arg), ("limit", "50")])?,
            "feed" => self.pages("stream", 300, &[("limit", "100")])?,
            "likes" => self.pages(&format!("users/{}/likes", uid()?), 600, &[])?,
            "tracks" => self.pages(&format!("users/{}/tracks", uid()?), 600, &[])?,
            "sets" => self.pages(&format!("users/{}/playlists", uid()?), 600, &[])?,
            _ => {
                let pl = self.api("resolve", &[("url", arg)])?;
                if pl["kind"] == "track" {
                    return Ok(vec![self.row(&pl)]);
                }
                if pl["kind"] != "playlist" {
                    bail!("{}{}", t("bad_link"), pl["kind"]);
                }
                let tracks = pl["tracks"].as_array().cloned().unwrap_or_default();
                let mut full: HashMap<u64, Value> = HashMap::new();
                let mut stubs = vec![];
                for tr in &tracks {
                    let id = tr["id"].as_u64().unwrap_or(0);
                    if tr["title"].is_string() { full.insert(id, tr.clone()); } else { stubs.push(id) }
                }
                // api отдаёт целиком только первые треки, остальное — id
                for chunk in stubs.chunks(50) {
                    let ids: Vec<String> = chunk.iter().map(|x| x.to_string()).collect();
                    if let Value::Array(a) = self.api("tracks", &[("ids", &ids.join(","))])? {
                        for tr in a {
                            full.insert(tr["id"].as_u64().unwrap_or(0), tr);
                        }
                    }
                }
                tracks.iter().filter_map(|tr| full.get(&tr["id"].as_u64().unwrap_or(0)).cloned()).collect()
            }
        };
        // в ленте один трек приходит и постом, и репостами
        let mut seen = std::collections::HashSet::new();
        Ok(items.iter().map(|o| self.row(o)).filter(|r| !r.url.is_empty() && seen.insert(r.url.clone())).collect())
    }

    pub fn track(&self, url: &str) -> Result<Value> {
        if let Some(v) = self.cache.lock().unwrap().get(url) {
            return Ok(v.clone());
        }
        let v = match track_id(url) {
            Some(id) => self.api(&format!("tracks/{id}"), &[])?,
            None => self.api("resolve", &[("url", url)])?,
        };
        self.cache.lock().unwrap().insert(url.to_string(), v.clone());
        Ok(v)
    }

    /// Фоллбэк, когда api-v2 капризничает: через yt-dlp, там хотя бы названия.
    pub fn listing_any(&self, kind: &str, arg: &str) -> Result<Vec<Row>> {
        match self.listing(kind, arg) {
            Ok(r) => Ok(r),
            // у ленты нет аналога в yt-dlp — фоллбэка нет
            Err(e) if kind == "feed" => Err(e),
            Err(e) => {
                let target = match kind {
                    "search" => format!("scsearch40:{arg}"),
                    "likes" | "tracks" | "sets" => format!("https://soundcloud.com/{}/{kind}", self.permalink()),
                    _ => arg.to_string(),
                };
                ytl(&target, kind != "search").map_err(|e2| anyhow!("{e}; yt-dlp: {e2}"))
            }
        }
    }
}

fn read_token() -> Option<String> {
    let prof = config::ff_profile()?;
    // копия: живая база залочена Firefox'ом
    let dir = config::run_dir().join(format!("sc-cookies-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    for f in ["cookies.sqlite", "cookies.sqlite-wal"] {
        let _ = std::fs::copy(prof.join(f), dir.join(f));
    }
    let tok = (|| {
        let con = rusqlite::Connection::open(dir.join("cookies.sqlite")).ok()?;
        con.query_row(
            "select value from moz_cookies where host like '%soundcloud.com' and name='oauth_token'",
            [],
            |r| r.get::<_, String>(0),
        )
        .ok()
    })();
    let _ = std::fs::remove_dir_all(&dir);
    tok
}

pub fn ytl(target: &str, cookies: bool) -> Result<Vec<Row>> {
    let mut cmd = Command::new("yt-dlp");
    cmd.args(["--ignore-config", "--flat-playlist", "-J", "--no-warnings", "-q"]);
    if let Some(i) = config::ytdl_impersonate() {
        cmd.args(["--impersonate", i]);
    }
    if cookies {
        if let Some(p) = config::ff_profile() {
            cmd.arg("--cookies-from-browser").arg(format!("firefox:{}", p.display()));
        }
    }
    let out = cmd.arg(target).stdin(Stdio::null()).stderr(Stdio::piped()).output()?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("yt-dlp failed"));
    }
    let info: Value = serde_json::from_slice(&out.stdout)?;
    let entries = match info["entries"].as_array() {
        Some(a) => a.clone(),
        None => vec![info],
    };
    Ok(entries
        .iter()
        .map(|e| {
            let url = e["webpage_url"].as_str().or(e["url"].as_str()).unwrap_or_default().to_string();
            let set = url.contains("/sets/") || e["_type"] == "playlist";
            Row {
                title: nfc(e["title"].as_str().unwrap_or(url.rsplit('/').next().unwrap_or_default())),
                artist: nfc(e["uploader"].as_str().or(e["artist"].as_str()).unwrap_or_default()),
                duration: e["duration"].as_f64(),
                kind: if set { Kind::Set } else { Kind::Track },
                count: None,
                url,
            }
        })
        .filter(|r| !r.url.is_empty())
        .collect())
}

/// Самый «сочный» цвет обложки — им красится весь интерфейс.
pub fn accent_from(img: &image::RgbImage) -> Rgb {
    let small = image::imageops::resize(img, 48, 48, image::imageops::FilterType::Triangle);
    let mut buckets: HashMap<i32, (f32, u64, u64, u64, u64)> = HashMap::new();
    for p in small.pixels() {
        let (h, s, v) = crate::util::rgb_to_hsv((p[0], p[1], p[2]));
        if s < 0.3 || v < 0.3 {
            continue;
        }
        let b = buckets.entry((h * 24.0) as i32).or_default();
        b.0 += s * v;
        b.1 += p[0] as u64;
        b.2 += p[1] as u64;
        b.3 += p[2] as u64;
        b.4 += 1;
    }
    let Some(best) = buckets.values().max_by(|a, b| a.0.total_cmp(&b.0)) else { return SC_ORANGE };
    let n = best.4.max(1);
    let (h, s, v) = crate::util::rgb_to_hsv(((best.1 / n) as u8, (best.2 / n) as u8, (best.3 / n) as u8));
    crate::util::hsv_to_rgb(h, s.max(0.55), v.max(0.92))
}

pub fn write_m3u(tracks: &[Row]) -> Result<PathBuf> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let path = config::run_dir().join(format!("sc-{}-{}.m3u", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let mut s = String::from("#EXTM3U\n");
    for tr in tracks {
        let name = if tr.artist.is_empty() { tr.title.clone() } else { format!("{} — {}", tr.artist, tr.title) };
        let dur = tr.duration.map(|d| d as i64).unwrap_or(-1);
        s.push_str(&format!("#EXTINF:{dur},{}\n{}\n", name.replace('\n', " "), tr.url));
    }
    std::fs::write(&path, s)?;
    Ok(path)
}
