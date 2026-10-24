# Recorded-session parsers

The client ships the original server's traffic from 2015 in
`client/LEGO Minifigures Online/Data/default_windowsx86.demo`.

    mkdir -p /tmp/demo && python3 tl.py /tmp/demo && python3 recs.py /tmp/demo

- `tl.py` → `tl.pkl`: every RPC in both directions (dicts: i, ms, dir, iface, meth, args, inst).
  `ms` is only the microsecond part of the event time.
- `recs.py` → `recs.pkl`: every IIR record inside PassBlobs (dicts: i, dir, name, id, body).
- `pblib.py`: protobuf/demo event helpers.
