//! Спектр: parec слушает только поток mpv, FFT в отдельном потоке.
use rustfft::{num_complex::Complex, FftPlanner};
use serde_json::Value;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const RATE: usize = 22050;
const N: usize = 1024;

#[derive(Default)]
pub struct Shared {
    pub raw: Mutex<Vec<f32>>,
    pub bands: AtomicUsize,
    /// новых кадров FFT в секунду — под fps интерфейса
    pub rate: AtomicU32,
    pub active: AtomicBool,
    pub stopped: AtomicBool,
    pub pid: Mutex<Option<u64>>,
    pub client: Mutex<String>,
    child: AtomicI32,
}

impl Shared {
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        self.kill();
    }
    fn kill(&self) {
        let pid = self.child.swap(0, Ordering::Relaxed);
        if pid > 0 {
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }
    pub fn set_active(&self, on: bool) {
        if self.active.swap(on, Ordering::Relaxed) == on {
            return;
        }
        if !on {
            self.kill();
        }
    }
}

pub fn start(bands: usize, fps: u32) -> Arc<Shared> {
    let sh = Arc::new(Shared::default());
    sh.bands.store(bands, Ordering::Relaxed);
    sh.rate.store(fps.clamp(10, 60), Ordering::Relaxed);
    *sh.raw.lock().unwrap() = vec![0.0; bands];
    *sh.client.lock().unwrap() = "sc-mpv".into();
    let s2 = sh.clone();
    std::thread::spawn(move || run(s2));
    sh
}

fn source(sh: &Shared) -> Option<u64> {
    let out = Command::new("pactl").args(["-f", "json", "list", "sink-inputs"]).stderr(Stdio::null()).output().ok()?;
    let v: Value = serde_json::from_slice(&out.stdout).ok()?;
    let client = sh.client.lock().unwrap().clone();
    let pid = *sh.pid.lock().unwrap();
    let list = v.as_array()?;
    // сначала по pid — имя у двух mpv может совпасть; у pipewire-вывода pid нет, тогда по audio-client-name
    let by_pid: Vec<&Value> = match pid {
        Some(pid) => list.iter().filter(|si| si["properties"]["application.process.id"].as_str() == Some(&pid.to_string())).collect(),
        None => vec![],
    };
    let mut mine = if by_pid.is_empty() {
        list.iter().filter(|si| si["properties"]["application.name"].as_str() == Some(&client)).collect()
    } else {
        by_pid
    };
    mine.sort_by_key(|si| si["corked"].as_bool().unwrap_or(false));
    mine.first()?["index"].as_u64()
}

fn bins(bands: usize) -> (Vec<usize>, Vec<f32>) {
    let nb = N / 2 + 1;
    let freq = |i: usize| i as f32 * RATE as f32 / N as f32;
    let (lo, hi) = (35f32, 10500f32);
    let mut edges: Vec<usize> = (0..=bands)
        .map(|i| {
            let f = lo * (hi / lo).powf(i as f32 / bands as f32);
            (0..nb).find(|&k| freq(k) >= f).unwrap_or(nb)
        })
        .collect();
    for i in 1..edges.len() {
        edges[i] = edges[i].max(edges[i - 1] + 1);
    }
    let tilt = edges[..bands]
        .iter()
        .map(|&e| (4.0 * ((freq(e.min(nb - 1)) + 1.0) / 250.0).log2()).clamp(0.0, 18.0)) // верха тише — подтягиваем
        .collect();
    (edges, tilt)
}

fn run(sh: Arc<Shared>) {
    let win: Vec<f32> = (0..N).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (N - 1) as f32).cos()).collect();
    let fft = FftPlanner::<f32>::new().plan_fft_forward(N);
    let mut buf = vec![0f32; N];
    let mut spec = vec![Complex::new(0f32, 0f32); N];
    let mut reference = -20f32;
    let mut bands = sh.bands.load(Ordering::Relaxed);
    let (mut edges, mut tilt) = bins(bands);
    while !sh.stopped.load(Ordering::Relaxed) {
        if !sh.active.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(150));
            continue;
        }
        let Some(src) = source(&sh) else {
            // потока ещё нет (буферизуется) — весь системный звук не слушаем, ждём
            std::thread::sleep(Duration::from_secs(1));
            continue;
        };
        let child = Command::new("parec")
            .args(["--raw", "--format=s16le", "--channels=1", "--latency-msec=20"])
            .arg(format!("--rate={RATE}"))
            .arg(format!("--monitor-stream={src}"))
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = child else {
            std::thread::sleep(Duration::from_secs(2));
            continue;
        };
        sh.child.store(child.id() as i32, Ordering::Relaxed);
        let mut out = child.stdout.take().unwrap();
        let mut got = false;
        let mut bytes = vec![0u8; 0];
        while sh.active.load(Ordering::Relaxed) && !sh.stopped.load(Ordering::Relaxed) {
            let hop = RATE / sh.rate.load(Ordering::Relaxed).max(1) as usize;
            bytes.resize(hop * 2, 0);
            if out.read_exact(&mut bytes).is_err() {
                break;
            }
            got = true;
            let nb = sh.bands.load(Ordering::Relaxed);
            if nb != bands {
                bands = nb;
                (edges, tilt) = bins(bands);
            }
            let x: Vec<f32> = bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect();
            let k = x.len().min(N);
            buf.copy_within(k.., 0);
            buf[N - k..].copy_from_slice(&x[x.len() - k..]);
            for i in 0..N {
                spec[i] = Complex::new(buf[i] * win[i], 0.0);
            }
            fft.process(&mut spec);
            let nb2 = N / 2 + 1;
            let db: Vec<f32> = (0..bands)
                .map(|b| {
                    let (a, z) = (edges[b].min(nb2), edges[b + 1].min(nb2));
                    let m = spec[a..z.max(a)].iter().map(|c| c.norm()).fold(0f32, f32::max);
                    20.0 * (m + 1e-6).log10() + tilt[b]
                })
                .collect();
            let mx = db.iter().cloned().fold(f32::MIN, f32::max);
            reference = (reference * 0.995 + mx * 0.005).max(mx).max(-10.0);
            let raw = db.iter().map(|d| ((d - (reference - 42.0)) / 42.0).clamp(0.0, 1.0).powf(1.4)).collect();
            *sh.raw.lock().unwrap() = raw;
        }
        sh.child.store(0, Ordering::Relaxed);
        let _ = child.kill();
        let _ = child.wait();
        *sh.raw.lock().unwrap() = vec![0.0; bands];
        if !got {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}
