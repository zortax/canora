//! Album art.
//!
//! The interface draws a picture from a `src` string. The framework loads a file or a registered
//! buffer, and it fetches nothing over the network, so this fetches the picture and registers the
//! bytes.
//!
//! One picture is asked for by many rows at once. Every ask for a picture that is already on its
//! way waits for that one fetch. A picture that was fetched before is read back from disk.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::{Mutex, broadcast};
use zgui_image::ImageBytes;

/// How many pictures stay registered.
///
/// A registration holds the bytes in memory and keeps its URL working. Dropping one frees the
/// memory; the next ask reads the file back from disk.
const HELD: usize = 512;

/// What stopped a picture from arriving.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ImageError {
    /// The fetch failed.
    #[error("cannot fetch the picture: {0}")]
    Fetch(String),
}

/// Holds album art and hands out `src` strings for it.
pub struct ImageCache {
    http: reqwest::Client,
    directory: PathBuf,
    state: Mutex<State>,
}

/// What a caller waiting on a fetch hears.
type Waiting = broadcast::Sender<Result<String, ImageError>>;

/// What the cache holds between calls.
#[derive(Default)]
struct State {
    /// Registered pictures, by URL. The order of `held` decides what is dropped first.
    registered: HashMap<String, ImageBytes>,
    /// The URLs in the order they were last asked for. The front is the oldest.
    held: Vec<String>,
    /// Pictures on their way, by URL. A second ask waits on the same channel.
    pending: HashMap<String, Waiting>,
}

impl ImageCache {
    /// Keeps pictures under `directory`.
    pub fn new(http: reqwest::Client, directory: PathBuf) -> Arc<Self> {
        if let Err(error) = std::fs::create_dir_all(&directory) {
            tracing::warn!(%error, "cannot make the picture directory");
        }
        Arc::new(Self {
            http,
            directory,
            state: Mutex::new(State::default()),
        })
    }

    /// The `src` string for the picture at `url`.
    ///
    /// Fetch it if this is the first ask. Wait for the fetch if one is already running.
    pub async fn src_for(&self, url: &str) -> Result<String, ImageError> {
        // Already registered: hand back the URL and mark it as recently used.
        {
            let mut state = self.state.lock().await;
            if let Some(bytes) = state.registered.get(url) {
                let src = bytes.url();
                touch(&mut state, url);
                return Ok(src);
            }
            if let Some(sender) = state.pending.get(url) {
                let mut receiver = sender.subscribe();
                drop(state);
                return match receiver.recv().await {
                    Ok(result) => result,
                    Err(error) => Err(ImageError::Fetch(error.to_string())),
                };
            }
            let (sender, _) = broadcast::channel(1);
            state.pending.insert(url.to_owned(), sender);
        }

        let result = self.load(url).await;

        let mut state = self.state.lock().await;
        if let Ok(src) = &result
            && let Some(bytes) = state.registered.get(url)
        {
            debug_assert_eq!(&bytes.url(), src);
        }
        if let Some(sender) = state.pending.remove(url) {
            // Nobody may be waiting. A send with no receiver is not a fault.
            let _ = sender.send(result.clone());
        }
        result
    }

    /// Reads the picture from disk, or fetches it, and registers the bytes.
    ///
    /// The reading and the fetching run off the interface's thread. That is not for speed: a
    /// future awaited on the interface's thread wakes nothing the platform can see, so a window
    /// with nothing else to do never polls it again and the picture never arrives. What
    /// [`zgui::reactive::background`] returns crosses back through the executor's wake edge, which
    /// asks for the frame that delivers it.
    ///
    /// Only the bytes cross. Registering them is the interface's own business and stays here.
    async fn load(&self, url: &str) -> Result<String, ImageError> {
        let path = self.path_for(url);
        let http = self.http.clone();
        let owned = url.to_owned();

        let bytes =
            zgui::reactive::background(async move { read_or_fetch(&http, &path, &owned).await })
                .await?;

        let registered = ImageBytes::new(bytes);
        let src = registered.url();

        let mut state = self.state.lock().await;
        state.registered.insert(url.to_owned(), registered);
        touch(&mut state, url);
        // Drop the oldest registrations. Their files stay, so they read back quickly.
        while state.held.len() > HELD {
            let oldest = state.held.remove(0);
            state.registered.remove(&oldest);
        }

        Ok(src)
    }

    /// Where the picture at `url` is kept.
    ///
    /// The name carries the whole address, folded to a number. A cover from `i.scdn.co` ends in an
    /// identifier that names it on its own, and one from `pickasso.spotifycdn.com` ends in the
    /// language it was drawn for: every Discover Weekly and Wrapped cover in the account ends in
    /// `/en` and would share one file.
    ///
    /// The last part of the address is kept in front of the number, so the directory can be read.
    fn path_for(&self, url: &str) -> PathBuf {
        let tail: String = url
            .rsplit('/')
            .next()
            .unwrap_or(url)
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(48)
            .collect();
        self.directory.join(format!("{tail}-{:016x}", fold(url)))
    }
}

/// The bytes of the picture at `url`, from `path` or from the network.
///
/// Holds nothing that belongs to the interface, so it runs off the interface's thread.
async fn read_or_fetch(
    http: &reqwest::Client,
    path: &Path,
    url: &str,
) -> Result<Vec<u8>, ImageError> {
    if let Ok(bytes) = tokio::fs::read(path).await {
        return Ok(bytes);
    }

    let response = http
        .get(url)
        .send()
        .await
        .map_err(|error| ImageError::Fetch(error.to_string()))?;
    if !response.status().is_success() {
        return Err(ImageError::Fetch(response.status().to_string()));
    }
    let bytes = response
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(|error| ImageError::Fetch(error.to_string()))?;

    // A picture that cannot be written is still shown. Report and carry on.
    if let Err(error) = tokio::fs::write(path, &bytes).await {
        tracing::warn!(%error, %url, "cannot keep the picture on disk");
    }
    Ok(bytes)
}

/// `text` folded to one number.
///
/// FNV-1a, written out rather than taken from the standard library: the hasher there may change
/// between compiler releases, and a name that changes throws away a cache that is still good.
fn fold(text: &str) -> u64 {
    const SEED: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    text.bytes().fold(SEED, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(PRIME)
    })
}

/// Marks `url` as the most recently used.
fn touch(state: &mut State, url: &str) {
    state.held.retain(|held| held != url);
    state.held.push(url.to_owned());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_a_file_after_the_picture() {
        let cache = ImageCache::new(reqwest::Client::new(), std::env::temp_dir().join("canora-test"));
        let path = cache.path_for("https://i.scdn.co/image/ab67616d0000b273886");
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
        assert!(name.starts_with("ab67616d0000b273886-"), "{name}");
    }

    #[test]
    fn two_covers_ending_the_same_way_get_two_files() {
        // Every Discover Weekly and Wrapped cover ends in the language it was drawn for. Naming
        // a file after that alone put them all in one place.
        let cache = ImageCache::new(reqwest::Client::new(), std::env::temp_dir().join("canora-test"));
        let one = cache.path_for("https://pickasso.spotifycdn.com/image/aaa/dt/v1/img/dw/cover/en");
        let two = cache.path_for("https://pickasso.spotifycdn.com/image/bbb/dt/v1/img/dw/cover/en");
        assert_ne!(one, two);
    }

    #[test]
    fn the_same_address_names_the_same_file() {
        let cache = ImageCache::new(reqwest::Client::new(), std::env::temp_dir().join("canora-test"));
        let url = "https://i.scdn.co/image/ab67616d0000b273886";
        assert_eq!(cache.path_for(url), cache.path_for(url));
    }

    #[test]
    fn keeps_the_most_recently_used_at_the_back() {
        let mut state = State::default();
        touch(&mut state, "a");
        touch(&mut state, "b");
        touch(&mut state, "a");

        assert_eq!(state.held, vec!["b".to_owned(), "a".to_owned()]);
    }
}
