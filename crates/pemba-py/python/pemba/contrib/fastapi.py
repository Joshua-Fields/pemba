"""A permission check for FastAPI routes (``docs/06-api.md`` §2.4, OQ-067).

The route body runs only after ``Engine.check`` returned ``True``. Every other outcome is an
HTTP error: 403 (or the configured client error) on deny, 503 only for a store failure that a
retry can clear (``_TRANSIENT_CODES``), 422 for an id the engine refuses, 401 for no subject,
and a 500 for everything else: an unknown name, an evaluation limit, a bad bound or a
programming mistake. Retrying cannot fix those, so they must not look like an outage.
Configuration reaches the deny response and the headers of the 401 only.

The dependency is a plain ``def``, so FastAPI runs the subject callable and the check in its
threadpool, on ``def`` and ``async def`` routes alike. ``pemba.aio`` does not fit: it covers
the check but not the subject callable, and it needs asyncio where FastAPI also runs on trio.
"""

from __future__ import annotations

import inspect
import logging
from collections.abc import Callable, Mapping
from typing import Any

try:
    from fastapi import HTTPException, Request
except ModuleNotFoundError as exc:
    if exc.name != "fastapi":
        # FastAPI is installed and something it needs is not: the extra would change nothing.
        raise
    raise ImportError(
        "pemba.contrib.fastapi needs FastAPI; install it with: pip install 'pemba[fastapi]'"
    ) from exc

from pemba import Engine, PembaError, PendingRevision, Relationship

__all__ = ["Authz"]

_log = logging.getLogger(__name__)

_UNAVAILABLE = "Authorization is temporarily unavailable"
_BAD_ID = "Invalid resource id"
_NO_SUBJECT = "Authentication required"
_SERVER_ERROR = "Internal Server Error"

# An allowlist, so a code added later is a 500 until someone decides it is transient. Python
# sees one code for every store failure, including the few a retry cannot clear (`Corrupt`).
_TRANSIENT_CODES = frozenset({"STORE_ERROR", "TRANSACTION_CONFLICT"})


def _refuse_coroutine_function(name: str, value: object) -> None:
    if inspect.iscoroutinefunction(value):
        raise TypeError(f"{name} must be a synchronous callable; it runs in FastAPI's threadpool")


def _close_coroutine(value: object) -> None:
    """A callable object can still return a coroutine. Nothing will await it."""
    if inspect.iscoroutine(value):
        value.close()


def _parse_subject(value: object) -> str | None:
    """The subject to check, or ``None`` if there is none. A malformed one raises.

    Parsed on its own, because the engine raises ``ValueError`` for a bad subject and a bad
    resource alike, and only the resource is the client's fault.
    """
    if value is None:
        return None
    if not isinstance(value, str):
        _close_coroutine(value)
        raise TypeError(f"subject must return a str or None, not {type(value).__name__}")
    # f"user:{''}" has no id to parse, so the parser would call it malformed: fill one in to
    # check the rest, which keeps ":" and "a:b:" malformed.
    no_id = value.endswith(":")
    try:
        parsed = Relationship.parse(f"pemba_contrib:x#rel@{value}{'x' if no_id else ''}")
    except ValueError as exc:
        raise ValueError("subject returned a malformed subject") from exc
    if parsed.subject_id == "*" or parsed.subject_relation is not None:
        # A request acts for one object: `user:*` is not a caller, and neither is a set.
        raise ValueError("subject must return a plain object such as user:anne")
    if no_id or parsed.subject_id == "None":
        # f"user:{None}" parses, and a `user:*` grant would admit it (OQ-067). Other strings
        # are not blocklisted: return None for no caller, as `06` §2.4 shows.
        return None
    return value


def _valid_resource(resource_type: str, resource_id: str) -> bool:
    """Whether the engine's parser reads ``resource_id`` as exactly that id of that type."""
    try:
        parsed = Relationship.parse(f"{resource_type}:{resource_id}#rel@user:x")
    except ValueError:
        return False
    return parsed.resource_type == resource_type and parsed.resource_id == resource_id


class Authz:
    """Builds route dependencies that check one permission against ``engine``.

    ``subject`` takes the ``Request`` and returns the caller, such as ``"user:anne"``, or
    ``None`` for no caller (401). It must be synchronous. An exception it raises stays an
    exception, so raise ``HTTPException(401, headers=...)`` to send your own challenge.

    A deny gets ``deny_status``, a client error (400 to 499), and ``deny_detail``.

    ``unauthenticated_headers`` go on the helper's own 401. RFC 9110 requires that response to
    carry ``WWW-Authenticate``, and the scheme is the application's, so set it here.
    """

    def __init__(
        self,
        engine: Engine,
        *,
        subject: Callable[[Request], str | None],
        deny_status: int = 403,
        deny_detail: str = "Forbidden",
        unauthenticated_headers: Mapping[str, str] | None = None,
    ) -> None:
        if not callable(subject):
            raise TypeError("subject must be a callable taking the Request")
        _refuse_coroutine_function("subject", subject)
        if isinstance(deny_status, bool) or not isinstance(deny_status, int):
            raise TypeError("deny_status must be an int")
        if not 400 <= deny_status <= 499:
            raise ValueError("deny_status must be a client error, 400 to 499")
        if not isinstance(deny_detail, str):
            raise TypeError("deny_detail must be a str")
        if unauthenticated_headers is not None and not (
            isinstance(unauthenticated_headers, Mapping)
            and all(isinstance(part, str) for item in unauthenticated_headers.items() for part in item)
        ):
            raise TypeError("unauthenticated_headers must map str to str")
        self._unauthenticated_headers = dict(unauthenticated_headers or {})
        self._engine = engine
        self._subject = subject
        self._deny_status = deny_status
        self._deny_detail = deny_detail

    def require(
        self,
        resource_type: str,
        permission: str,
        *,
        path_param: str,
        at_least: Callable[[Request], int | PendingRevision | None] | None = None,
    ) -> Callable[[Request], None]:
        """A dependency for ``Depends``: ``permission`` on ``resource_type:<path_param>``.

        ``path_param`` is read from the route's path and nowhere else. A route without it
        fails on every request. The check runs before FastAPI validates the route's parameters
        and sees the raw value, so a route that changes it (a normalising validator, a coercion
        to ``int`` or ``UUID``) can read a different object than the one checked: declare it
        as a plain ``str`` or a path convertor. ``at_least``, if given, takes the ``Request``
        and returns a revision bound for this check; the default is the newest committed
        revision. There is no ``at_exactly``: a permission gate must not answer from before a
        revocation.
        """
        if not isinstance(resource_type, str) or not isinstance(permission, str):
            raise TypeError("resource_type and permission must be str")
        if not isinstance(path_param, str):
            raise TypeError("path_param must be a str")
        if not path_param:
            raise ValueError("path_param must name a parameter of the route's path")
        if at_least is not None and not callable(at_least):
            raise TypeError("at_least must be a callable taking the Request, or None")
        _refuse_coroutine_function("at_least", at_least)
        parsed = Relationship.parse(f"{resource_type}:x#{permission}@user:x")
        if parsed.resource_type != resource_type or parsed.relation != permission:
            raise ValueError(f"not a type and permission name: {resource_type!r}, {permission!r}")

        def dependency(request: Request) -> None:
            self._authorize(request, resource_type, permission, path_param, at_least)

        return dependency

    def _authorize(
        self,
        request: Request,
        resource_type: str,
        permission: str,
        path_param: str,
        at_least: Callable[[Request], int | PendingRevision | None] | None,
    ) -> None:
        if path_param not in request.path_params:
            raise LookupError(
                f"path_param {path_param!r} is not in this route's path; "
                f"it has {sorted(request.path_params)}"
            )
        # A convertor such as {id:int} may have made it something other than a str.
        resource_id = str(request.path_params[path_param])

        # Before the id, so that a caller with no identity learns nothing about ids.
        subject = _parse_subject(self._subject(request))
        if subject is None:
            raise HTTPException(
                status_code=401,
                detail=_NO_SUBJECT,
                headers=dict(self._unauthenticated_headers) or None,
            )
        if not _valid_resource(resource_type, resource_id):
            raise HTTPException(status_code=422, detail=_BAD_ID)

        bound: Any = None if at_least is None else at_least(request)
        if isinstance(bound, bool) or not isinstance(bound, (int, PendingRevision, type(None))):
            _close_coroutine(bound)
            raise TypeError(f"at_least must return a revision or None, not {type(bound).__name__}")

        try:
            # Three arguments, never the tuple string: an id must not be able to supply a
            # subject. A ValueError here is not the client's: both parts were checked above.
            allowed = self._engine.check(
                f"{resource_type}:{resource_id}", permission, subject, at_least=bound
            )
        except PembaError as exc:
            if exc.code == "REVISION_NOT_YET_AVAILABLE":
                # A client can send any bound, so one must not write a traceback per request:
                # raised unchanged, the server would log it at ERROR (OQ-067).
                _log.info("pemba check got an at_least bound ahead of the store, type %s", resource_type)
                raise HTTPException(status_code=500, detail=_SERVER_ERROR) from exc
            if exc.code not in _TRANSIENT_CODES:
                raise
            # FastAPI does not log an HTTPException, so `from` alone would lose the cause.
            _log.error(
                "pemba check failed with %s for %s on type %s",
                exc.code, permission, resource_type, exc_info=exc,
            )
            raise HTTPException(status_code=503, detail=_UNAVAILABLE) from exc
        if allowed is not True:
            raise HTTPException(status_code=self._deny_status, detail=self._deny_detail)
