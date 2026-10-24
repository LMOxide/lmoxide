#!/usr/bin/env python3
"""Keep the game server's ports listening across restarts.

    tools/keep-listening.py --bind 0.0.0.0 --ports 7100,7101,7102,7200 --pidfile F -- CMD ARGS...

Binds the ports once and runs CMD with the listening sockets inherited (LMOXIDE_LISTEN_FDS =
"port:fd,..."; lmoxide-server takes them over). When CMD exits (a restart: kill it, a crash, a
new build), it is started again with the same sockets, so a client reconnecting meanwhile (the
client's "Lost connection ... relocate?" Yes tries once) waits in the backlog instead of being
refused, and the new server answers it. SIGINT/SIGTERM to this process stop CMD and end it.
The pid of the running CMD is written to --pidfile (tools/lan-server.sh --restart kills it).
"""
import argparse, os, signal, socket, subprocess, sys, time

p = argparse.ArgumentParser()
p.add_argument("--bind", default="0.0.0.0")
p.add_argument("--ports", required=True)
p.add_argument("--pidfile")
p.add_argument("--holder-pidfile", help="where to write this process's pid")
p.add_argument("cmd", nargs=argparse.REMAINDER)
a = p.parse_args()
cmd = a.cmd[1:] if a.cmd[:1] == ["--"] else a.cmd

if a.holder_pidfile:
    with open(a.holder_pidfile, "w") as f:
        f.write(f"{os.getpid()}\n")

socks = []
for port in (int(x) for x in a.ports.split(",")):
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind((a.bind, port))
    s.listen(128)
    s.set_inheritable(True)
    socks.append((port, s))
env = dict(os.environ, LMOXIDE_LISTEN_FDS=",".join(f"{port}:{s.fileno()}" for port, s in socks))

stopping = False
child = None


def stop(signum, _frame):
    global stopping
    stopping = True
    if child and child.poll() is None:
        child.send_signal(signal.SIGTERM)


signal.signal(signal.SIGINT, stop)
signal.signal(signal.SIGTERM, stop)

while not stopping:
    started = time.monotonic()
    child = subprocess.Popen(cmd, env=env, pass_fds=[s.fileno() for _, s in socks])
    if a.pidfile:
        with open(a.pidfile, "w") as f:
            f.write(f"{child.pid}\n")
    code = child.wait()
    if stopping:
        break
    # a server that dies right away (bad arguments, locked database) isn't restarted in a tight loop
    pause = 1 if time.monotonic() - started > 10 else 5
    print(f"keep-listening: server exited ({code}); starting it again in {pause} s", file=sys.stderr, flush=True)
    time.sleep(pause)
for f in (a.pidfile, a.holder_pidfile):
    if f:
        try:
            os.remove(f)
        except FileNotFoundError:
            pass
