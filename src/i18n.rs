//! Строки интерфейса ru/en.
use std::sync::atomic::{AtomicBool, Ordering};

pub const LANGS: [(&str, &str); 2] = [("ru", "Русский"), ("en", "English")];

static EN: AtomicBool = AtomicBool::new(false);

pub fn default_lang() -> &'static str {
    let env = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .unwrap_or_default();
    if env.starts_with("ru") { "ru" } else { "en" }
}

pub fn set_lang(lang: Option<&str>) {
    let l = match lang {
        Some(l @ ("ru" | "en")) => l,
        _ => default_lang(),
    };
    EN.store(l == "en", Ordering::Relaxed);
}

pub fn lang() -> &'static str {
    if EN.load(Ordering::Relaxed) { "en" } else { "ru" }
}

fn en() -> bool {
    EN.load(Ordering::Relaxed)
}

pub fn t(key: &str) -> &'static str {
    let (ru, en_) = match key {
        "usage" => (
            "sc — SoundCloud в терминале. mpv играет в фоне, это — пульт к нему.

  sc                       открыть плеер
  sc <запрос>              открыть и сразу искать
  sc feed|likes|tracks|sets  открыть на вкладке
  sc <url>                 трек / плейлист / профиль → в очередь и играть
  sc next|prev|pause|stop  управление без UI (для хоткеев)

Внутри: ? — все клавиши, c — настройки. q — закрыть UI (музыка играет), Q — стоп.",
            "sc — SoundCloud in your terminal. mpv plays in the background, this is the remote.

  sc                       open the player
  sc <query>               open and search right away
  sc feed|likes|tracks|sets  open on a tab
  sc <url>                 track / playlist / profile → queue and play
  sc next|prev|pause|stop  control without UI (for global hotkeys)

Inside: ? — all keys, c — settings. q — close UI (music keeps playing), Q — stop.",
        ),
        "not_running" => ("sc: плеер не запущен", "sc: player is not running"),
        "tab.queue" => ("Очередь", "Queue"),
        "tab.search" => ("Поиск", "Search"),
        "tab.feed" => ("Лента", "Feed"),
        "tab.likes" => ("Лайки", "Likes"),
        "tab.tracks" => ("Мои треки", "My tracks"),
        "tab.sets" => ("Плейлисты", "Playlists"),
        "search.ph" => ("что ищем? Enter — искать, Esc — к списку", "search… Enter — go, Esc — back to list"),
        "col.title" => ("название", "title"),
        "col.artist" => ("автор", "artist"),
        "col.time" => ("время", "time"),
        "back" => ("← Backspace", "← Backspace"),
        "loading" => ("загружаю", "loading"),
        "empty_search" => ("введи запрос и жми Enter", "type a query and hit Enter"),
        "empty_queue" => (
            "очередь пуста — выбери что-нибудь в лайках или поиске",
            "queue is empty — pick something from likes or search",
        ),
        "idle.title" => ("тишина", "silence"),
        "idle.sub" => ("выбери трек и жми Enter", "pick a track and hit Enter"),
        "next" => ("далее", "next"),
        "mode.track" => ("🔂 трек", "🔂 track"),
        "mode.queue" => ("🔁 очередь", "🔁 queue"),
        "mode.off" => ("→ без повтора", "→ no repeat"),
        "vol" => ("громк", "vol"),
        "hint" => (
            "space пауза · n/p трек · ←/→ перемотка · +/- громк · r повтор · / поиск · c настройки · ? клавиши · q выйти",
            "space pause · n/p track · ←/→ seek · +/- vol · r repeat · / search · c settings · ? keys · q quit",
        ),
        "hint.short" => ("c настройки · ? клавиши · q выйти", "c settings · ? keys · q quit"),
        "n.load_fail" => ("не загрузилось: ", "failed to load: "),
        "n.empty" => ("пусто", "nothing here"),
        "n.mpv_fail" => ("mpv не запустился", "mpv failed to start"),
        "n.append" => ("в конец очереди: ", "added to queue: "),
        "n.insert" => ("следующим: ", "playing next: "),
        "n.shuffled" => ("очередь перемешана", "queue shuffled"),
        "n.bad_hex" => ("не похоже на цвет: ", "not a colour: "),
        "n.no_login" => (
            "нет логина SoundCloud: зайди в Firefox или задай SC_USER",
            "no SoundCloud login: sign in via Firefox or set SC_USER",
        ),
        "bad_link" => ("непонятная ссылка: ", "unsupported link: "),
        "help.title" => ("клавиши", "keys"),
        "k.enter" => (
            "играть список с этого трека / прыгнуть в очереди / открыть плейлист",
            "play list from this track / jump in queue / open playlist",
        ),
        "k.ae" => ("в конец очереди / играть следующим", "add to queue / play next"),
        "k.space" => ("пауза", "pause"),
        "k.np" => ("следующий / предыдущий", "next / previous"),
        "k.seek" => (
            "перемотка ±5 с (shift — ±30 с), клик по волне — туда",
            "seek ±5 s (shift — ±30 s), click the waveform to jump",
        ),
        "k.vol" => ("громкость", "volume"),
        "k.repeat" => ("повтор: очередь → трек → выкл", "repeat: queue → track → off"),
        "k.shuffle" => ("перемешать очередь", "shuffle queue"),
        "k.remove" => ("убрать из очереди", "remove from queue"),
        "k.search" => ("поиск", "search"),
        "k.tabs" => ("вкладки", "tabs"),
        "k.back" => ("назад из плейлиста", "back from playlist"),
        "k.home" => ("в начало / конец списка", "top / bottom of list"),
        "k.reload" => ("обновить список", "reload list"),
        "k.vis" => ("визуализатор вкл/выкл", "visualizer on/off"),
        "k.settings" => ("настройки: язык, цвет, спектр", "settings: language, colour, spectrum"),
        "k.mouse" => ("колесо — листать, клик — выбрать, двойной — играть", "wheel — scroll, click — select, double — play"),
        "k.q" => ("закрыть UI — музыка играет", "close UI — music keeps playing"),
        "k.Q" => ("стоп и выйти", "stop and quit"),
        "set.title" => ("настройки", "settings"),
        "set.lang" => ("язык", "language"),
        "set.accent" => ("цвет", "colour"),
        "set.vis" => ("спектр", "spectrum"),
        "set.style" => ("вид спектра", "spectrum style"),
        "set.fps" => ("кадров/с", "fps"),
        "set.fps_note" => ("больше — плавнее, но жрёт CPU", "higher is smoother but costs CPU"),
        "small" => ("окно маловато — растяни терминал", "window is too small — make the terminal bigger"),
        "set.auto" => ("из обложки", "from cover"),
        "set.custom" => ("свой", "custom"),
        "set.on" => ("вкл", "on"),
        "set.off" => ("выкл", "off"),
        "style.bars" => ("столбики", "bars"),
        "style.thin" => ("тонкие", "thin"),
        "style.mirror" => ("зеркало", "mirror"),
        "set.hint" => (
            "↑↓ выбрать · ←→ менять · Enter — свой цвет · Esc закрыть",
            "↑↓ select · ←→ change · Enter — custom colour · Esc close",
        ),
        "set.hex_ph" => ("#rrggbb и Enter", "#rrggbb and Enter"),
        _ => ("?", "?"),
    };
    if en() { en_ } else { ru }
}

fn plural(n: usize, one: &'static str, few: &'static str, many: &'static str) -> &'static str {
    if n % 10 == 1 && n % 100 != 11 {
        one
    } else if (2..=4).contains(&(n % 10)) && !(12..=14).contains(&(n % 100)) {
        few
    } else {
        many
    }
}

pub fn count_tracks(n: usize) -> String {
    if en() {
        format!("{n} track{}", if n == 1 { "" } else { "s" })
    } else {
        format!("{n} {}", plural(n, "трек", "трека", "треков"))
    }
}

pub fn count_sets(n: usize) -> String {
    if en() {
        format!("{n} playlist{}", if n == 1 { "" } else { "s" })
    } else {
        format!("{n} {}", plural(n, "плейлист", "плейлиста", "плейлистов"))
    }
}

pub fn queued(n: usize) -> String {
    if en() { format!("  · {n} queued") } else { format!("  · в очереди {n}") }
}

pub fn total_time(sec: f64) -> String {
    let s = sec.max(0.0) as u64;
    let (h, m) = (s / 3600, s % 3600 / 60);
    match (en(), h > 0) {
        (false, true) => format!("{h} ч {m} мин"),
        (false, false) => format!("{m} мин"),
        (true, true) => format!("{h} h {m} min"),
        (true, false) => format!("{m} min"),
    }
}
