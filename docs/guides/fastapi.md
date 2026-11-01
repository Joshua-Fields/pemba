# FastAPI

> **Unreleased.** This helper is on `main` and planned for 0.3.0. It is not in 0.2.0, so
> `pip install pemba[fastapi]` and `pemba.contrib.fastapi` do not work with the current
> release. To try it now, [build the current checkout](installation.md).

`pemba.contrib.fastapi` checks a permission before a route runs. The route body runs only
if the check returned true. Everything else is an HTTP error.

```bash
pip install 'pemba[fastapi]'
```

`import pemba` does not need FastAPI. Importing `pemba.contrib.fastapi` without it raises an
`ImportError` that names the extra.

## Runnable example

This needs `httpx` as well, for FastAPI's test client:

```python
from fastapi import Depends, FastAPI
from fastapi.testclient import TestClient
from pemba import Engine
from pemba.contrib.fastapi import Authz

engine = Engine.memory()
engine.write_schema("""
definition user {}
definition document {
    relation viewer: user
    permission view = viewer
}
""")
engine.write(touch=["document:readme#viewer@user:anne"])

app = FastAPI()
# Use your own authentication here. A header is only good enough for this example.
authz = Authz(engine, subject=lambda request: f"user:{request.headers['x-user']}")


@app.get("/documents/{doc_id}")
def get_doc(doc_id: str, _=Depends(authz.require("document", "view", path_param="doc_id"))):
    return {"id": doc_id}


client = TestClient(app)
assert client.get("/documents/readme", headers={"x-user": "anne"}).status_code == 200
assert client.get("/documents/readme", headers={"x-user": "bob"}).status_code == 403
assert client.get("/documents/a:b", headers={"x-user": "anne"}).status_code == 422
```

It works the same on `async def` routes. FastAPI runs the dependency in its threadpool, so a
check never blocks the event loop, but it does take a thread from the pool that `def` routes
use. On SQLite, engine calls hold the GIL ([details](../06-api.md#22-signatures-ship-as-pemba__init__pyi)).

## What each outcome returns

| Outcome | Response | Route body runs |
|---|---|---|
| Allowed | The route's response | Yes |
| Denied | 403 `Forbidden`, or your `deny_status` and `deny_detail` | No |
| Store failure, evaluation limit, or any other `PembaError` | 503 `Authorization is temporarily unavailable` | No |
| An id the engine refuses (`*`, `#`, `@`, `:`, non-ASCII, over 1,024 characters) | 422 `Invalid resource id` | No |
| `subject` returned `None`, or a string ending in `:None` | 401 `Authentication required` | No |
| `subject` raised, or returned a malformed subject, a wildcard or a subject set | 500 | No |
| The route has no such `path_param`, or `at_least` returned a bad value | 500 | No |

A deny and an error are different answers: a store outage is never reported as a 403.

The 503 body never contains a store message, SQL or connection details. The cause goes to the
`pemba.contrib.fastapi` logger at `ERROR`, with the traceback, and is on the exception's
`__cause__`. Make sure your logging configuration keeps that logger.

## Configuring a deny

```python
authz = Authz(engine, subject=get_subject, deny_status=404, deny_detail="Not found")
```

This hides whether a document exists from callers who may not see it. `deny_status` must be a
client error, 400 to 499. These settings apply to a deny only: they cannot turn a deny into a
success or a redirect, and they do not change any other row of the table above.

## The subject

`subject` is a synchronous callable that takes the `Request` and returns the caller as
`type:id`, such as `user:anne`.

- There is no anonymous fallback. Return `None` for a request with no caller, and it gets a
  401 without a check.
- That 401 has no `WWW-Authenticate` header, because the helper does not know your scheme. To
  send one, raise your own exception from the callable:
  `raise HTTPException(401, headers={"WWW-Authenticate": "Bearer"})`. An `HTTPException` from
  the callable is passed through unchanged.
- Any other exception stays an exception, so the client gets a 500.
- `f"user:{user_id}"` with `user_id = None` gives the string `user:None`, which is a valid
  subject that a `user:*` grant would admit. The helper treats an id of exactly `None` as no
  caller, so an account whose id is that string cannot pass this helper.
- `user:*` and `group:eng#member` are refused: a request acts for one object.

## Consistency

A check reads the newest committed revision by default, which is what a permission gate
usually wants. See [consistency](consistency.md). To add a lower bound, pass a callable that
returns a revision for this request:

```python
def session_revision(request):
    return request.session.get("pemba_revision")  # None means no bound

gate = authz.require("document", "view", path_param="doc_id", at_least=session_revision)
```

If the store is behind the bound, the check fails and the client gets a 503. Parse any
client-supplied token in the callable, and raise your own `HTTPException` if it is invalid; a
value the engine refuses is a 500.

There is no `at_exactly`. A gate that answered from an older revision would let a caller back
in after their access was revoked.

## Limits

- Ids come from the route's path only, never the query string or a header. If the route has no
  such parameter, every request to it fails, so test each gated route.
- HTTP routes only. A dependency on a websocket route fails closed: the connection is refused.
- `subject` and `at_least` cannot be `async def`. Passing one raises `TypeError` when you
  create the `Authz` or call `require`. If your authentication is async, do it in a dependency
  or middleware that runs first, and read the result from `request.state`.
