-- What the interface reads while it waits for Spotify.
--
-- Every row is a copy of something Spotify holds. Nothing here is authoritative: a sync replaces
-- it, and a row that disagrees with Spotify is stale rather than wrong.

CREATE TABLE playlist (
    id           TEXT    NOT NULL PRIMARY KEY,
    name         TEXT    NOT NULL,
    description  TEXT    NOT NULL DEFAULT '',
    -- 'owned' or 'followed'. Saved tracks are not a row here: they have no identifier.
    kind         TEXT    NOT NULL,
    -- Who owns it, for the ones this person only follows.
    owner_name   TEXT    NOT NULL DEFAULT '',
    total_tracks INTEGER NOT NULL DEFAULT 0,
    snapshot_id  TEXT,
    -- Where it sits in the sidebar.
    position     INTEGER NOT NULL DEFAULT 0,
    images       TEXT    NOT NULL DEFAULT '[]'
) STRICT;

CREATE TABLE track (
    id           TEXT    NOT NULL PRIMARY KEY,
    name         TEXT    NOT NULL,
    artists      TEXT    NOT NULL DEFAULT '[]',
    album_id     TEXT,
    album_name   TEXT    NOT NULL DEFAULT '',
    album_images TEXT    NOT NULL DEFAULT '[]',
    duration_ms  INTEGER NOT NULL DEFAULT 0,
    explicit     INTEGER NOT NULL DEFAULT 0,
    playable     INTEGER NOT NULL DEFAULT 1,
    track_number INTEGER NOT NULL DEFAULT 0
) STRICT;

-- Which tracks a playlist holds, and in what order. Saved tracks use the reserved identifier
-- below, so one table answers for every list.
CREATE TABLE playlist_track (
    playlist_id TEXT    NOT NULL,
    position    INTEGER NOT NULL,
    track_id    TEXT    NOT NULL REFERENCES track(id) ON DELETE CASCADE,
    PRIMARY KEY (playlist_id, position)
) STRICT;

CREATE INDEX playlist_track_by_track ON playlist_track(track_id);

-- Which tracks are saved.
CREATE TABLE saved_track (
    track_id TEXT NOT NULL PRIMARY KEY REFERENCES track(id) ON DELETE CASCADE
) STRICT;

-- When each list was last read from Spotify.
CREATE TABLE synced (
    what TEXT    NOT NULL PRIMARY KEY,
    at   INTEGER NOT NULL
) STRICT;
