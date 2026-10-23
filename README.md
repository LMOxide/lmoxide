# lmo-server

A server emulator, in Rust, for the LEGO Minifigures Online client (Funcom, shut down 2016).

**Status:** the real client patches against this server, logs in through the universe agent and
territory manager, requests character creation, and connects and authenticates to the game
server, and loads the starting playfield ("Entering playfield"). Spawning the player character
(creating its dynel) is the next piece of work — see docs/PROTOCOL.md "Dynel creation". See
[docs/PROTOCOL.md](docs/PROTOCOL.md) for everything learned so far.

## Layout

- `crates/lmo-net`: wire protocol (framing, RPC header, BinaryStream, protobufs, method tables)
- `crates/lmo-server`: the server binary (HTTP control/patch/track, universe, territory)
- `proto/`: protobuf schemas extracted from `LMO.exe`
- `data/rpc_methods.json`: every RPC method ID and C++ signature, extracted from `LMO.exe`
- `tools/re/`: reverse-engineering scripts; `tools/dev-run.sh`: run server and client together

## Running

You need an extracted client (`LEGO Minifigures Online/` including `RDB/`) and Wine. The
scripts assume it sits next to this repo at `../client/LEGO Minifigures Online`:

```sh
mkdir -p ../client && (cd ../client && unzip -q "../LEGO Minifigures Online.zip" && chmod -R u+w .)

# server only
cargo run -p lmo-server -- --client-dir "../client/LEGO Minifigures Online"

# server and client (separate Wine prefix in ~/.local/share/lmo-wine), auto-login:
LMO_CLIENT_ARGS="-username tester -password test" tools/dev-run.sh "$PWD/../client/LEGO Minifigures Online" /tmp
```

`dev-run.sh` writes the client's `LocalConfig.xml` from the server
(`http://127.0.0.1:8080/control/lmo_live/LocalConfig.xml`) so that the client talks to it.

Ports (all configurable, see `--help`): HTTP 8080, universe 7100, territory 7101.

## Tools

| Command | What it does |
|---|---|
| `tools/test-run.sh [secs]` | restart server + client (auto-login), wait, print a summary of RPCs, warnings and client progress, screenshot the game window to `logs/screen.png`; `LMO_AUTOPLAY=1` also presses "start" via Lua |
| `tools/dev-run.sh <client-dir> <log-dir>` | restart server + client and leave them running |
| `tools/re/py method.py <name\|0xID\|Iface::>` | RPC lookup: ID, signature, client dispatch/sender function, argument read order |
| `tools/re/py xref.py <regex>` | search exe strings, with referencing functions |
| `tools/re/py d.py <va> [bytes]` | disassemble with string annotations |
| `tools/re/py extract_rpc.py <out.json>` | regenerate `data/rpc_methods.json` |
| `tools/re/py extract_protos.py <exe> <dir>` | regenerate `proto/` |
| `tools/ghidra/analyze.sh` | one-time Ghidra import + analysis into `../ghidra/` |
| `tools/ghidra/decompile.sh <va...>` | decompiled C of the functions containing each address |
| `tools/ghidra/xrefs.sh <va...>` | references to each address (code and data, one level through vtables) |
| `tools/ghidra/symbols.sh <regex> [n]` | symbols matching a regex; for vtables, the first n slots |
| `tools/re/py extract_iir.py <out.json>` | regenerate `data/iir_types.json` (IIR names, type ids, classes, protobufs) |

Setup for the Python tools: `python3 -m venv .venv && .venv/bin/pip install pefile capstone protobuf`.
They read `../client/LEGO Minifigures Online/LMO.exe` by default, or `$LMO_EXE`. Ghidra comes
from `/opt/ghidra` (Arch `ghidra` package) or `$GHIDRA_HOME`.
