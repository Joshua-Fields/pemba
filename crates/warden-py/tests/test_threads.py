"""``07`` §6: 8 threads checking while 1 thread writes; no errors, results monotone in
revision. Engine calls release the GIL, so the threads really run concurrently."""

from __future__ import annotations

import threading

import warden

DOCS = 200
CHECKERS = 8


def test_checkers_and_a_writer(engine: warden.Engine) -> None:
    written: dict[int, int] = {}  # doc index -> revision its viewer tuple was written at
    errors: list[BaseException] = []
    seen: list[list[tuple[int, int, bool]]] = [[] for _ in range(CHECKERS)]
    done = threading.Event()
    start = threading.Barrier(CHECKERS + 1)

    def writer() -> None:
        try:
            start.wait()
            for i in range(DOCS):
                written[i] = engine.write(touch=[f"document:d{i}#viewer@user:anne"])
        except BaseException as e:  # noqa: BLE001 - reported by the main thread
            errors.append(e)
        finally:
            done.set()

    def checker(n: int) -> None:
        try:
            start.wait()
            i = n
            while not done.is_set() or i < n + DOCS:
                doc = i % DOCS
                r = engine.check_detailed(f"document:d{doc}", "view", "user:anne")
                seen[n].append((doc, int(r.revision), r.allowed))
                i += 1
        except BaseException as e:  # noqa: BLE001
            errors.append(e)

    threads = [threading.Thread(target=writer)]
    threads += [threading.Thread(target=checker, args=(n,)) for n in range(CHECKERS)]
    for t in threads:
        t.start()
    for t in threads:
        t.join(timeout=120)
    assert not any(t.is_alive() for t in threads)
    assert errors == []
    assert len(written) == DOCS

    for results in seen:
        assert results
        revisions = [rev for _, rev, _ in results]
        assert revisions == sorted(revisions), "a thread saw head go backwards"
        for doc, rev, allowed in results:
            # Monotone in revision: allowed exactly from the revision the tuple appeared at.
            assert allowed == (rev >= written[doc]), (doc, rev, written[doc])
    assert any(not a for r in seen for _, _, a in r), "no check overlapped the writes"
