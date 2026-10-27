# Plan: the world editor edits the content tree

A hand-off for the session that overhauls the world editor (repository `lmoxide-dev-ui`, page
`/editor/`) so that it edits the game's content tree directly, with forms built from the
content JSON Schemas. It touches three repositories: `lmoxide` (survey server, content crate,
API), `lmoxide-dev-ui` (the editor) and `lmoxide-content` (the data). The web UI
(`lmoxide-webui`) gets the approval queue, by its own session, from the API this plan adds.

Read first, in this order: `docs/CONTENT.md` (the format), `content/README.md`, `docs/SURVEY.md`
(how the survey server and editor work today), `docs/WORLD-CONTENT.md` (publish, approval,
history on play servers), `crates/lmoxide-content/src/model.rs` (every record type),
`content/schemas/*.schema.json`, and in the dev-ui `src/editor/` (`WorldEditor.tsx`,
`Browser.tsx`, `Tree.tsx`, `nodes.tsx`, `Details.tsx`, `backend.ts`, `mock.ts`, `types.ts`).

## Where things stand

- **The content tree** (`lmoxide/content/`, a submodule of the private repository
  `LMOxide/lmoxide-content`) holds all game data: 19 record kinds (`world`, `spawn`, `portal`,
  `bouncer`, `area`, `trigger`, `puzzle`, `script`, `region`, `npc`, `faction`, `object`, `skill`,
  `buff`, `loot`, `item`, `minifig`, `dialogue`, `achievement`) plus `tunables.json`,
  `texts.json` and `content.json`. Ids are `<kind>/<slug>` (world records
  `<kind>/<playfield>/<slug>`), files are canonical JSON (`lmoxide-content fmt`), every record has
  `extends`, `feature`, `source`, `tags`, `comment`, `rev`, `author` and `updated`. The loader
  (`lmoxide_content::load`) applies features, `extends` and spawn `overrides` and reports
  every problem. The game and API servers read only this tree.
- **JSON Schemas** (`content/schemas/<kind>.schema.json`, written by
  `cargo run -p lmoxide-content -- schema`, from the Rust types with schemars) carry the
  field descriptions from the doc comments. They don't yet say which kinds a reference may name,
  which client catalog an id comes from, or which folder a kind lives in (phase 1).
- **The world editor** edits a separate format: the survey set (`lmoxide-survey` crate:
  placements, trigger walls, notes, scripts and waves, smash rules) in `var/survey-set/`, on the
  survey server (`--mode survey`, `survey.rs`, routes `/v1/survey/*`). Editors publish to
  `published/`; play servers queue, approve, record history and apply it over the base content
  (`content.rs`, `/v1/content/*`, `lmoxide-survey::ledger`). The importer turned a snapshot of the
  published set into content records (`source: surveyed`), but the set is still the live copy
  and play servers still apply it.
- **The editor UI** (React 19, Tailwind v4, shadcn/radix components in `src/components/ui`,
  `combobox.tsx`): one tree with tabs Spawns / Scripts / Achievements (`nodes.tsx`), a details
  pane per selection (`Details.tsx`, hand-written per kind), in-game placing and selection,
  hiding, marks, wave preview, trigger walls (`Walls.tsx`), notes, areas (`AreaEditor.tsx`),
  features, tags, undo/redo (`history.ts`), import/export, publish (`Publish.tsx`), a mock
  backend (`?mock`, `mock.ts`).

## Goal

1. Every record kind can be browsed, searched, created, edited, copied and deleted in the
   editor, with forms generated from the schemas, not written per kind.
2. In-game work (placing, moving, confirming, deleting, trigger walls, waves, patrols) edits
   content records (`spawn`, `bouncer`, `trigger`, `script`, `area`), not the survey set.
3. Concurrent editors don't overwrite each other (`rev`, field-level merge); every record shows
   its problems, where it's used, and its history.
4. What editors publish is approved in the web UI; the server commits it to
   `lmoxide-content` `main` and pushes, and play servers take it in. Contributors via git (pull
   requests) end up in the same history.
5. The survey set format, its import/merge and the play servers' own approval ledger are retired
   once nothing uses them.

## Decided by the user

- Content lives in its own repository (`LMOxide/lmoxide-content`), private for now, license
  undecided (no outside contributions until there is one).
- Approval happens in the web UI. The server, not the browser, holds the GitHub token, commits
  to `main` and pushes. A commit is authored as the editor and records who approved it (trailers:
  `Content-Author:`, `Approved-By:`, `Lmoxide-Change:`; see `content/README.md`).
- Readable slug ids that hold client ids; hand-written units (`"1.5s"`, `"20%"`); features
  extend or override records; no CRDT (`rev` plus field-level three-way merge, git for history).
- The user wants shadcn components in the editor, not hand-written ones, and testing with as few
  screenshots as possible.

## Ask the user before starting (with a recommendation each)

1. **Where unpublished edits live.** Recommended: the survey server keeps a git working copy
   of `lmoxide-content` (`var/content-work`), autosaved as editors work (as the survey set is
   today). "Publish" turns an editor's changed records into a queue entry; approval commits
   exactly those records. Alternative: a git branch per editor.
2. **Play servers taking new content.** Recommended first step: they `git pull` and restart
   their content-dependent state with the existing "playfields respawn" path (spawns, walls,
   scripts, bouncers reload live today through `content.rs`); everything else on restart. A
   full live reload of skills, loot, buffs etc. is a later step (phase 6).
3. **Who may edit definitions** (NPCs, skills, buffs, loot, items, minifigures, tunables) as
   opposed to world records. Recommended: every editor proposes; only admins approve, as now.
4. **Deleting a record others refer to.** Recommended: refuse, listing the references, with a
   "replace references with …" action.
5. **The play servers' preview** ("try this change for 15 minutes", WORLD-CONTENT.md). Keep it
   (the server applies the proposed records in memory) or drop it.

## Phases

Each phase ends with its checks passing, committed (lmoxide and lmoxide-dev-ui commit straight
to `main` / `master`; push only when the user asks). Write `docs/` as you go.

### 1. Schemas the editor can build forms from (lmoxide, `lmoxide-content`)

- **Reference kinds.** `Ref` is a plain `String` today. Annotate every reference field with the
  kinds it may name, e.g. schemars `#[schemars(extend("x-lmoxide-ref" = ["npc", "object"]))]`
  on `Spawn.template` (or newtype refs per kind: `Ref<Npc>`, if it stays readable). Also refs
  inside `Action` variants and `BuffEffect`, `LootBundle` (`items`, `chest`, `table`), map keys
  (`World.arrival_from`).
- **Client ids.** Annotate numbers that are client ids with the catalog they come from:
  `x-lmoxide-client: "monster_data" | "effect_package" | "spell" | "item" | "visual" | "text"
  | "nano" | "stat"`. The editor uses them for pickers with names and previews.
- **Kinds index.** `schemas/index.json`: per kind its id prefix, folder or world file
  (`lmoxide_content::KINDS`), title, description, whether it's per world, the fields to show in
  lists (name, kind, level ...).
- **Units and shapes.** Duration, Ratio and Range already have patterns; add `x-lmoxide-unit`
  so the form shows a unit-aware input. Positions (`Vec3`) and `Rot`: `x-lmoxide-position`,
  `x-lmoxide-rotation` (the form offers "use my position" and a heading dial).
- **A test** that validates every record in the tree against its schema (dev-dependency
  `jsonschema`), so the schemas and the loader can't drift.
- Regenerate `content/schemas` and commit it in the content repository.

### 2. A content editing API on the survey server (lmoxide)

New routes on the survey server, passed on by `lmoxide-api` like `/v1/survey/*` (survey API
port, every account an admin there). Keep them separate from the play servers' `/v1/content/*`
(approval), e.g. `/v1/editor/*`:

| Route | |
|---|---|
| `GET /v1/editor/kinds` | `schemas/index.json` and every schema |
| `GET /v1/editor/records?kind=&world=&feature=&tag=&source=&q=` | lists: id, name, kind, world, feature, source, tags, problems count, changed (unpublished) |
| `GET /v1/editor/records/{id}` | the record as written, as resolved (features, `extends`, overrides; each value's origin), its problems, where it's used (back-references), its file |
| `PUT /v1/editor/records/{id}` | `{record, base_rev}`: field-level three-way merge with the working copy (conflicts returned, nothing saved), validation (the record parses, its references exist), `rev`+1, `author`, `updated`, canonical write |
| `POST /v1/editor/records` | create: kind, name (slug -> id, numbered if taken), world, feature, fields; or `copy_of` |
| `DELETE /v1/editor/records/{id}` | refuses while referenced (lists them) unless `replace_with` |
| `GET /v1/editor/problems` | the whole tree's `check` |
| `GET /v1/editor/catalog/{name}` | client catalogs for pickers (looks from `data/monster_data.json`, effects, spells, items, texts, stats), searchable |
| `GET /v1/editor/records/{id}/history` | the file's git log (author, approver, date, diff) |
| `GET /v1/editor/changes` | the working copy's unpublished changes per author, as record diffs |
| `POST /v1/editor/publish` | an author's chosen changes -> a queue entry (records, before/after) |
| `POST /v1/editor/loot/{id}/simulate` | roll a loot table N times (drop rates for the loot editor) |

- The working copy is a git checkout of `lmoxide-content` (decision 1); writes are autosaved
  (debounced, as the survey set is today). `lmoxide-content` gets the merge (three-way, per
  field, on `serde_json::Value`) and single-record validation, with unit tests.
- **In-game routes change underneath.** `place`, `objects/{id}` (move, edit), `confirm`,
  `forget`, `place-trigger`, waves and notes write content records: a placement is a `spawn`,
  `bouncer` or `trigger` in its world's file; a guess is confirmed by editing that spawn
  (`source: surveyed`, tag `verified`), deleted by removing it (or `removed: true` in a non-live
  feature), changed into another spawn by changing its `template`. The survey's base/overlay
  split (`Origin`, `replaces`) goes away. The survey server spawns from the working copy and
  respawns a playfield when its records change (it already does for the set).
- Keep the game-side features: hiding (per editor, not saved), marks, fire circles, wave preview,
  trigger-wall looks.

### 3. Approval and the content repository (lmoxide server and API; web UI hand-off)

- Play servers' `/v1/content/queue|approve|reject|history` read the survey server's queue
  entries (as they read `published/` today). Approving commits the entry's records to the play
  server's checkout of `lmoxide-content` with the trailers, pushes (token in the server's
  environment, e.g. `LMOXIDE_CONTENT_GIT_TOKEN`; a fine-grained token or GitHub App that can
  write that one repository), then pulls and reloads (decision 2). Undo is `git revert` of that
  commit (also committed and pushed). Rejected entries stay out of the queue until changed.
- Pull requests merged on GitHub reach the servers the same way (they pull `main`).
- Write the web UI's hand-off (update `docs/WEB-UI.md`): the queue shows record diffs, approve
  and reject, history with authors and approvers; the API contract in `docs/openapi.json`
  (`UPDATE_OPENAPI=1 cargo test -p lmoxide-api`).

### 4. The editor: schema-driven forms (lmoxide-dev-ui)

One form engine (`src/editor/form/`) that renders any record from its schema, with shadcn
components:

- objects (fields in schema order, optional fields hidden behind "add field", descriptions as
  hints), arrays (add, remove, reorder), maps (key/value), enums (select with each value's
  description), externally tagged enums (`Action`, `BuffEffect`, `Shape`, `WaveTrigger`,
  `Target`, `SpawnAt`: a variant picker, then that variant's form);
- references: a combobox of records of the allowed kinds (search, "go to", "create new …");
- client ids: a picker over the catalog (name, id, preview where there is one);
- units: text inputs that accept `1.5s`, `200ms`, `20%`, numbers, and show what they parsed to;
- positions and headings: inputs plus "use my position" (from the editor's game state) and
  "show in game" (teleport/mark);
- inheritance: values coming from `extends`, a feature or a spawn's template shown greyed with
  their origin, an "override" toggle per field; spawn `overrides` edited as a partial form of the
  template's schema;
- the shared fields (feature, source, tags, comment) in one header; `rev`, `author`, `updated`
  read-only.

Replace the hand-written forms in `Details.tsx` with the engine where it covers them; keep custom
widgets as field renderers registered by schema path or annotation (the area polygon editor,
wave stepper, wall size and look, trigger targets).

### 5. The editor: browsing everything (lmoxide-dev-ui)

- The browser gets a **Definitions** side: one folder per kind (NPCs, objects, skills, buffs,
  loot, items, minifigures, dialogues, achievements, regions, factions, tunables, texts), lists
  from `/v1/editor/records` with search and filters (feature, tag, source, author, "has
  problems", "changed"). The existing tabs (Spawns, Scripts, Achievements) become views over
  content records.
- Every record view: the form, problems, "used by" (back-references, clickable), history (diffs
  with author and approver), its unpublished diff, copy, delete.
- Special views where a form isn't enough: loot tables (entries plus simulated drop rates),
  buffs (effects and procs as readable sentences next to the form), skills (client action lists
  read-only with the server-native fields editable), dialogues (texts in order from texts.json),
  minifigures (stats, attacks, upgrades with their buffs).
- Conflicts: when a save returns conflicts, show both values per field and let the editor pick.
- Undo/redo (`history.ts`) works on record versions.
- The mock backend (`mock.ts`) serves a copy of the real content tree (a fixture built from
  `lmoxide/content`), so the editor can be developed and tested without servers.

### 6. Migration and clean-up (lmoxide)

- Freeze the survey set (tell the user first): convert its current working copy and published
  folder with the importer's survey part (`crates/lmoxide-content/src/import/survey.rs`) into
  content records, check (`lmoxide-content check`, `tools/content/verify_import.py`-style counts),
  then switch the survey server to the working copy.
- Remove the survey set format, `published/`, the import/merge routes and the play servers'
  `world-content/` ledger once nothing reads them; adapters that only existed for them go too.
- Later: a full live reload of content on play servers (gamedata plus the `LazyLock` tables in
  `skills.rs`, `spawns.rs`, `world_data.rs`, `minifigs.rs`, `loot.rs`, `procs.rs` ...), so an
  approved skill or loot change doesn't need a restart.

## How to test

- lmoxide: unit tests for merge, validation, id allocation, delete-with-references, the git
  commit (in a temporary repository: author, trailers, revert), the schema test; `cargo test
  --workspace`; `cargo run -p lmoxide-content -- check` and `fmt --check` after every write path.
- dev-ui: `npm run typecheck`, the mock backend, and browser checks with playwright-core
  (`~/Dropbox/Development/typescript/monergismDownload/node_modules`, `/bin/chromium`; mock the API
  with `page.route` on pathname `/api/v1/` or `/editor-api/v1/`). Prefer assertions on the DOM
  over screenshots.
- In game only where nothing else can show it (placing, moving, waves), with the Linux test
  client (`LMO_RUNNER=linux`); the user's LAN and survey servers may be restarted to test
  (`tools/servers.sh restart both`).

## Rules of the house

- Match live behaviour; guesses go in `docs/TUNABLES.md`, and content records say `source`.
- Don't run `cargo fmt` on `lmoxide-server`: it isn't rustfmt-clean and reformats ~50 files.
- Leave other sessions' uncommitted files alone (check `git status` before committing).
- Commit messages end with the attribution lines the session is given; push only when asked.
- Content changes are commits in `content/` (the submodule) and then the pointer in lmoxide.
