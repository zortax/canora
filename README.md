# canora

A small, fast Spotify client. It plays audio itself through librespot and reads the library
through the Web API. The interface is built on [zgui](https://github.com/zortax/zgui). 
Spotify Premium is necessary.

<img width="1960" height="1314" alt="canora" src="https://github.com/user-attachments/assets/87b7c27b-dc8e-499c-ab31-61797e416489" />


## Build and run

```sh
cargo run
```

### Platforms

Linux, macOS and Windows. `Cargo.toml` picks the audio backend per target.

* **Linux** plays through PulseAudio, which is what PipeWire answers to. The build needs the
  PulseAudio headers: `libpulse-dev` on Debian and Ubuntu, `libpulse-devel` on Fedora, `libpulse`
  on Arch.
* **macOS** and **Windows** play through rodio, which reaches CoreAudio and WASAPI. Both ship with
  the system.

Canora tells the shell what is playing on Linux (MPRIS) and macOS (Now Playing) on macOS.

macOS shows Now Playing only for a bundled application, so build one to see the panel:

```sh
cargo install cargo-bundle
cargo bundle --release --format osx
open target/release/bundle/osx/canora.app
```

### The client identifier

Put a Spotify client identifier in `.env` beside `Cargo.toml`:

```
SPOTIFY_CLIENT_ID=<your identifier>
```

`build.rs` compiles it in, so the program needs no configuration to run. The identifier is public:
the login uses PKCE and carries no secret. `.env` stays out of git.

Make one at [the Spotify dashboard](https://developer.spotify.com/dashboard). Add exactly this
redirect address, which is what librespot listens on:

```
http://127.0.0.1:5588/login
```

Without an identifier the program still plays audio, and the Web API answers `429` to almost
every request: the identifier librespot falls back to is shared by every librespot program in the
world and its quota is spent.

### Assets

The icons and the themes are files under `assets`, compiled in where they are used.

* `assets/icons` — [Lucide](https://lucide.dev), except the transport and the hearts, which are
  [Phosphor](https://phosphoricons.com) filled: a play mark reads better solid. The heart has both
  styles, and an unsaved track shows the outline.
* `assets/themes` — one file per theme per surface. Adding a theme is two files and one line in
  the table in `src/ui/theme.rs`.
* `assets/app.css` — the sheet the window is drawn by.

### The local database

The library is cached in SQLite. The statements go through `sqlx`'s macros, so the schema in
`migrations` is checked at build time. The checked-in `.sqlx` directory holds that answer, so a
plain `cargo build` needs no database.

Changing a query means regenerating it:

```sh
export DATABASE_URL="sqlite://$PWD/target/dev-cache.db"
sqlx database create && sqlx migrate run
cargo sqlx prepare -- --all-targets
```

## What it does

* Browse and play the library: saved tracks, every playlist, albums and artists.
* Playlist folders, as the account arranges them, opening and closing with a swing.
* Spotify's own playlists: Discover Weekly, Release Radar, the Daily Mixes, Blends and Wrapped.
* Edit playlists: make one, rename it, delete it, add tracks and remove them.
* Save and unsave tracks from any list and from the bar.
* Search tracks, artists, albums and playlists.
* Play a station built around any track.
* Play, pause, seek, volume, shuffle, repeat, and a queue.
* Ten themes, each light and dark, following the desktop by default. Onyx is the black one,
  with a single green for everything the interface points at.
* A local copy of the library, so a second start draws a full window before Spotify answers. The
  header carries a button that reads it again.
* Its own window frame: the strip across the top moves the window and carries the controls.
* A welcome screen on a first run, with one button, and a page in the browser that says how the
  login went and closes itself. Every run after that opens straight onto the cached library and
  connects behind it; an account out of reach is a mark in the header rather than a wait.
* The desktop's own player: MPRIS on Linux and Now Playing on macOS, so the shell shows the track
  and the keys on a keyboard work.

## Diagnostics

The program takes a few commands that open no window:

Every one reports through `tracing`, so `RUST_LOG` decides how much comes out.

```sh
canora check            # sign in, then read the account, the playlists and the search
canora play             # play the top of the saved tracks and report what the engine does
canora raw /me          # print what the Web API answers for one path
canora station <track>  # print what the session's radio endpoints answer
canora rootlist         # print the playlist tree, folders and all
canora list <playlist>  # print one playlist as the session describes it
canora sp <endpoint>    # print what one session endpoint answers
canora cover <url>      # fetch one picture, with no window in the way
RUST_LOG=canora::api::http=debug canora   # every Web API request the window makes
```

## What Spotify no longer allows

Spotify closed several endpoints to applications registered after 2024, and closed more in its
2026 migration. This program works around the ones it needs:

* Batch reads (`?ids=`) answer `403`. Tracks are read one at a time, a few at once.
* `/v1/recommendations` answers `404`. A station comes from the streaming session instead.
* `/v1/artists/{id}/top-tracks` answers `403`. The artist page shows releases instead.
* The catalogue endpoints refuse a `limit` above ten.
* An artist carries no follower count.
* Spotify's own playlists — Discover Weekly, Release Radar, the Daily Mixes — answer `404`, and
  `/me/playlists` leaves them out. The session serves every one of them in full.
* Playlist folders are absent from the Web API entirely.

The last two both come from the streaming session, which is not held to the Web API's rules.
`src/api/direct.rs` reads them: the account's *rootlist* for the arrangement, and
`/playlist/v2/playlist/{id}` for a playlist the Web API will not describe. Both replies are the
same protobuf message, so one decoder serves both.


## License

Apache License 2.0. The full text is in [LICENSE](LICENSE).
