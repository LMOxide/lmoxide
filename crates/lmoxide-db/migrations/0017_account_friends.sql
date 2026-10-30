-- Account friends: one account asks another; once accepted, every character of each is friends
-- with every character of the other (rows in `friendships`), those made later too
-- (Db::save_character). A request answered no is deleted.
CREATE TABLE account_friends (
    requester    INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    target       INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    -- 0: waiting for the target's answer; 1: friends
    accepted     INTEGER NOT NULL DEFAULT 0,
    created_at   INTEGER NOT NULL,
    accepted_at  INTEGER,
    -- when the Discord bot sent the target the request (in a DM); NULL: not yet
    notified_at  INTEGER,
    PRIMARY KEY (requester, target),
    CHECK (requester <> target)
);
CREATE INDEX account_friends_target ON account_friends(target);
