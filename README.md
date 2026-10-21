# sc-cli

SoundCloud in your terminal. A TUI remote for `mpv` that plays in the background — close the UI, the music keeps going.

https://github.com/user-attachments/assets/bb6d9c20-d905-4b9a-8177-ea98be0aae1a

- feed, likes, your tracks, playlists, search — via api-v2 under your own account (cookie from Firefox)
- truecolor cover art, accent colour picked from the cover
- real SoundCloud waveform with mouse seeking, live spectrum
- queue: play from here, append, play next, shuffle, repeat
- MPRIS: headphone buttons, media keys, desktop player widgets
- English / Russian

## Install

### Linux

Runtime deps: `mpv`, `yt-dlp`, `parec`/`pactl` (PipeWire or PulseAudio), Firefox logged in to soundcloud.com.

```sh
# Arch
sudo pacman -S mpv yt-dlp libpulse
# Debian / Ubuntu
sudo apt install mpv yt-dlp pulseaudio-utils
```

Recommended: `curl_cffi` for yt-dlp (`pip install "yt-dlp[default,curl-cffi]"`). SoundCloud's CDN sometimes answers plain yt-dlp with 403; with it, sc makes yt-dlp impersonate Chrome and tracks play fine.

Build (Rust 1.85+):

```sh
git clone https://github.com/la4zen/sc-cli && cd sc-cli
cargo build --release
ln -s "$PWD/target/release/sc" ~/.local/bin/sc
```

### Windows

Not supported yet: sc talks to mpv over a Unix socket, reads audio via PulseAudio and speaks D-Bus. A native port (named pipes, WASAPI, SMTC) is on the list.

## Usage

```sh
sc                            # player
sc <query>                    # search right away
sc feed|likes|tracks|sets     # open on a tab
sc <url>                      # track / playlist / profile → play
sc next|prev|pause|stop       # no UI, for hotkeys
```

Inside: `?` — all keys, `c` — settings, `q` — close UI (music keeps playing), `Q` — stop.

| env | |
|---|---|
| `SC_USER` | whose profile to open (default: cookie owner) |
| `SC_FIREFOX_PROFILE` | Firefox profile path |
| `SC_SOCK` | mpv IPC socket (`$XDG_RUNTIME_DIR/sc-mpv.sock`) |
| `SC_STATE` | settings file (`~/.config/sc/state.json`) |

## License

MIT — see [LICENSE](LICENSE).

---

*Asked nicely, so in the end I did it — even though nobody asked me to.* — Claude Opus 5.5
