mod app;
mod config;
mod i18n;
mod mpris;
mod mpv;
mod soundcloud;
mod spectrum;
mod ui;
mod util;

use app::{App, Ev};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use serde_json::json;
use std::sync::mpsc;
use std::time::Instant;

fn fast(cmd: &str) -> Option<serde_json::Value> {
    Some(match cmd {
        "next" => json!(["playlist-next", "force"]),
        "prev" => json!(["playlist-prev", "force"]),
        "pause" => json!(["cycle", "pause"]),
        "stop" => json!(["quit"]),
        _ => return None,
    })
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let first = args.first().map(String::as_str);
    if first == Some("mpris-daemon") {
        return mpris::main();
    }
    if let Some(a) = first.filter(|a| matches!(*a, "-h" | "--help") || fast(a).is_some()) {
        i18n::set_lang(config::State::load().lang.as_deref());
        match fast(a) {
            Some(cmd) => {
                if config::ctl(cmd).is_none() {
                    eprintln!("{}", i18n::t("not_running"));
                    std::process::exit(1);
                }
            }
            None => println!("{}", i18n::t("usage")),
        }
        return Ok(());
    }
    let initial = match first {
        None => (None, None),
        Some(t @ ("feed" | "likes" | "tracks" | "sets")) => (Some("tab"), Some(t.to_string())),
        Some(u) if is_sc_url(u) => (Some("url"), Some(u.to_string())),
        Some(_) => (Some("query"), Some(args.join(" "))),
    };
    run(initial)
}

fn is_sc_url(u: &str) -> bool {
    let rest = u.strip_prefix("https://").or_else(|| u.strip_prefix("http://")).unwrap_or("");
    let host = rest.split('/').next().unwrap_or("");
    rest.contains('/') && (host == "soundcloud.com" || host.ends_with(".soundcloud.com"))
}

fn run(initial: (Option<&str>, Option<String>)) -> anyhow::Result<()> {
    let (tx, rx) = mpsc::channel::<Ev>();
    let mut app = App::new(tx.clone());
    let mut term = ratatui::init();
    execute!(std::io::stdout(), EnableMouseCapture)?;
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            while let Ok(ev) = crossterm::event::read() {
                if tx.send(Ev::Term(ev)).is_err() {
                    break;
                }
            }
        });
    }
    app.start(initial);
    let res = (|| -> anyhow::Result<()> {
        loop {
            let wait = app.deadline().saturating_duration_since(Instant::now());
            match rx.recv_timeout(wait) {
                Ok(ev) => app.handle(ev),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            // пачкой: колесо мыши и события mpv прилетают очередями
            while let Ok(ev) = rx.try_recv() {
                app.handle(ev);
            }
            if app.quit {
                break;
            }
            app.tick();
            if app.dirty {
                app.dirty = false;
                term.draw(|f| ui::draw(f, &mut app))?;
                if app.relayout() {
                    term.draw(|f| ui::draw(f, &mut app))?;
                }
            }
        }
        Ok(())
    })();
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    app.shutdown();
    res
}
