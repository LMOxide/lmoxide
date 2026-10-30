-- Which character friendships an account friendship made (1): removing the account friendship
-- removes only those, not friendships the characters had before or made themselves in game.
ALTER TABLE friendships ADD COLUMN account_friend INTEGER NOT NULL DEFAULT 0;
