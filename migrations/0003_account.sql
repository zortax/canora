-- Who was signed in when the window last closed.
--
-- The window opens on what the cache holds rather than on a waiting screen, so it needs a name for
-- the header before it has said a word to Spotify.
CREATE TABLE account (
    -- One row. The identifier is what keeps it that way.
    only_row INTEGER PRIMARY KEY CHECK (only_row = 1),
    id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    product TEXT NOT NULL,
    images TEXT NOT NULL
) STRICT;
