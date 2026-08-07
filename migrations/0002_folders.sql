-- How the account arranges its playlists.
--
-- The Web API hands out a flat list; the order and the folders come from the streaming session.
-- One row per entry, in the order the sidebar shows them. A playlist inside a folder names it.

CREATE TABLE library_entry (
    position    INTEGER NOT NULL PRIMARY KEY,
    -- 'playlist' or 'folder'.
    kind        TEXT    NOT NULL,
    -- The playlist identifier, or the folder's own.
    id          TEXT    NOT NULL,
    -- What a folder is called. Empty for a playlist.
    name        TEXT    NOT NULL DEFAULT '',
    -- Which folder a playlist sits in, when it sits in one.
    folder_id   TEXT
) STRICT;
