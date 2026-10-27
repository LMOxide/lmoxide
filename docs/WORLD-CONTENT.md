# World content: from the world editor to the game servers

Contributors only need the game client and a browser: they edit worlds on the world editor's
survey server (docs/SURVEY.md, the dev UI's world editor), publish what is ready, and play
servers take it in once it is approved. No exports, imports or files to pass around.

```
world editor (survey server)              play server (LAN, public, ...)
  var/survey-set/  = the working copy
  (every editor's records, autosaved
   as they work, author + feature)
  var/survey-set/published/  ──────────►  <data-dir>/world-content/
  (what editors published)                  baseline/      approved before histories (once)
                                            changes/       one file per approval (history)
                                            approved/      the approved copy, built from them
                                            rules.json     active features, auto-approval
                                            rejected.json  rejected versions
```

## Features

Every record (placement, trigger wall, note, script) names a feature, nested with `/`; the first
part is its category:

| Category | Id | For |
|---|---|---|
| Mimic live | `live` (default) | recreating live as it was |
| New content | `feature/...`, e.g. `feature/christmas-event/2026` | events, new areas, our own additions |
| Other | `other/...` | fixes, experiments |

Features beyond the categories live in the set's `features.json` (title, comment, author,
`copied_from`). The world editor creates them (`POST /v1/survey/features {parent, title}`), and
copies one with everything in it (`POST /v1/survey/features/copy {from, parent, title}`): the
copy gets new ids, scripts' waves stay connected, so `feature/christmas-event/2027` can start
as 2026 and then differ. Placing, place-trigger, edits and snapshots take a `feature`.

## Play servers

`lmoxide-server --world-editor-set <survey set folder>` (tools/lan-server.sh passes
`../var/survey-set`): every 10 s its `published/` folder is read again when a file in it changed
(the working copy beside it is never read: unpublished work stays on the editor's server). The
difference between the published set and the approved copy is the **queue**: one entry per playfield, feature
and author, with each record added, changed or removed and what changed ("moved 3.2 m",
"deletes a guess", "level 5 -> 7").

- **Rules** (`rules.json`): `active` features are spawned (default `["live"]`; a feature is on
  when it or an ancestor is listed, so a Christmas event is approved ahead and switched on when it
  starts); `auto_features` (e.g. `live`) and `auto_authors` are approved as they come.
- **Approve** takes the working version of a change (or of some of its records) into the approved
  copy; **reject** keeps that version out of the queue until the record changes again.
- **History** (`changes/000001.json`, ...): every approval (by an admin or the rules) is a file with
  who, when, the change key and each record before and after. The approved copy is the baseline
  plus the changes that apply, in order, so any change can be **undone** (later changes to the same
  records keep theirs) and **redone**. An undone version stays out of the queue, as a rejected one,
  until the record changes again (so auto-approval rules don't take it back).
- **Preview**: a waiting change applied for everyone for a while (default 15 minutes): **keep**
  approves it, **revert** or its time ends it and it waits in the queue again. Previews are seen by
  every player: one only the admin sees would need enemies that fight only some players.
- **Live reload**: playfields whose approved, active placements change are spawned again (base
  content with the placements applied, as the survey server shows them). Not on play servers yet:
  placements of script waves, trigger walls, notes (they wait in the approved copy).
- Without `--world-editor-set` the approved copy is still applied.

Routes, admin scope, through lmoxide-api (`/v1/content/...`, forwarded to the control interface):

| Method | Path | |
|---|---|---|
| GET | `/v1/content/state` | source folder and error, rules, revision, pending changes/records, `authors` and `tags` of either set (for suggestions) |
| GET | `/v1/content/queue` | changes waiting: `{playfield, playfield_name, feature, feature_path, author, active, summary, items: [{kind, id, name, op, details, updated, tags}]}` (details include "tags +todo -approximate") |
| POST | `/v1/content/approve` | `{playfield, feature, author, ids?}` → `{records, revision}` |
| POST | `/v1/content/reject` | same |
| POST | `/v1/content/preview` | `{playfield, feature, author, ids?, minutes?}` → `{records, revision, seq, until}` |
| GET | `/v1/content/history?limit=` | approvals, newest first: `{seq, at, by, playfield, playfield_name, feature, feature_path, author, summary, status: applied\|undone\|preview\|preview_ended, preview_until?, last_action?: [who, when], actions: [undo\|redo\|keep\|revert], edits: [{kind, id, name, op, details, tags}]}` |
| POST | `/v1/content/history/{seq}/{undo\|redo\|keep\|revert}` | → the entry as above |
| GET/PUT | `/v1/content/rules` | `{active, auto_features, auto_authors}` (saving approves what the new rules approve and respawns what `active` changes) |
| GET | `/v1/content/summary?of=approved\|working` | per playfield and feature: placements, deletions, in waves, triggers, notes, scripts, authors, `tags` (tag → records), newest change |
| GET | `/v1/content/features` | categories and the world editor's features (`{id, path, comment, copied_from}`) |
| POST | `/v1/content/refresh` | read the world editor's set now |

The admin dashboard (lmoxide-webui) is the place for the queue, rules and summaries (docs/WEB-UI.md).

## Decisions and open questions

- **Storage**: the working copy stays the survey set folder (plain JSON per playfield, records
  sorted by id, small diffs) rather than per-user `.json.gz` files: authors are on every record
  and the queue groups by author, so per-user files add nothing but merging work, and plain JSON
  diffs well if the folder is put under git.
- **Its own git repository?** Not needed for the flow above. Worth it for history and backups of
  the approved copy (or the working copy): a repo per server's `world-content/approved/` (or a
  shared `LMOxide/world-content`) that a cron job commits to. Not set up; the user's call.
- **Remote editor servers**: `--world-editor-set` is a folder, so the editor server must be on the
  same host (or a synced/mounted folder). An HTTP source (the survey server's export) is next if
  the editor server moves elsewhere.
