# Client-to-server actions from the live capture

Source: `LMOp1-lmo-exe.cap` (about 381 s, one login session, two characters on one account).
Decoder: `tools/re/live_client_actions.py` (run `python3 tools/re/live_client_actions.py <capture>`).
Outputs: `client-actions.json`, `player-movement.json` (both in this folder).

## Method names and arguments

Argument layouts come from `data/rpc_methods.json` and are checked per message against
`args_len` (the decoder requires the whole argument to be consumed). Fields marked unverified
keep their raw meaning only.

- `Ping` (559): sequence number. Not written to events.
- `TeleportMe(playfield, spawn)`: request to go to a playfield.
- `TeleportWindowBypass(window, playfield)`: same, sent by the tutorial map (server: tutorial.rs).
- `TeleportMessageBoxAnswer(int)`: the value is the destination playfield (positive; high bit =
  enter existing instance), -1 = cancel. Confirmed by `crates/lmoxide-server/src/game.rs` (the
  server teleports on the same value). The capture contains no server reply to check it against.
- `EndlessPlayfieldsTeleportMessageBoxAnswer(int, ctx)`: 1 seen 4 times. Meaning of 1 is
  unverified (matches the Yes value of ScryMessageBoxAnswer); the destination is server-chosen.
- `ClientRequestDestinationPlayfieldStatus(PlayfieldProxy)`: a query. Carries the destination as
  ProtoID32 model_id (type 51100, instance = playfield). Does not change the current playfield.
- `CinematicTransition(ID32, bool)`, `ExitCinematic`: entering/leaving cinematic (36 / 3).
- `ScryMessageBoxAnswer(int, ctx, uint64 cookie)`: 1 = Yes (33).
- `TryLootItem(bag ID32, item_pos, char ID32, desired_pos)` (633, 303 distinct bags, type 51322).
- `UpdateSpellTargetPosition(commandID, Vector3)` (481): aim point of a spell.
- `AddShortcut` / `RemoveShortcut` (132 / 90): fields are type, position, data, inventory type.
- `TutorialShown(tag)` (15): tag ids 4221 and 330 etc.; not matched to any data file.
- `CreateCharacterWithData`, `SetCharacterCreationData`: strings "Shrouded", "Electric", "Echo" then a
  u32 (layout unverified beyond the strings).
- `LoginCharacter`, `Authenticate`, `Authenticate2`, `InitiateAuthentication`,
  `AnswerChallenge`: the auth cookies and the challenge response are not written out.
- Item shop, guild, gear, friends, resurrection: counted (counts in client-actions.json), arguments
  decoded as listed in the decoder.

## IIR (SignalPassBlob) records

Each SignalPassBlob argument is a BinaryStream: u32 length + one IIR record (u32 type, ID32
identity, bool isToBePassedOn, class body). The length matched in all 9,398 messages.
One record per message is assumed: the decoder reads only the first record. Exact consumption was checked for every parsed body; unparsed bodies were not checked.

Verified bodies:
- `CharDCMoveIIR_t` (1,509): flags, position (3 f32), 8 bytes rotation (unverified: two f32 in the
  samples), align byte, velocity if not flags & 0x04, second vector if not flags & 0x08, f32, 2 bytes.
  Size check passed for all 1,509.
- `MoveTaskIIR_c` (4,834): task type 0 only. Start position, flag 0x0f, 10 zero bytes, f32 speed,
  count, waypoints, then a 16-byte tail. Tail is zero in most records; otherwise an ID32 (e.g.
  51321) plus an f32 (unverified). All 4,834 parsed.
- `CharInPlayIIR_t` (21): no body.
- `KnubotAnswerIIR` (71): u16 1, NPC ID32, u32 answer index. Index was 0 every time.

Counted but not parsed: `CoreProjectileIIR` (1,076), `BehaviorControlCenterIIR_c` (1,039),
`LookAtIIR_t` (742), `QuestCooldownIIR` (61), `CoreClaimIIR` (27), `QuestRewardSelectionIIR_c`
(16), `CharacterActionIIR_t` (2). Raw record hex is kept only for the last four in events.

## Playfields

Sources: the client's teleport RPCs and the ctrack analytics lines (`action=enter_splashscreen`,
`exit_splashscreen`, `enter_game`, value `51100:<id>`), which the client sends to 37.18.192.73:80.

Sequence (game time, seconds):

| t | Playfield | Evidence | Confidence |
|---|---|---|---|
| 28.1 | 4200 Isle of Yarr | exit_splashscreen (first arrival) | high |
| 44.8 / 53.6 | 1640 Natives' Hollow | TeleportMessageBoxAnswer 1640, TeleportMe 1640, enter_game | medium (no splash line) |
| 58.8 | 1200 LEGO Center | TeleportMe 1200 at 57.1, exit_splashscreen | high |
| 60.8-83.1 | 8060 Cursed Grove | 3 Endless answers, exit_splashscreen at 83.9 | high (first two answers not followed by an arrival line) |
| 111.7 | 8030 Monster Laboratory | Endless answer at 111.2, exit_splashscreen | high |
| 141.7 | unknown | enter_game with no request | unknown |
| 176.5 | 1200 LEGO Center | TeleportMe at 176.1, exit_splashscreen | high |
| 241.0 | loading (-1) | enter_splashscreen 51100:-1 | state unknown |
| 286.9 / 301.9 | 4600 requested, 4610 Jail arrived | TeleportWindowBypass(4600), enter_game, exit 4610 | medium: ids differ by 10 |
| 308.9 | loading 4200 | enter_splashscreen with no exit line | state unknown to session end |

Notes:
- The lead's login value 1200 is not the first playfield in this session: the first confirmed
  arrival is 4200 at 28.1 s, before any request. 1200 is reached by TeleportMe at 57 s and again
  at 176 s.
- Samples during loading and after an enter_game with no request are "unknown".
- Movement during the -1 loading period (241-301 s) and after 309 s by the second character
  (17478575) keeps moving, so the "loading" state is not reliable for that character. Those samples
  stay "unknown".

## Movement samples

6,343 samples (1,509 CharDCMove + 4,834 MoveTask start), unthinned:
`unknown` 3,023; `4200` 806; `1200` 601; `8060` 967; `8030` 698; `1640` 25; `4610` 222; `4600` 1.
`pf_confirmed` marks samples with an arrival line and no pending request.

Characters: 50000:16780472 (the one logged in, instance 0x01000cb8) and 50000:17478575
(instance 0x010ab3af). Identities 50000:1330698 and 50000:2466875 appear in samples on 8060/8030,
meaning unverified (not checked against any NPC data).

## Unknown or unverified

- Server replies are missing (zlib break), so no teleport outcome is confirmed by the server.
- Axis meaning (which position component is height) is unverified.
- CharDCMove rotation (8 bytes) is not written out.
- Endless answer value 1 and the meaning of the Endless destination are unverified.
- No ids were matched to dialogues.json, encounters.json or npc data (npc 587000 and tag 4221 are
  not found there).
- Playfield names used: data/playfields.json (4200 Isle of Yarr, 1200 LEGO Center, 1640
  Natives' Hollow, 8060 Cursed Grove, 8030 Monster Laboratory, 4600 Pirate World, 4610 Jail).
