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
    relation viewer: user | user:*
    permission view = viewer
}
""")
engine.write(touch=["document:readme#viewer@user:anne", "document:public#viewer@user:*"])

app = FastAPI()


def current_subject(request):
    # Use your own authentication here. A header is only good enough for this example.
    user_id = request.headers.get("x-user")
    return None if user_id is None else f"user:{user_id}"


authz = Authz(
    engine,
    subject=current_subject,
    unauthenticated_headers={"WWW-Authenticate": "Bearer"},
)


@app.get("/documents/{doc_id}")
def get_doc(doc_id: str, _=Depends(authz.require("document", "view", path_param="doc_id"))):
    return {"id": doc_id}


client = TestClient(app)
assert client.get("/documents/readme", headers={"x-user": "anne"}).status_code == 200
assert client.get("/documents/readme", headers={"x-user": "bob"}).status_code == 403
assert client.get("/documents/a:b", headers={"x-user": "anne"}).status_code == 422
assert client.get("/documents/public", headers={"x-user": "bob"}).status_code == 200
# No caller gets a 401, even on a document that every user can read.
anonymous = client.get("/documents/public")
assert anonymous.status_code == 401
assert anonymous.headers["WWW-Authenticate"] == "Bearer"
```

It works the same on `async def` routes. FastAPI runs the dependency in its threadpool, so a
check never blocks the event loop, but it does take a thread from the pool that `def` routes
use. On SQLite, engine calls hold the GIL ([details](../06-api.md#22-signatures-ship-as-pemba__init__pyi)).

## What each outcome returns

| Outcome | Response | Route body runs |
|---|---|---|
| Allowed | The route's response | Yes |
| Denied | 403 `Forbidden`, or your `deny_status` and `deny_detail` | No |
| Store failure or transaction conflict, which a retry can clear | 503 `Authorization is temporarily unavailable` | No |
| Any other engine error: unknown type or permission, no schema, an evaluation limit, a revision bound ahead of the store, a database not migrated | 500 | No |
| An id the engine refuses (`*`, `#`, `@`, `:`, non-ASCII, over 1,024 characters) | 422 `Invalid resource id` | No |
| `subject` returned `None`, or a string with an empty id or ending in `:None` | 401 `Authentication required` | No |
| `subject` raised, or returned a malformed subject, a wildcard or a subject set | 500 | No |
| The route has no such `path_param`, or `at_least` returned a bad value | 500 | No |

A deny and an error are different answers: a store outage is never reported as a 403. A 503
means try again, so a misspelled permission or an evaluation limit, which stay broken until
someone changes the code or the data, are 500s. Test each gated route to catch those early.

The body of an error never contains a store message, SQL or connection details. The cause of a
503 goes to the `pemba.contrib.fastapi` logger at `ERROR`, with the traceback, and is on the
exception's `__cause__`. Make sure your logging configuration keeps that logger. Other engine
errors reach your server's own error handling unchanged.

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
- RFC 9110 requires a 401 to carry a `WWW-Authenticate` header. The helper does not know your
  scheme, so it sends one only if you set it:
  `Authz(engine, subject=..., unauthenticated_headers={"WWW-Authenticate": "Bearer"})`.
- An `HTTPException` from the callable is passed through unchanged, for a challenge that varies
  by request. Any other exception stays an exception, so the client gets a 500.
- Do not format a missing value. `f"user:{user_id}"` with `user_id = None` gives `user:None`,
  a valid subject that a `user:*` grant would admit. The helper treats an empty id or an id of
  exactly `None` as no caller, so an account whose id is `None` cannot pass this helper. It does
  not catch other strings such as `user:null` or `user:undefined`: to the engine those are
  ordinary ids, and a `user:*` grant would admit them. Check for a missing user and return
  `None`, as in the example above.
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

If the store is behind the bound, the check fails and the client gets a 500, with one line at
`INFO` on the `pemba.contrib.fastapi` logger and no traceback. A client that sends its own
token can cause that 500 at will, so keep tokens server-side, as here, or sign them. Parse any
client-supplied token in the callable, and raise your own `HTTPException` if it is invalid; a
value the engine refuses is a 500.

There is no `at_exactly`. A gate that answered from an older revision would let a caller back
in after their access was revoked.

## Keep the checked id and the route's id the same

The check runs before FastAPI validates the route's parameters, and it reads the raw value from
the path. If the route then changes that value, the check and the body can name different
objects:

- A validator that normalises the id is a security bug. If it lowercases, a caller allowed on
  `document:Alice` passes the check on `/documents/Alice`, and the body loads `alice`.
- `doc_id: int` on `/documents/007` checks `document:007`, and the body reads `7`.
  `/documents/abc` gets the check's answer (401 or 403) rather than a 422.
- `doc_id: UUID` with an uppercase UUID checks that exact text, so it gets a 403 if your
  relationships store lowercase.

Declare a gated parameter as a plain `str` and use it as given. For an integer id, use a path
convertor such as `/documents/{doc_id:int}`, which runs before the check, so both see `7`.

## Limits

- Ids come from the route's path only, never the query string or a header. If the route has no
  such parameter, every request to it fails, so test each gated route.
- HTTP routes only. A dependency on a websocket route fails closed: the connection is refused.
- `subject` and `at_least` cannot be `async def`. Passing one raises `TypeError` when you
  create the `Authz` or call `require`. If your authentication is async, do it in a dependency
  or middleware that runs first, and read the result from `request.state`.
