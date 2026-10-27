"""Authorising an AI agent's tool calls with pemba.

An agent never holds more authority than the person it acts for. Gate 1 filters
the tool list (MCP ``tools/list``) to the tools the agent was granted; gate 2
checks every call for delegation, tool grant and the person's own access to the
resource, all at one revision. Revoking an agent commits in the application's
own SQLite transaction.

    pip install --pre pemba
    python agent_authz.py

Walkthrough: https://joshuafields.uk/blog/authorizing-ai-agents-mcp-tool-calls-zanzibar
CI runs this file (crates/pemba-py/tests/test_examples.py), without the timing loop.
"""
import os
import sqlite3
import statistics
import time

from pemba import Engine

SCHEMA = """
definition user {}

definition group {
    relation member: user | group#member
}

// An agent is software. It holds no authority of its own:
// it acts for the people who delegated to it.
definition agent {
    relation principal: user
    permission acts_for = principal
}

// Gate 1: which tools this agent is allowed to see and call at all.
definition tool {
    relation granted: agent
    permission invoke = granted
}

definition document {
    relation owner: user
    relation viewer: user | group#member
    permission view = viewer + owner
}
"""

DB = "agents.db"
if os.path.exists(DB):
    os.remove(DB)

engine = Engine.sqlite(DB)
engine.write_schema(SCHEMA)

engine.write(touch=[
    "group:finance#member@user:bea",
    "document:payroll#viewer@group:finance#member",
    "document:roadmap#viewer@user:anne",
    "document:roadmap#viewer@user:bea",
    # Anne delegates to her triage agent, and grants it two tools.
    "agent:triage#principal@user:anne",
    "tool:read_document#granted@agent:triage",
    "tool:search_documents#granted@agent:triage",
])


def visible_tools(agent: str) -> list[str]:
    """Gate 1, capability admission: the tools/list this agent gets."""
    return engine.lookup_resources("tool", "invoke", agent).ids


def authorize(agent: str, principal: str, tool: str, resource: str, permission: str) -> bool:
    """Gate 2, execution authorisation: every check at one revision."""
    results = engine.check_bulk([
        (agent, "acts_for", principal),    # is this agent acting for this person?
        (tool, "invoke", agent),           # was it granted this tool?
        (resource, permission, principal), # can the *person* do this to this resource?
    ])
    # check_bulk returns a PembaError in place of a bool on failure: fail closed.
    return all(r is True for r in results)


print("tools/list for agent:triage:", visible_tools("agent:triage"))

cases = [
    ("read roadmap for anne", "user:anne", "tool:read_document", "document:roadmap"),
    ("read payroll for anne", "user:anne", "tool:read_document", "document:payroll"),
    ("read payroll for bea ", "user:bea", "tool:read_document", "document:payroll"),
    ("send_email for anne  ", "user:anne", "tool:send_email", "document:roadmap"),
]
for label, who, tool, res in cases:
    print(f"{label}: {authorize('agent:triage', who, tool, res, 'view')}")

# Revocation joins the app's own transaction: the session row and the
# delegation commit together, so there is no window where one is gone and
# the other is not.
conn = sqlite3.connect(DB, isolation_level=None)
conn.execute("CREATE TABLE agent_sessions (agent TEXT, principal TEXT, revoked_at TEXT)")
conn.execute("INSERT INTO agent_sessions VALUES ('triage', 'anne', NULL)")
conn.execute("BEGIN IMMEDIATE")
conn.execute("UPDATE agent_sessions SET revoked_at = datetime('now') WHERE agent = 'triage'")
pending = engine.write(delete=["agent:triage#principal@user:anne"], conn=conn)
conn.execute("COMMIT")
rev = pending.revision()
conn.close()
print("after revoke, read roadmap for anne:",
      authorize("agent:triage", "user:anne", "tool:read_document", "document:roadmap", "view"),
      f"(revision {rev})")

if __name__ == "__main__":
    # Put the delegation back, then time one tool-call authorisation.
    engine.write(touch=["agent:triage#principal@user:anne"])
    args = ("agent:triage", "user:anne", "tool:read_document", "document:roadmap", "view")
    for _ in range(2_000):
        authorize(*args)
    samples: list[float] = []
    for _ in range(20_000):
        t = time.perf_counter_ns()
        authorize(*args)
        samples.append((time.perf_counter_ns() - t) / 1_000)
    samples.sort()
    p50 = statistics.median(samples)
    p99 = samples[int(len(samples) * 0.99)]
    print(f"authorize(): p50 {p50:.1f} us, p99 {p99:.1f} us over {len(samples)} calls")
    print(f"50 tool calls: {50 * p50 / 1000:.2f} ms of authorisation at p50")
