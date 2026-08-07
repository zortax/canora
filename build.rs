//! Compiles the Spotify client identifier into the program.
//!
//! The identifier comes from `.env` beside this file, or from the environment. It is public by
//! design: the login uses PKCE, which carries no secret. Compiling it in means the program needs
//! no configuration to run.
//!
//! Nothing else is generated here. The icons and the themes are files under `assets`, and the
//! program includes them where it uses them.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=.env");
    println!("cargo:rerun-if-env-changed=SPOTIFY_CLIENT_ID");

    let from_file = std::fs::read_to_string(".env").ok().and_then(|text| {
        text.lines()
            .filter_map(|line| line.split_once('='))
            .find(|(key, _)| key.trim() == "SPOTIFY_CLIENT_ID")
            .map(|(_, value)| value.trim().trim_matches(['"', '\'']).to_owned())
    });

    let id = from_file
        .or_else(|| std::env::var("SPOTIFY_CLIENT_ID").ok())
        .unwrap_or_default();

    if id.is_empty() {
        println!(
            "cargo:warning=No SPOTIFY_CLIENT_ID. canora plays audio, and the Web API refuses \
             most requests. Put SPOTIFY_CLIENT_ID in .env and build again."
        );
    }
    println!("cargo:rustc-env=CANORA_CLIENT_ID={id}");
}
