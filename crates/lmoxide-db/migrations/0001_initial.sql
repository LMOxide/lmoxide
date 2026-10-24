-- Accounts log in with a username and password. `id` is the account instance the client sees
-- (ID32 instance, < 2^31); characters currently share it (one character per account).
CREATE TABLE accounts (
    id             INTEGER PRIMARY KEY,
    username       TEXT NOT NULL UNIQUE COLLATE NOCASE,
    -- argon2 PHC string; NULL = no password set yet
    password_hash  TEXT,
    gm_level       INTEGER NOT NULL DEFAULT 0,
    created_at     INTEGER NOT NULL,
    last_login_at  INTEGER,
    last_login_ip  TEXT,
    banned         INTEGER NOT NULL DEFAULT 0,
    -- unix time the ban ends; NULL with banned = 1 is permanent
    ban_until      INTEGER,
    ban_reason     TEXT
);

CREATE TABLE characters (
    id          INTEGER PRIMARY KEY,
    account_id  INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    playfield   INTEGER NOT NULL,
    pos_x       REAL NOT NULL,
    pos_y       REAL NOT NULL,
    pos_z       REAL NOT NULL,
    stars       INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL,
    saved_at    INTEGER NOT NULL
);
CREATE INDEX characters_account ON characters(account_id);

-- Minifigure team: inventory 101 position and item template, in team order (slot 0 active).
CREATE TABLE character_team (
    character_id  INTEGER NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    slot          INTEGER NOT NULL,
    position      INTEGER NOT NULL,
    template      INTEGER NOT NULL,
    PRIMARY KEY (character_id, slot)
);

-- Active upgrade bitmask per minifigure template.
CREATE TABLE character_upgrades (
    character_id  INTEGER NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    template      INTEGER NOT NULL,
    mask          INTEGER NOT NULL,
    PRIMARY KEY (character_id, template)
);

-- Token counters (minifigure parts), as UpdateTokenAmount sends them.
CREATE TABLE character_tokens (
    character_id  INTEGER NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    token         INTEGER NOT NULL,
    gained        INTEGER NOT NULL,
    spent         INTEGER NOT NULL,
    PRIMARY KEY (character_id, token)
);

-- Friendships between characters, stored once with low < high.
CREATE TABLE friendships (
    low         INTEGER NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    high        INTEGER NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (low, high),
    CHECK (low < high)
);

-- Keys for the REST API (bots, scripts). Only a SHA-256 of the secret is stored.
CREATE TABLE api_keys (
    id            INTEGER PRIMARY KEY,
    name          TEXT NOT NULL,
    key_hash      BLOB NOT NULL UNIQUE,
    scopes        TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    last_used_at  INTEGER,
    revoked       INTEGER NOT NULL DEFAULT 0
);

-- REST API sessions from an account login (web UI). Only a SHA-256 of the token is stored.
CREATE TABLE api_sessions (
    token_hash  BLOB PRIMARY KEY,
    account_id  INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    created_at  INTEGER NOT NULL,
    expires_at  INTEGER NOT NULL
);

CREATE TABLE audit_log (
    id       INTEGER PRIMARY KEY,
    at       INTEGER NOT NULL,
    actor    TEXT NOT NULL,
    action   TEXT NOT NULL,
    target   TEXT,
    details  TEXT
);

CREATE TABLE settings (
    key    TEXT PRIMARY KEY,
    value  TEXT NOT NULL
);

-- Login cookies handed out by the universe agent after a password check; the territory, game
-- and chat servers accept a client only with its account's live cookie.
CREATE TABLE game_sessions (
    cookie      INTEGER PRIMARY KEY,
    account_id  INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    created_at  INTEGER NOT NULL,
    expires_at  INTEGER NOT NULL
);
CREATE INDEX game_sessions_account ON game_sessions(account_id);
