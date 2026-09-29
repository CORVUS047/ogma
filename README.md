# ogma
A lightweight music player, simple as that!

ogma is a terminal music player in three pieces:

| Binary | What it is |
| --- | --- |
| `ogma` | the TUI: browse, queue, playlists, search YouTube, config |
| `ogma-daemon` | playback. Holds the queue and drives the audio device. Keeps playing with no interface attached, and never stops while one is |
| `ogma-cmd` | one-shot commands to a running daemon, for keybindings and scripts |

The interface only drives the daemon over a Unix socket, so closing the TUI does not have to stop the music.

## INSTALLATION

### Requirements

- Rust 1.85 or newer (the crate is edition 2024)
- A Unix-like system. The daemon talks over a Unix socket, so Windows is not supported
- Linux: ALSA development headers and `pkg-config`, for `cpal`

```sh
# Debian/Ubuntu
sudo apt install build-essential pkg-config libasound2-dev

# Fedora
sudo dnf install gcc pkgconf-pkg-config alsa-lib-devel

# Arch
sudo pacman -S base-devel pkgconf alsa-lib

# Gentoo
sudo emerge --ask media-libs/alsa-lib dev-util/pkgconf
```

macOS needs no extra packages; `cpal` uses CoreAudio.

Optional, for the YouTube screen: [`yt-dlp`](https://github.com/yt-dlp/yt-dlp) on `PATH`, and
`ffmpeg` for downloads. Without them the rest of the player works as it always did and the screen
says what is missing.

```sh
sudo apt install yt-dlp ffmpeg      # Debian/Ubuntu
sudo dnf install yt-dlp ffmpeg      # Fedora
sudo pacman -S yt-dlp ffmpeg        # Arch
sudo emerge --ask net-misc/yt-dlp media-video/ffmpeg   # Gentoo
```

`$OGMA_YTDLP` names the program to run, for a yt-dlp kept somewhere unusual.

### Build from source

```sh
git clone https://github.com/CORVUS047/ogma
cd ogma
cargo build --release
```

Three binaries land in `target/release/`: `ogma`, `ogma-daemon`, `ogma-cmd`.

### Install

```sh
cargo install --path .
```

That puts all three in `~/.cargo/bin`, which needs to be on `PATH`.

`ogma` starts the daemon itself when none is running, looking for `ogma-daemon` beside its own
executable, then one directory up, then on `PATH`. Keep the three together — installing them into the
same directory is enough.

Run it straight out of the build tree without installing:

```sh
./target/release/ogma
```

## USAGE

### First run

```sh
ogma
```

The start menu opens. **Select Folder** picks the folder the library is scanned from, and remembers
it in the config, so later runs go straight to it. The scan walks 8 levels deep, skips hidden files,
and filters by extension: `aac aif aifc aiff alac ape caf flac m4a m4b mka mp1 mp2 mp3 mp4 mpc oga
ogg opus spx wav wave wv`.

Nothing is queued on open. The queue is yours to fill from the file listing.

### TUI
#### Main menu:

<img width="1856" height="1044" alt="image" src="https://github.com/user-attachments/assets/8a780f49-ad3a-4aaf-a1bb-cdd7f52d560a" />

| Key | Does |
| --- | --- |
| `j` / `k`, up / down | Move |
| `g` / `G`, home / end | First / last entry |
| enter, space | Choose |
| `q`, esc | Quit |

**Continue** only appears once there is a player screen to go back to.

#### Folder browser

Where **Select Folder** goes. It chooses the folder it is *listing*, not the one highlighted, so walk
into a folder before selecting it — the title says which one `s` would take.

| Key | Does |
| --- | --- |
| enter, right, `l` | Into the highlighted folder |
| left, `h`, backspace | Up a folder |
| `s`, space | Use the listed folder as the library |
| `~` | Jump to your home folder |
| `.` | Show or hide dotfolders |
| `/` | Find in the listing; in youtube mode, start a query. Esc clears it |
| `j` `k` `g` `G`, page up / down | Move |
| `q`, esc | Back to the menu, changing nothing |

The choice is written to the config as soon as it is made, so there is nothing to save.

#### Player:

<img width="1856" height="1044" alt="image" src="https://github.com/user-attachments/assets/5d1bfd9a-3bd3-497f-8ec2-59069393a056" />

Three panes — playlists, files, queue — with cover art and the progress of what is playing. `tab`
moves the focus; the keys below work wherever the focus is.

Under the progress bar is what the player is doing and where the fader sits — `▶ playing  vol
█████░░░░░  50%  -10.0 dB`. It stays on screen with nothing playing, since that is the level the next
song comes out at. A narrow pane gives up the bar's cells and then the percentage to keep
the decibel reading, which is the part that cannot be guessed, and only falls back to the percentage
alone when even a four-cell bar and the reading will not fit. Nothing is ever clipped.

Playback:

| Key | Does |
| --- | --- |
| space | Play / pause |
| `n` / `p` | Next / previous track |
| left / right | Seek 5 seconds back / forward |
| `+` `=` / `-` `_` | Volume, 5 points a press |
| `S` | Stop and rewind, keeping the queue |
| `z` | Shuffle the queue |
| `R` | Cycle repeat: off → queue → song |
| `tab` / `shift-tab`, `l` / `h` | Next / previous pane |
| `q`, esc | Back to the menu (playback carries on) |

Multimodal panel. One pane with four things it can list, `m` cycling between the first three. The
border is titled for whichever is showing — Files, Browse, YouTube, Playlist — and the line under it
says what within that mode: the folder, the grouping, the query, the playlist.

| Mode | Lists |
| --- | --- |
| **files** | The folder on disk, where the panel opens |
| **browse** | Everything below that folder grouped under one tag — artist, album, genre or year |
| **youtube** | What a search turned up, to stream or to download |
| **playlist** | A playlist's own order, opened with `o` from the playlists pane |

The rows are the same kind of thing in every mode, which is the point of the panel: `a` queues, `A`
queues the lot, `P` sends to the selected playlist, and enter plays — whether the row came off the
disk or out of a search.

| Key | Does |
| --- | --- |
| `m` | Next mode: files → browse → youtube |
| enter | Open a folder or a group; play a track or stream a result |
| backspace | Out a step: up a folder, out of a group, back to the files |
| `a` | Queue the highlighted row — a track, a whole folder, or a whole group |
| `A` | Queue everything listed |
| `P` | Add the highlighted row to the selected playlist |
| `x` | Pick the highlighted file or folder up to move it; again puts it back down |
| `M` | Move everything held into the folder being listed |
| `/` | Find in the listing; esc clears it |
| `j` `k` `g` `G`, page up / down | Move |

In browse mode, `t` changes what the tracks are filed under. The groups are listed by name with the
count beside them, and the tracks that say nothing land in an `Unknown Artist` group at the bottom
rather than being left out. Reading a folder's tags is the slow part, so it happens on its own
thread — the pane says `reading tags…` meanwhile — and the result is kept until you walk somewhere
else.

In youtube mode, `/` starts a query and enter runs it; `d` downloads the highlighted result instead
of streaming it. Arriving in the mode does not put the keyboard in a text field — the keys stay the
panel's until `/` asks for one. See [YouTube](#youtube) below.

Moving files: `x` marks a row — the margin shows `✂` — then walk wherever you like and press `M` to
move everything held into the folder on screen. Folders move whole, several things can be held at
once, and holding survives walking about: that is the point of it.

Nothing is ever overwritten. A name already taken in the target folder, a folder being put inside
itself, or a file that has since gone is refused and stays held, so a second `M` somewhere else is
one key away. A move between filesystems copies and then removes, so an interrupted one leaves the
file where it was.

A queued file keeps the path it was queued under, so moving it leaves the queue pointing where it
used to be. Move first, queue after.

Playlists pane. A playlist holds tracks from YouTube as happily as files: the URL goes in the list
with the title, artist and length beside it, so it comes back named rather than as a link. The
`tracks` list itself stays a plain list of paths, so a playlist written by an older version still
loads and one written now still opens in anything that only knows about files.

| Key | Does |
| --- | --- |
| enter | Load the playlist into the queue, replacing it |
| `o` | Open it in the multimodal panel to look through |
| `c` | New playlist |
| `s` / `r` | Cycle the sort / reverse it |
| `d` | Delete (asks first) |
| `/` | Find |

Queue pane:

| Key | Does |
| --- | --- |
| `j` / `k` | Move |
| enter | Play this entry now |
| `x`, delete | Take it out of the queue |

The three played songs above the current one stay visible, so the current song holds its row. The
pane's title says what repeats when anything does — `Queue · repeat song`.

#### Repeating

`R` cycles three modes, and the mode lives in the daemon, so it holds whether or not an interface is
attached:

| Mode | What happens when a song runs out |
| --- | --- |
| off | The next queued song plays; the end of the queue is silence |
| queue | At the end of the queue, what played is queued again in the order it played, and the first of it starts |
| song | The same song plays again from its beginning |

Only a song **ending by itself** is affected. `n` and `p` always move on and back — that is what they
are for. The one exception is the end of the queue with queue repeat on, where `n` wraps round to the
start rather than stopping, since there is always a next song.

#### YouTube

The **youtube** mode of the multimodal panel, which `m` reaches. Needs `yt-dlp`; without it the mode says
so and the rest of the player is unaffected.

`/` starts a query, enter runs it, and what comes back lists like any other track. Until then the
keys are the panel's own, so entering the mode by accident costs one `m` to leave rather than a
screenful of typing. A result can be
**streamed** — played straight from the network, with nothing written to disk — or **downloaded**
with `d` into the download folder, where it is an ordinary file the library scan picks up.

Because the results are ordinary rows, the keys are the pane's own: enter streams the highlighted
result now, `a` queues it, `A` queues the page, and `P` sends it to the selected playlist.

Streamed tracks sit in the queue beside local files and play the same way, with two differences:
they cannot be seeked (the audio arrives as it plays), and they are named by what the search said
rather than by tags, since there is no file to read any. That name follows them into the queue, into
a playlist and back out again.

Searching and downloading both run on their own threads, so the pane stays usable while they work,
and the line under the listing says how far a download has got.

Downloads are named `Artist - Title`, with tags and cover art embedded where the format and the
installed helpers allow; a cover that cannot be embedded costs the track its picture, not the
download.

What this fetches is between you and the rights holder — yt-dlp is the same tool either way.

#### Configuration:

<img width="1856" height="1044" alt="image" src="https://github.com/user-attachments/assets/2f0609b5-15ff-48b0-8524-90d59e39ac8c" />

| Setting | What it does |
| --- | --- |
| Master volume | Fader applied to everything, shown as a percentage and in dB. Left / right adjust, `0`-`9` set the level |
| Default folder | The folder the library is built from. `enter` types a path, `d` clears it |
| Fill metadata | Writes missing artwork into your music files. **Off by default — it modifies your files** |
| Look online | Lets the filling ask MusicBrainz and iTunes instead of only reading the disk. Off by default, and separate from the above on purpose. Needs Fill metadata on |
| Control hints | Whether the keys are listed on each screen. On by default, and applies as soon as the config is left — the player waiting behind Continue included |
| Hide messages | Silences what an action reports. The failures share that line, so turning it on hides those too. Applies straight away, like the hints |
| On close | What becomes of the daemon when the **last** interface closes: keep it if it was started by hand (default), always kill it, or always keep it. Closing one of several interfaces never stops playback — see [Several interfaces at once](#several-interfaces-at-once) |
| Downloads | Where YouTube downloads are written. `enter` types a path, `d` goes back to the default: `Downloads` beside the library, or the platform's music folder under `ogma` when no library is set |
| Download as | What a downloaded track is kept as: as served (default, no re-encoding), opus, mp3, m4a or flac. Converting needs `ffmpeg` |
| Search results | How many hits the panel's YouTube mode asks for. Left / right adjust, 5 at a time |
| Theme | Colours from `theme.toml` instead of the terminal's palette. Restart to apply |

`s` saves. `q` or esc leaves; unsaved edits are still live for the session, but only the file survives
a restart.

### Command line

`ogma-cmd` sends one command to the running daemon and prints what it said. A command the daemon
could not carry out exits non-zero, so it is safe to drive from a window-manager keybinding or a
script.

```sh
$ ogma-cmd play_pause
ok: playing Xtal
$ ogma-cmd volume -10
ok: volume 45% (-14.0 dB)
$ ogma-cmd load_playlist Late Night
ok: queued 24 from Late Night by title
```

| Command | Does |
| --- | --- |
| `play` | Start or resume playback |
| `pause` | Hold playback where it is |
| `play_pause` | Pause if playing, play if not |
| `next` | On to the next song in the queue |
| `previous` | Back to the song that played before |
| `shuffle` | Rearrange the queue |
| `repeat [off\|queue\|song]` | Cycle what repeats, or set it. `all` and `one` are accepted for queue and song |
| `load_playlist <name>` | Replace the queue with a playlist |
| `add_playlist <name>` | Add a playlist to the end of the queue |
| `play_song <name>` | Find a song in the library and play it |
| `play_stream <url>` | Play a track from the network now, e.g. a YouTube URL |
| `queue_stream <url>` | Add a track from the network to the queue |
| `volume <±points>` | Move the volume, e.g. `10` or `-5` |
| `seek <±seconds>` | Move the position, e.g. `30` or `-10` |
| `stop` | Halt playback and rewind, keeping the queue |
| `clear` | Stop and forget the queue and history |
| `status` | Report what is playing, as JSON, including what repeats and what any streams are called |
| `interfaces` | How many interfaces are attached |
| `version`, `protocol` | Which version of the socket protocol the daemon speaks |
| `quit`, `close` | Kill the daemon, whoever is attached |

Names need no quoting — arguments are joined, so `ogma-cmd load_playlist Late Night` works. Hyphens
and underscores are both accepted (`play-pause`).

Example keybinding, for anything that runs a command on a key:

```sh
ogma-cmd play_pause
ogma-cmd volume +5
```

### Several interfaces at once

Every TUI attaches to the daemon when it starts and tells it when it leaves, so the daemon knows how
many interfaces are driving it. **It never stops while one of them is still open**, whatever
`daemon_on_close` says: closing one of two open TUIs leaves the music playing, and the setting is
about the last one to close.

```sh
$ ogma-cmd interfaces
ok: 2 interfaces
```

Which interface started the daemon does not matter, only that one did: the daemon remembers that, so
with the default `stop_if_we_started_it` the last TUI to close is the one that stops it, even if it
was not the one that started it. A daemon started by hand is never stopped by an interface closing.

The attachment is a connection the interface holds open for as long as it runs, so an interface that
is killed or crashes stops being counted at once — and asks for nothing, since something that died did
not ask for the music to stop. Two commands carry this, and are meant for an interface rather than a
person:

| Command | Does |
| --- | --- |
| `attach <id> [spawned]` | Count this interface, on a connection whose closing means it has gone |
| `leave <id> [keep\|stop\|stop_if_spawned]` | It has gone; act on the wish only if it was the last |

`ogma-cmd attach x` therefore attaches and detaches again immediately — its connection closes when it
exits. `ogma-cmd quit` is the way to stop a daemon by hand; it does not ask who is attached.

### Running the daemon on its own

```sh
$ ogma-daemon &
ogma-daemon: listening on /run/user/1000/ogma.sock
$ ogma-cmd play_pause
ok: playing Xtal
```

Only one daemon can hold the socket; a second exits with a message rather than taking over. A socket
left behind by a crash is detected and replaced at start-up.

The protocol is one line of text in, one line out, so it can be driven by hand:

```sh
printf 'volume -10\n' | nc -U "${XDG_RUNTIME_DIR}/ogma.sock"
```

### Protocol versions

The interface and the daemon are separate programs talking over a socket, so they can end up being
different builds — a daemon left running from before an upgrade, or two copies installed in
different places. Left alone they would misread each other quietly, which is worse than not
starting.

So `ogma` asks first. On start-up it sends `version`; if the answer is not the version it speaks,
it stops that daemon and starts one from its own installation, then asks again. That fixes the
common case — an old daemon still running — without saying anything.

If the second answer still does not match, the interface stops and shows what it found:

```
╭──────────── version mismatch ────────────╮
│ ogma and ogma-daemon are different       │
│ builds.                                  │
│                                          │
│ ogma speaks        IPC version 2         │
│ ogma-daemon speaks IPC version 1         │
│ …                                        │
╰─ any key closes ogma and stops the daemon ─╯
```

Any key closes the player and stops the daemon with it, so the next run starts clean rather than
finding the same wrong version again. A daemon too old to know the `version` command is reported as
"an older version, which cannot say which" — not knowing how to answer is itself an answer.

`ogma-cmd version` asks the same question by hand:

```sh
$ ogma-cmd version
ok: ipc 1
```

The number goes up whenever a command or a reply changes shape in a way the other side could
misread. Installing the three binaries together — `cargo install --path .` — keeps them in step.

### Files and environment

| Path | What |
| --- | --- |
| `~/.config/ogma/config.toml` | Settings. Written with the defaults on first run |
| `~/.config/ogma/theme.toml` | Colours, when Theme is on. Written when you first turn it on |
| `~/.config/ogma/playlists/*.toml` | One file per playlist |
| `$XDG_RUNTIME_DIR/ogma.sock` | The daemon's socket |

macOS keeps the config under `~/Library/Application Support/ogma/` instead. With no
`XDG_RUNTIME_DIR`, the socket falls back to a per-user name in the temp directory.

| Variable | What |
| --- | --- |
| `OGMA_SOCKET` | Move the socket. Set it for both the daemon and `ogma-cmd` to run a second player |

The config file is hand-editable. A missing file is written out with the defaults; a broken one is
left alone and the defaults are used for that run, so a typo cannot lose your settings.

```toml
master_volume = 0.5
default_folder = "/home/you/Music"
auto_fill_metadata = false
fetch_artwork_online = false
show_control_hints = true
hide_status_messages = false
daemon_on_close = "stop_if_we_started_it"
custom_theme = false
```

`theme.toml` takes a colour per field — a name (`cyan`), a palette index (`104`), a hex triplet
(`#7aa2f7`), or `reset` to let the terminal decide. Fields: `accent`, `border`, `border_focused`,
`title`, `text`, `muted`, `highlight`, `highlight_background`, `playing`, `success`, `error`.

## TESTS

```sh
cargo test
```

## LICENSE

MIT. See [LICENSE](LICENSE).
