-- Discord accounts linked to game accounts (the Discord bot), API keys that act as an account,
-- password-reset sessions, and the one-time codes the bot sends in DMs.

-- A Discord user id (snowflake, as decimal text); NULL = not linked.
ALTER TABLE accounts ADD COLUMN discord_id TEXT;
CREATE UNIQUE INDEX accounts_discord ON accounts(discord_id) WHERE discord_id IS NOT NULL;

-- Set: the key acts as this account (its scope follows the account's GM level).
ALTER TABLE api_keys ADD COLUMN account_id INTEGER REFERENCES accounts(id) ON DELETE CASCADE;
CREATE INDEX api_keys_account ON api_keys(account_id) WHERE account_id IS NOT NULL;

-- 1: a session from a one-time sign-in code, which may set a password without the old one.
ALTER TABLE api_sessions ADD COLUMN password_reset INTEGER NOT NULL DEFAULT 0;

-- One live code per Discord user and purpose ('link' or 'login'); only a SHA-256 is stored.
CREATE TABLE discord_codes (
    discord_id  TEXT NOT NULL,
    purpose     TEXT NOT NULL,
    code_hash   BLOB NOT NULL UNIQUE,
    created_at  INTEGER NOT NULL,
    expires_at  INTEGER NOT NULL,
    PRIMARY KEY (discord_id, purpose)
);
