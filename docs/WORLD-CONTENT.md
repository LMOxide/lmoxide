# World content: from the world editor to play servers

What world editors make (spawns, bouncers, trigger walls, scripts and their waves, notes, smash
rules, and any other record on the editor's Definitions pages) is content records
(docs/CONTENT.md), in the world editor's copy of the content repository. It reaches play servers
through the content repository's `main`, after an admin approved it.

```
world editor (survey server, docs/SURVEY.md)
  var/content-work/            a git checkout of LMOxide/lmoxide-content: every editor's work,
                               saved as they make it (unpublished: differs from its last commit)
        │ Publish (the editor's header)
        ▼
  var/content-queue/<seq>.json one entry per publish: the records it changes, before and after
        │ approve (web UI, admin)            ── not built yet: see "Approval" below
        ▼
  LMOxide/lmoxide-content main  a commit per approved entry, authored as the editor, with
                               Content-Author:, Approved-By:, Lmoxide-Change: (content/README.md)
        │ git pull
        ▼
play servers (content/ or --content-dir): run it from their next start
```

## On play servers

A play server runs its content tree as loaded at start (`--content-dir`, default the checkout's
`content/`), the world editor's records like every other: spawns (surveyed ones too; spawns of a
script wave wait for their wave, scripts.rs), bouncers, trigger walls (walls.rs: invisible,
crossing one does what it says), scripts (scripts.rs, `!script list|start|stop`), smash rules
(achievements.rs). Features: `live` and what is nested under the server's active features
(lmoxide-content `Options::features`).

`--content-queue <dir>` (tools/lan-server.sh passes `../var/content-queue`, the survey server's)
lets admins see and decide on what editors published, on the control interface and through
lmoxide-api (admin):

| Route | |
|---|---|
| `GET /v1/content/state` | `content` (the tree it runs), `queue` (the folder), `queued` (how many wait) |
| `GET /v1/content/queue` | every entry, oldest first: `seq`, `created`, `by` (who published), `summary`, `status` (`queued`, `rejected`), `reason`, `records: [{ id, feature, change, file, author, before, after }]` |
| `GET /v1/content/queue/{seq}` | one entry |
| `POST /v1/content/reject` | `{ seq, reason }`: the entry stays, rejected, with the reason |
| `POST /v1/content/approve` | `{ seq }`: 501 for now (approval commits; below) |

The survey server lists and withdraws entries too (`GET /v1/editor/queue`,
`DELETE /v1/editor/queue/{seq}`), and leaves queued records out of what it calls unpublished.

## Approval (not built yet)

Decided (docs/WORLD-EDITOR-CONTENT.md): approving an entry commits exactly its records to the
content repository's `main` (the server holds the token, e.g. `LMOXIDE_CONTENT_GIT_TOKEN`; never
the web UI), with the trailers above, pushes, and play servers pull and spawn again what they
can (spawns, walls, scripts, bouncers), the rest on restart; undo is `git revert` of that commit.
Pull requests merged on GitHub arrive the same way. Before approving, an admin can **preview** an
entry: the play server applies its records in memory for a while (15 minutes), then drops them.
On hold at the user's request (they may change the approach), so approve answers 501 and preview
isn't there yet.

## Before content records

Until 2026-10-28 editors worked on a survey set (`var/survey-set`) that play servers approved
into their own copy (`<data-dir>/world-content/`: a ledger of changes, rules, a rejected list).
That is gone: the survey server took its set into the working copy once
(`var/survey-set/migrated.json` says what it did), and play servers no longer read
`world-content/` (it can be deleted).
