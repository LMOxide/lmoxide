# Game options

Server options that change how the game plays. Each is a command-line flag of `lmoxide-server`
(or a key in the `[game]` table of `lmoxide.toml`, without the `--`) and an environment variable.
Values the client data doesn't contain are in [TUNABLES.md](TUNABLES.md).

## Progression

Progression is the default: characters start at level 1, earn XP from defeated enemies (capped
per world) and rank minifigures up with stars, as on live. One server runs several game modes on
their own universe ports (`--modes`, `LMOXIDE_MODES`; the patcher's Game Mode picks one):

| Mode | |
|---|---|
| `progression` | the live behaviour (the default) |
| `maxed` | every character is level 30 and every minifigure is fully ranked |
| `world-editor` | for contributors: nothing moves or attacks; content is placed from the dev UI ([SURVEY.md](SURVEY.md)) |

Without `--modes`, `--mode` (`LMOXIDE_MODE`) picks the one mode: `progression` (the default),
`play` (maxed) or `survey` (the world editor).

## Aiming at moving enemies

When a player attacks an enemy, the server aims the shot. `--lob-aim` (`LMOXIDE_LOB_AIM`) sets
this for thrown attacks that land where they were aimed (bombs, eggs). `lead` (the default) aims
where the enemy will be when the shot lands. `current` aims where the enemy was at the throw,
which misses an enemy running at you. `--shot-aim` (`LMOXIDE_SHOT_AIM`) does the same for
straight shots: `current` (the default) or `lead`. A GM can change both while the server runs
with `!aim`.

## Story and teleport windows

Every teleport window is open by default, whatever your story. `--story-windows locked`
(`LMOXIDE_STORY_WINDOWS`) makes them open as your story reaches them, as on live. Any player can
start their own story over by typing `/story reset` in chat (it asks for `/story reset confirm`).
This forgets story quests, cinematics and conversations seen. Achievements, minifigures and levels
stay. Entering a world afterwards puts your story there.
