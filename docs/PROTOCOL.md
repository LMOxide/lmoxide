# LMO client protocol notes

Findings for the shipped client (`Version.txt`: `LMOv1.5@557666`, built 2016-05-23). LMO runs on
Funcom's Dreamworld/N3 engine (the Anarchy Online / Age of Conan / The Secret World lineage),
not on LEGO Universe's engine, so DLU and the LUDevNet docs don't apply.

Status legend: **verified** = observed working against the real client; **static** = read from
the binary only.

## Boot sequence

1. `Launcher.exe` (.NET) reads `ControlHttpPatchAddr` from `LocalConfig.xml`, downloads
   `<control>/lmo_live/LocalConfig.xml` and `PatchInfoClient.txt`, then runs `LMO.exe -verified`.
   We skip the launcher: write `LocalConfig.xml` into the client folder and run `LMO.exe -verified`
   directly (see `tools/dev-run.sh`). **verified**
2. The in-client patcher (`CorePatcher`) fetches `LocalConfig.xml` and `PatchInfoClient.txt` again,
   then walks: GetClientFileList → GetDownloadList → GetRDBHashIndex → PatchRDBHashIndex →
   PatchPacks → ProcessDownloadList → WaitingForSubstanceCaching → PatchingDone. **verified**
3. The login window is a CEF page loaded from `LoginURL`. The page calls `user.login(a, b)`
   (injected by `DWWebHelper.exe`). Alternatively pass `-username X -password Y` on the command
   line for automatic login (`charslot`, `charname` also exist). **verified (command line)**
4. Universe agent (`UniverseAddr`, TCP) → territory manager (PlayerAgent) → game server.

## HTTP patch layout

Relative to `HttpPatchAddr`:

| Path | Content |
|---|---|
| `rdb/full/<md5>` | content-addressed file named by `PatchInfoClient.txt` hashes (download lists, `RDBHashIndex.bin`) |
| `rdb/res/<h[0:2]>/<h[2]>/<h[3:]>` | one RDB resource; `h` = md5 of the *uncompressed* resource (also its local file name, `RDB/<type>/<HASH>.rdbres`) |
| `client/<h[0:2]>/<h[2:]>` | client files from `FileList.bin` (94 shader-cache files missing from the archive; failures are non-fatal) |

- Downloads may be compressed: bodies starting with magic `IOz1`, `IOz2` or `IOg1` are decompressed
  (zlib/LZMA); anything else is stored as-is (`0x48e830`). We serve raw files. **verified**
- `PatchInfoClient.txt` → `DownloadList-CLIENT-Windows32-SSE2DXT-modelQ0=<md5>` selects the
  platform download list. If the local `RDB/DownloadList-Platform.xml` has a different md5 the
  client downloads `rdb/full/<md5>`.
- The archived client lacks ~4,065 listed resources (~430 MB, mostly rdbtype 1010001/1010180).
  The server serves a filtered list. The client rejects it unless counters are consistent:
  per `<ResourceList>`, `ResourceCount` = entries, `DownloadSize` = Σ `size`, `SubstanceCount` =
  entries with rdbtype 1010061; on `<DownloadList>`, `ResourceCount`/`DownloadSize` count unique
  hashes. **verified**
- Funnel tracker: `GET <FunnelTrackerAddr>?category=..&platform=..&action=..`; any 200 is fine.

## TCP framing **verified**

```
frame   = u32be len, u32be crc32(body), body        ; len = 4 + body.len()
body    = u32be hlen, RpcHeader (protobuf, hlen bytes), args
```

`RpcHeader`/`ProtoEndpoint` are in `proto/Util.proto`. `endpoint_id.type` and `caller_id.type`
are a Java-style hash (`h = h*31 + c`) of the endpoint name (e.g. `UniverseInterface` →
`0x0c384054`). `caller_id.instance` carries an identity some handlers check.

## Method IDs

Extracted for every interface by `tools/re/extract_rpc.py` → `data/rpc_methods.json` → generated
constants `lmo_net::methods::<interface>::<METHOD>`. Most interfaces use 32-bit hashes; some use
declaration-order indices (`UniverseAgent`, `ClientToGameserverRPC`, `GameserverToClientRPC`).
Receivers (server→client) come from each `*Receiver::DispatchMethod` switch; senders
(client→server) from the index passed to `rpc::Proxy_t::SendMethodInvocation`.

## Argument encoding (BinaryStream) **verified**

Big-endian. `std::string`/`String`: u16 length + bytes (length ≥ 0x8000: `ffff 01ff` + u32).
`ID32`: u32 type, u32 instance. `ID32::Instance`: u32.

## Login flow

| Step | Call | Notes |
|---|---|---|
| 1 | → `UniverseAgent::InitiateAuthentication(username, gameID=1)` | **verified** |
| 2 | ← `UniverseInterface::Challenge(data)` | we send 32 `0`s; the client accepts any |
| 3 | → `UniverseAgent::AnswerChallenge(answer)` | 641 chars `"<hex>-<hex>"` (Diffie-Hellman); not verified by us |
| 4 | ← `UniverseInterface::AckAuthenticate(nAuthStatus, playerID, territoryAddr, cookie, eReason)` | handler `0xb4b580`: `nAuthStatus` 1 = OK; `eReason` must be 0 (non-zero is shown as a login problem, e.g. `e_NoGameServers`); **caller instance must equal `playerID.instance`** **verified** |
| 5 | → `PlayerAgent::Authenticate(account, cookie, territoryVersion=0)` on the territory address | **verified** |
| 6 | ← `PlayerInterface::AuthenticateAck(nStatus=1)` | client proceeds **verified** |
| 7 | → `PlayerAgent::GetStartupData(md5)` | **next**: probably answered with `UpdateClientPlayerData`, `SetDimensionList`, `SendCharacterSettingsCheckSumMap`, `PlayerSetupComplete`, … (needs `ClientPlayerData_t`, `FidData_c`, `DimensionDataVec_t` encodings) |

## Tooling

- `tools/re/pe.py`, `d.py <va> [bytes]` (disassemble with string annotations), `dispatch.py <string>`
  (find string xrefs and containing function), `vt.py <func>` (vtable containing a function).
  They need a Python venv with `pefile capstone` (and `protobuf` for `extract_protos.py`).
- `tools/re/extract_protos.py LMO.exe proto/` regenerates `proto/` from descriptors embedded in
  the exe.
- Receiver dispatch functions log `X::DispatchY() not possible, stream bad after '<arg>'`, which
  gives the argument names and read order; sender functions log their full C++ signature.
