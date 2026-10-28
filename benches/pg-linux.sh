#!/usr/bin/env bash
# The Rust benches' Postgres group at one or more commits, on one machine, for
# benches/results.py's "Postgres" section (T5.12, OQ-056). Run by
# .github/workflows/bench-postgres.yml; locally on Linux it needs PEMBA_PG_URL.
#
# Commits come from benches/pg-compare-refs, one `<label> <commit>` per line (`#` starts a
# comment), benched in that order; without the file, `head HEAD`. Output, under
# target/bench-pg-linux/ (or $1):
#   order                      the labels, in order
#   machine.txt                CPU, memory, kernel, toolchain, server round-trip time
#   <label>/commit             the commit benched
#   <label>/bench-latency/     per-call durations, as target/bench-latency/
#   <label>/bench-round-trips/ round trips per operation
#   <label>/bench-postgres-version.txt
set -euo pipefail

repo=$(git rev-parse --show-toplevel)
cd "$repo"
out=$(realpath -m "${1:-target/bench-pg-linux}")
: "${PEMBA_PG_URL:?set PEMBA_PG_URL}"
export PEMBA_BENCH_STORES=postgres

if [ -f benches/pg-compare-refs ]; then
  refs=$(grep -v '^[[:space:]]*\(#\|$\)' benches/pg-compare-refs)
else
  refs="head HEAD"
fi
# Resolve every commit before checking any out: the list lives in the starting commit.
resolved=""
while read -r label ref; do
  resolved+="$label $(git rev-parse --verify "$ref^{commit}")"$'\n'
done <<< "$refs"
start=$(git rev-parse HEAD)

rm -rf "$out"
mkdir -p "$out"
{
  echo "CPU: $(lscpu | sed -n 's/^Model name:[[:space:]]*//p'), $(nproc) logical CPUs"
  echo "Memory: $(free -g | awk '/^Mem:/ {print $2}') GiB"
  echo "OS: $(uname -srmo)"
  echo "rustc: $(rustc -V)"
  # The network the figures are bound by: one simple-query round trip to the server.
  echo "Server round trip (SELECT 1, p50 of 2000): $(python3 - <<'EOF'
import os, socket, struct, time, urllib.parse
u = urllib.parse.urlparse(os.environ["PEMBA_PG_URL"])
s = socket.create_connection((u.hostname or "localhost", u.port or 5432))
s.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
params = b"user\0" + (u.username or "postgres").encode() + b"\0database\0" + (u.path.lstrip("/") or "postgres").encode() + b"\0\0"
s.sendall(struct.pack("!ii", 8 + len(params), 196608) + params)
def until_ready():
    buf = b""
    while True:
        buf += s.recv(65536)
        while len(buf) >= 5:
            n = struct.unpack("!i", buf[1:5])[0]
            if len(buf) < 1 + n:
                break
            tag, buf = buf[0:1], buf[1 + n:]
            if tag == b"Z":
                return
until_ready()
q = b"SELECT 1\0"
msg = b"Q" + struct.pack("!i", 4 + len(q)) + q
ts = []
for _ in range(2000):
    t = time.perf_counter_ns()
    s.sendall(msg)
    until_ready()
    ts.append(time.perf_counter_ns() - t)
ts.sort()
print(f"{ts[len(ts) // 2] / 1000:.0f} µs")
EOF
)"
} > "$out/machine.txt"

while read -r label sha; do
  [ -n "$label" ] || continue
  echo "== $label ($sha)"
  git checkout -q --detach "$sha"
  rm -rf target/bench-latency target/bench-round-trips target/bench-postgres-version.txt
  mkdir -p "$out/$label"
  cargo bench -p pemba-benches --bench engine > "$out/$label/bench.log" 2>&1 \
    || { tail -50 "$out/$label/bench.log"; exit 1; }
  cp -r target/bench-latency target/bench-round-trips target/bench-postgres-version.txt \
    "$out/$label/"
  echo "$sha" > "$out/$label/commit"
  echo "$label" >> "$out/order"
done <<< "$resolved"
git checkout -q --detach "$start"
