# lmo-server

A server emulator, in Rust, for the LEGO Minifigures Online client (Funcom, shut down 2016).

**Status:** the real client patches against this server, logs in through the universe agent,
and authenticates with the territory manager. Character list/creation is next. See
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

## Reverse-engineering tools

```sh
python3 -m venv .venv && .venv/bin/pip install pefile capstone protobuf
cd tools/re && ../../.venv/bin/python extract_rpc.py ../../data/rpc_methods.json
```

The scripts read `../client/LEGO Minifigures Online/LMO.exe` by default, or `$LMO_EXE`.
