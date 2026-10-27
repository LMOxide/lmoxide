# Live capture: playfield objects (LMOp1-lmo-exe.cap)

Decoder: `tools/re/live_playfield.py` (`python3 tools/re/live_playfield.py <capture>`).
Output: `playfield-objects.json` in this folder. Source flow: server to client
`GameserverToClientRPC::PassBlob` on 37.18.193.16:7112 (the zlib game stream). Those messages have
no capture frame or timestamp in livecap, so `frame` and `ts_raw` are null. The `rpc` field is the
PassBlob's ordinal in capture order.

## What the blobs are

A PassBlob is a u32 BE length followed by IIR records: u32 type id, ID32 identity, bool pass-on,
then class data (docs/PROTOCOL.md, "IIRs").

| PassBlob (args bytes) | Record | Content |
|---|---|---|
| 106 | PlayfieldFIIR_t | arrival position (242.46, 19.0, 238.40); PlayfieldProxyData: model_id (51100, 1643), running_instance 0x01610550 |
| 11790 | FullPlayerCharacterIIR | the player's own character 50000:17478575: header protobuf (27 character tags, 5 inventory items, 2 quests). Class data not decoded. No NPC or smashable list in it |
| 426, 420, 804, 464-468 x 12 (15 total) | UpdateMessage op 0 | creates a dw::GameObject: 2 doors (type 51016), the player's own GameObject (50000), 12 destructibles (51321) |
| 37, 84, 25, 25, 25, 37, 37 | GameTimeIIR_t, MagicBuffIIR_c, StatIIR_t x3, StatUpdateIIR_c x2 | player state (not world content); type only |

UpdateMessage layout: u8 op, ID32 object, u32 playfield key (BE), u32 BE length + blob. The blob is
little-endian DreamWorld reflection data: u32 length, then (u32 field id, value) pairs. Field ids
are ELF hashes of the field names: Id (ID32), Position (x,y,z f32), Rotation (x,y,z,w), Scale
(x,y,z). After Scale come protobuf-style strings: `0x12 len name` (dev name, e.g. "XX_Banana Crate")
and `0x1a len xml` (`<remoteformat id=...>` for destructibles, template id; `<localized id=...>`
for doors, GUID). Positions are little-endian floats, not big-endian as first suspected.

## Playfield

Monkey Cave, id 1643 (data/playfields.json). Confidence: high, for the playfield of this game
session. Evidence: PlayfieldFIIR model_id = (51100, 1643); running_instance 0x01610550 equals the
playfield key of all 15 UpdateMessages and sits inside the character record; the player's
GameObject position equals the PlayfieldFIIR arrival position; door 33183 at (241.79, 20.48,
231.73) is at the same spot as the 1643 -> 4200 portal in data/destinations.json.
Caveat: the LoginConnectReady for the same character (17478575, frame 10910) names playfield 4000,
while the in-session PlayfieldFIIR names 1643. I trust the in-session record. The login
PlayfieldProxy semantics are not established.

## Counts

- 24 PassBlobs; 15 UpdateMessages decoded (2 door, 1 character, 12 destructible, 0 buildie).
- 16 PassBlobs decoded exactly to their length. The other 8 are single records of unknown size.
- Cross-check against data/world_content.json (1643) and buildies / return_points / destinations:
  - 2 doors: match world_content doors by GUID (594988, 634358) and position (0.01 m).
  - 1 destructible (XX_Small chest, template 149028): same template as a proposed smashable, but
    14.6 m away. A different instance.
  - 12 new (no existing entry): 10 XX_Banana Crate (template 197959), 1 XX_Tiki Statue (194057),
    and the player's own GameObject. The templates 197959 and 194057 do not occur anywhere in
    data/.
  - Position-only matches are restricted to the same kind (a crate is not an enemy).
- Login records (37.18.192.52:7101): ChatServerConnectReady (chat.lmo_live.funcom.com:7001) and
  LoginConnectReady (gs007-nj4.lego.funcom.com:7102 at frame 201, 7103 at frame 10910),
  OverrideGameTweaks (empty), SendCharacterSettings (character 17478575, empty).

## Not decoded or unknown

- The field after Scale, id 0x076023f3 (u32, value 1 in every record). Static guess: a flag.
  The reflection data after it is not decoded.
- Destructible look (monster_data) and CARS group: they come in the full-update IIR, which is not
  in this capture. Only the template is known.
- Instance and identity semantics beyond "the dynel's id". Records with identity (0,0) are the
  UpdateMessage wrapper; the object's ID32 is in the body.
- Player character class data (stats, positions, costume): not decoded.
- The playfield key 0x01610550 is the runtime key of the client's playfield map, not the ID32.
- Only 15 world objects survived; the zlib stream breaks early, so the playfield's other dynels
  (NPCs, enemies, doors, simple dynels) are not in the capture.
