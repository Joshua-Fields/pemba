"""Print one compact line per scheme for the given results files (SERIALIZABLE retry runs)."""
import json, sys
for f in sys.argv[1:]:
    for r in json.load(open(f)):
        if "scheme" not in r:
            continue
        rt = r.get("retry") or {}
        affected = rt.get("txns_needing_retry", 0) + rt.get("gave_up", 0)
        total = r["committed_txns"] + rt.get("gave_up", 0)
        print(f"{f[:-5]:42} {r['scheme']:9} iso={r['config']['isolation'][:4]} long={r['config']['long_every_s']}"
              f" tx={r['committed_txns']} wps={r['writes_per_s']} viol={r['violations']} regr={r['head_regressions']}"
              f" dang={r['dangling_provisional_rows']} 40001_attempts={r['errors'].get('40001', 0)}"
              f" affected={affected}/{total} ({100 * affected / max(total, 1):.2f}%) gave_up={rt.get('gave_up')}"
              f" lag_p99={r['visibility_lag_ms']['p99']} blk_p99={r['writer_blocked_ms']['p99']} blk_max={r['writer_blocked_ms']['max']}")
