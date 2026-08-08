//! Song radio: a queue of tracks that go with one track.
//!
//! There are two ways to ask, and the first one does not always answer.
//!
//! * `/v1/recommendations` is the documented endpoint. Spotify closed it to applications
//!   registered after November 2024, so a recent client identifier gets `403` or `404`.
//! * The station endpoint answers over the streaming session, which is not the Web API and is not
//!   restricted the same way. It returns track URIs, so the tracks are read back in one batch.
//!
//! Try the first. Fall back to the second.

use crate::api::{ApiClient, ApiError};
use crate::models::{Track, TrackId};
use crate::session::SessionCell;

/// How many tracks a radio queue holds.
const WANTED: usize = 50;

/// Reads a radio queue for one track.
pub struct Radio {
    session: SessionCell,
}

impl Radio {
    /// Asks over this session when the Web API refuses.
    pub fn new(session: SessionCell) -> Self {
        Self { session }
    }

    /// The tracks that go with `seed`, with `seed` at the front.
    ///
    /// Asking for a song's radio and hearing a different song first reads as the wrong station,
    /// so the seed leads it.
    pub async fn for_track(&self, api: &ApiClient, seed: &TrackId) -> Result<Vec<Track>, ApiError> {
        let mut tracks = self.related(api, seed).await?;
        tracks.retain(|track| &track.id != seed);

        // Read the seed itself, so the list opens with what was asked for.
        match api.tracks(std::slice::from_ref(seed)).await {
            Ok((mut first, _)) if !first.is_empty() => {
                first.append(&mut tracks);
                Ok(first)
            }
            Ok(_) => Ok(tracks),
            Err(error) => {
                tracing::warn!(%error, %seed, "cannot read the track the station is built on");
                Ok(tracks)
            }
        }
    }

    /// What the station holds, without the seed.
    async fn related(&self, api: &ApiClient, seed: &TrackId) -> Result<Vec<Track>, ApiError> {
        match api.track_radio(seed).await {
            Ok(tracks) if !tracks.is_empty() => return Ok(tracks),
            Ok(_) => tracing::info!("recommendations returned nothing, asking the session"),
            Err(error) if is_closed(&error) => {
                tracing::info!(%error, "recommendations are closed to this application");
            }
            Err(error) => return Err(error),
        }

        let ids = self.station_ids(seed).await?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        // Whether every track was read matters to a cache. A station is not kept, so the
        // count of what came back is the whole answer here.
        api.tracks(&ids).await.map(|(tracks, _)| tracks)
    }

    /// The track identifiers the session's station endpoint returns.
    ///
    /// The reply lists the station's tracks in play order. Read that list. Walk the whole document
    /// only if the shape is not the one this expects.
    async fn station_ids(&self, seed: &TrackId) -> Result<Vec<TrackId>, ApiError> {
        let bytes = self
            .session
            .get()
            .spclient()
            .get_apollo_station("stations", &seed.uri(), Some(WANTED), Vec::new(), true)
            .await
            .map_err(|error| ApiError::Auth(error.to_string()))?;

        let value: serde_json::Value = serde_json::from_slice(&bytes)?;
        let mut ids = match serde_json::from_value::<Station>(value.clone()) {
            Ok(station) => station
                .tracks
                .into_iter()
                .filter_map(|track| track_id_from_uri(&track.uri))
                .collect(),
            Err(error) => {
                tracing::warn!(%error, "station reply has an unexpected shape, reading every URI");
                let mut ids = Vec::new();
                collect_track_ids(&value, &mut ids);
                ids
            }
        };

        ids.retain(|id| id != seed);
        ids.dedup();
        ids.truncate(WANTED);
        Ok(ids)
    }
}

/// The station the session returns.
#[derive(serde::Deserialize)]
struct Station {
    #[serde(default)]
    tracks: Vec<StationTrack>,
}

/// One track of a station.
#[derive(serde::Deserialize)]
struct StationTrack {
    uri: String,
}

/// The track identifier in a `spotify:track:` URI.
fn track_id_from_uri(uri: &str) -> Option<TrackId> {
    let id = uri.strip_prefix("spotify:track:")?;
    (!id.is_empty()).then(|| TrackId(id.to_owned()))
}

/// Whether Spotify refused because the endpoint is closed, rather than because of a fault.
fn is_closed(error: &ApiError) -> bool {
    matches!(error, ApiError::Forbidden { .. } | ApiError::NotFound)
        || matches!(error, ApiError::Status { status, .. } if *status == 400)
}

/// Every track identifier anywhere in `value`.
///
/// The reply nests its list under keys this application does not pin down. Walk the whole document
/// and take every track URI, in the order they appear.
fn collect_track_ids(value: &serde_json::Value, out: &mut Vec<TrackId>) {
    match value {
        serde_json::Value::String(text) => {
            if let Some(id) = track_id_from_uri(text)
                && !out.contains(&id)
            {
                out.push(id);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_track_ids(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            for item in map.values() {
                collect_track_ids(item, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_every_track_uri_once_and_in_order() {
        let value = serde_json::json!({
            "mediaItems": [
                {"uri": "spotify:track:aaa"},
                {"uri": "spotify:track:bbb"},
                {"uri": "spotify:album:zzz"},
                {"uri": "spotify:track:aaa"}
            ],
            "seed": "spotify:track:seed"
        });
        let mut ids = Vec::new();
        collect_track_ids(&value, &mut ids);

        assert_eq!(
            ids,
            vec![
                TrackId("aaa".to_owned()),
                TrackId("bbb".to_owned()),
                TrackId("seed".to_owned())
            ]
        );
    }
}
