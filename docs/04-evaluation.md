# 04 — Evaluation: check, lookup_resources, lookup_subjects

Crate: `pemba-core::eval`. Inputs: a `CompiledSchema` (`docs/03` §6), a `ReadStore`
(`docs/05` §2) and a resolved `Revision`.

Every algorithm here must agree with the reference evaluator (`docs/07` §2) on every input.

---

## 1. Request lifecycle (all read APIs)

1. Validate the request against the schema (types/relations exist, subject type valid).
   Invalid → `Err(InvalidRequest{..})` — never evaluate a malformed request.
2. Take one store snapshot (`ReadStore::snapshot()`, `docs/05` §2 "Revision checks"),
   resolve `Consistency` → one `Revision` against it (`docs/02` §4), and make every read of
   the request through that snapshot at that revision. `check_bulk` uses one snapshot for
   the whole call. The snapshot is released when the request returns, on every path.
3. Create a fresh `EvalContext { rev, memo, stack, depth, stats }`. Nothing is shared
   between requests except the immutable schema and the store.
4. Evaluate. Return `{ result, revision, stats }`.

**What `Exact(r)` means (OQ-030).** A read at `Exact(r)` answers with **the current
schema applied to the data as of `r`**, not "what was allowed at `r`". Schema history is
not replayed. Example: at revision 10 the schema has `permission view = viewer - banned`
and `doc:d#banned@user:eve` is live, so eve cannot view `doc:d`. At revision 20 `banned`
is removed from the schema (after its tuples are deleted, `03` §10). `check(doc:d, view,
user:eve)` at `Exact(10)` now evaluates `view = viewer`, never reads `banned`, and answers
`true` if eve was a viewer at 10, although at revision 10 she was denied. `Exact` is for
consistent paging and read-after-write, **not an audit log**; keep one in the application
if "who could do what, when" matters. Stale tuples on relations that still exist fail
closed (`StaleRelationship`, §3.3).

`EvalStats` (returned, also emitted on the tracing span): `store_reads`, `tuples_read`,
`memo_hits`, `max_depth_reached`, `cycle_passes` (§3.6), `elapsed`.

---

## 2. Semantics in one table

For object `o`, member `m`, subject `s`, at revision `R`, `S(o, m)` is the set of subjects:

| Member / expr | `S(o, ·)` |
|---|---|
| relation `r` (`Direct`) | subjects stored on `o#r`, where a stored subject set `x#y` contributes `S(x, y)` and a stored wildcard `t:*` contributes every object of type `t` |
| `Computed(m)` | `S(o, m)` |
| `Union[e1..en]` | ∪ |
| `Intersection[e1..en]` | ∩ |
| `Exclusion{a, b}` | `S(o,a) \ S(o,b)` |
| `Arrow{t, p, Any}` | ∪ over objects `x` stored on `o#t` (subject relation ignored, wildcards skipped) of `S(x, p)` (∅ if `x`'s type lacks `p`) |
| `Arrow{t, p, All}` | ∩ over the same objects; ∅ if there are none |

Additional rule — **wildcards and subject sets**: a stored wildcard `t:*` contributes
objects of type `t` only, never a subject set `t:x#r`. SpiceDB matches the subject set as
well (KD-3, `crates/pemba-difftest/KNOWN_DIFFERENCES.md`). Where that wildcard sits on a
granting path, pemba's answer is the stricter one. Where it sits under the subtracted side
of an exclusion, pemba's would be the more permissive one, so `check` (and
`lookup_resources`, whose candidates are checked) fails with
`AmbiguousWildcardExclusion` instead of answering (§3.3 step 2a, T3.4b). The reason is
that an error can later become an answer without breaking callers, but an answer cannot
change silently.

Additional rule — **self membership**: a subject set `o#m` is always a member of `S(o, m)`.
So `check(group:eng, member, group:eng#member) = true`. (Zanzibar semantics; verify
against SpiceDB in differential tests.)

`check(o, m, s)` ≡ `s ∈ S(o, m)`.

---

## 3. `check`

### 3.1 Signature

```rust
fn check(&self, req: CheckRequest) -> Result<CheckResponse, Error>;

pub struct CheckRequest {
    pub resource: ObjectRef,
    pub permission: RelName,       // relation or permission
    pub subject: SubjectRef,       // Object or Set; Wildcard → Err(InvalidRequest)
    pub consistency: Consistency,
}
pub struct CheckResponse {
    pub allowed: bool,
    pub revision: Revision,
    pub stats: EvalStats,
}
```

Also `check_bulk(Vec<CheckItem>, Consistency) -> Vec<Result<bool>>`: one revision, one
shared memo per distinct subject. Item errors are per-item; a store error fails the call.

### 3.2 Core recursion

Memo key: `NodeKey { ty: TypeId, id: ObjectId, rel: RelId }` (subject is constant per request).

```text
eval(key, depth) -> Result<Outcome>          // Outcome { value: bool, low: Option<usize> }
  if depth > max_depth:               return Err(MaxDepthExceeded{ path: stack })
  if memo[key] exists:                return Outcome{ memo[key], low: None }
  if key on SCC stack:                return on_scc_hit(key)          // §3.6
  if subject == Set(key.obj, key.rel): return Outcome{ true, None }    // self membership
  push key to SCC stack at position p
  loop:                                                               // passes, §3.6
    push frame; out = match schema.member(key.ty, key.rel):
                        Relation(r)   => eval_direct(key, r, depth)
                        Permission(e) => eval_expr(key.obj, e.expr, depth)
    pop frame
    value = out.value | provisional[key]
    if out.low < p:  finish as member (pass memo); return Outcome{ value, out.low }
    head of SCC[p..]: settle into memo and return Outcome{ value, None },
                      or record provisional values and re-run
```

### 3.3 `eval_direct(o#r)`

Order matters for performance, not correctness:

1. **Exact hit** — one point query: does a visible tuple `o#r@subject` exist exactly
   (same type, id, and subject relation or lack of it)? → `true`; but if the subject's
   form (`t`, or `t#rel` for a subject set) is not an allowed type of `r` →
   `Err(StaleRelationship)` (T2.6).
2. **Wildcard hit** — only if subject is `Object(t:id)`: if `t:*` is an allowed type on `r`,
   does `o#r@t:*` exist? → `true`. If it is not allowed, the same query runs only when the
   read is at a revision below the current schema's (`05` §7: only there can a stale
   wildcard exist), and a hit → `Err(StaleRelationship)`, since ignoring it would shrink
   `S(o, r)`. (Wildcards never match subject *sets*.)
2a. **Ambiguous wildcard (T3.4b)**: only if the subject is a subject set `t:x#y`, evaluation
   is under the subtracted side of at least one exclusion (any number, not parity), and
   `t:*` is an allowed type on `r`. If `o#r@t:*` exists → `Err(AmbiguousWildcardExclusion
   { wildcard, subject })`. Off a subtracted side the wildcard is not consulted (step 2).
   Memoised node values computed outside a subtracted side are not reused under one, so a
   value settled on a granting path cannot hide this error (`Memo`).
3. **Subject-set fan-out** — read visible tuples `o#r@x:*#y` (i.e. subject_relation
   non-empty), and for each stored subject set `x:id#y` in store order: if `x#y` is not an
   allowed type of `r` (left behind by a schema change, or `x`/`y` no longer defined) →
   `Err(StaleRelationship{ tuple, relation })`; else evaluate `eval(NodeKey{x, id, y},
   depth+1)`; return `true` on the first `true`.
4. Else `false`. Taint = OR of taints of fan-out children evaluated.

Step 3's query runs even when the relation allows no subject sets, so stale ones are
always seen. Skipping them would silently shrink `S(o, r)`, which on the subtracted side
of an exclusion grants access (OQ-019). Since T2.6, `write_schema` refuses changes that
orphan live tuples (`05` §7), so stale tuples exist only at revisions below the current
schema's; reads there (`Exact(old)`) still meet them, so the query keeps running.

**Stale arrow objects (T2.6).** An arrow (§2 `Arrow`) reads the related objects of its
tupleset; one whose type is no longer an allowed type of the tupleset (as an object or a
subject set) → `Err(StaleRelationship)`, rather than ∅ (which fails open under exclusion).
No extra read. `lookup_subjects` (§6.2) applies the same rules: a stored object or
wildcard of the subject type that `r` no longer allows is an error, not counted or
ignored.

### 3.4 `eval_expr(o, expr)`

Evaluate left-to-right with short-circuiting. Error propagation is in evaluation order:
an `Err` in a branch evaluated before a short-circuit propagates (deterministic).

| Expr | Evaluation |
|---|---|
| `Computed(m)` | `eval(NodeKey{o, m}, depth+1)` |
| `Union[..]` | first `true` wins; else `false` |
| `Intersection[..]` | first `false` wins; else `true` |
| `Exclusion{a,b}` | `a` false → `false`; else evaluate `b` **one negation level deeper** (§3.6); result `!b` |
| `Arrow{t,p,Any}` | read visible tuples on `o#t`; distinct related objects `x` (skip wildcards, drop subject relation); for each `x` whose type has `p` → `eval(NodeKey{x, p})`; first `true` wins |
| `Arrow{t,p,All}` | same objects; none → `false`; any `x` whose type lacks `p` → `false`; first `false` wins; else `true` |

Taint of a composite = OR of taints of children actually evaluated.

### 3.5 Fan-out guard

If one node reads more than `max_check_fanout` tuples (default 100 000), return
`Err(FanoutExceeded{ node })`. Prevents pathological checks from silently taking seconds.

### 3.6 Cycles (ADR-009)

Data-dependent recursion (`folder:a#parent@folder:b`, `folder:b#parent@folder:a`;
nested groups that contain each other) is legal. Rules:

- *Negation depth.* The evaluator tracks `neg_depth`: how many `sub` sides of exclusions
  evaluation is currently inside (`+1` on entering a `sub` side, `−1` on leaving it). The
  call stack holds frames `{ key, neg_depth }`, recording the depth at which each node was
  pushed. Depth never falls along the stack, so the path from frame `i` (its own
  expression included) to the current point crosses a `sub` side **iff** the current depth
  differs from frame `i`'s.
- **Current rule (T1.4c): SCC fixed-point iteration** (Tarjan-style). Plain "in-progress
  node = `false`, never memoise what used that" is correct but exponential: on groups
  `g1..gn` whose `member`s all contain each other, with the subject in none, it walks every
  simple path (~(n−1)! of them; OQ-021). Instead each node of a strongly connected region
  is evaluated at most once per pass:
  - *SCC stack.* A node is pushed on the SCC stack on first visit and stays there until the
    head of its region settles; the stack only ever loses a suffix, so a position names the
    same node for as long as anything refers to it. Lowlinks are SCC-stack positions, not
    call-stack indices: once a frame pops, a sibling reuses its call-stack index, and a
    lowlink naming that index would make the sibling look like the head of its own region
    and settle a `false` early (test `reused_call_stack_index_does_not_make_a_false_head`).
  - *Lowlink.* An `Outcome` carries `low: Option<usize>`: the lowest SCC-stack position
    whose unsettled value it used, `None` if it used none (the value is settled). Composite
    `low` = min over the children actually evaluated.
  - *Hit on a node in progress* (frame `i`, SCC position `p`). If the current depth
    differs from frame `i`'s → `Err(NonMonotonicCycle{ path })` (below). Else return `provisional[key]` (`false` if absent) with `low = p`; if that was `false`,
    mark the node *assumed false*.
  - *Hit on a finished member* (the **pass memo**: SCC-stack entries finished this pass).
    Return its value and its `low` without re-evaluating it. The entry's value depends
    on the head of its region (the first node in progress at or above its `low`; every
    member has its head's depth), so the hit closes a cycle through that head: if the
    current depth differs from the head frame's → `NonMonotonicCycle`.
  - *Finish value.* A node finishes with `computed | provisional[key]`, so a finished value
    is never below what a hit assumed for it.
  - *Member finish.* A node at position `p` finishing with `low = l < p` belongs to a region
    headed further down. It stays on the SCC stack, finished, with value and `low = l`, and
    `l` is propagated to every entry above it (`low = min(low, l)`): they are in its region,
    so a later hit on any of them yields `low ≤ l`.
  - *Head finish.* A node at position `p` finishing with `low` `None` or `≥ p` heads the
    region `SCC[p..]`. If no entry of the region was assumed false and finished `true`, the
    pass is consistent: every entry's value goes into the request memo (settled), and the
    region is popped. If the head finished `true`, the region is popped too, memoising the
    head and every member's `true` and dropping members' `false`s (shortcut: a `true` is
    always sound). Otherwise set `provisional[x] |= value(x)` for every entry, pop the
    members, and re-run the head (next pass).
  - *Inner regions* settle inside an outer pass: a head at `p` depends on nothing below `p`,
    so its values are final and go straight into the request memo; no outer pass re-runs
    them.
  - *Termination.* `provisional` only grows (`false → true`), a re-run happens only when an
    assumed `false` finished `true` (so some provisional value flips), and an SCC of `|S|`
    nodes has at most `|S|` flips: at most `|S| + 1` passes.
  - *Soundness (every value `≤` the least fixed point `μ`).* Pass 1 assumes `false = ⊥ ≤ μ`;
    each node is a monotone function of its children, so values computed from inputs `≤ μ`
    are `≤ f(μ) = μ`; `|=` keeps `provisional ≤ μ`. Hence any `true`, provisional or not,
    is a correct `true`.
  - *Leastness (final values `= μ`).* In the final pass every in-progress hit returned the
    value its node finished with, so the values `V` of the nodes `E` evaluated in that pass
    are mutually consistent. A `true` in `V` is `≤ μ` by soundness. A `false` in `V` was
    computed (not taken from `provisional`), and short-circuiting means it is determined by
    the children evaluated, all in `E` or settled. Kleene iteration from `⊥` then stays
    `≤ V` on `E` at every step (induction, by monotonicity), so `μ ≤ V` on `E`, and with
    soundness `V = μ` on `E`. Only nodes in `E` are memoised as settled; members evaluated
    only in earlier passes are dropped, except their `true`s.
  - *Example.* `A = B + C`, `B = A`, C true. Evaluating A: B hits A (assumed false) → B
    false, a member. C true → A true: the head is `true`, so A and C are memoised and B's
    `false` is dropped; a later lookup of B in the same request evaluates it again (true).
  - *Cost.* Each pass evaluates each SCC node at most once, so one `check` makes at most
    `(|S| + 1) · Σ r(x)` store reads per SCC, where `r(x)` is the reads one evaluation of
    `x` makes (2–3 for a relation, one per arrow): quadratic in the reachable nodes in the
    worst case, usually one or two passes (the complete group graph above: one pass, ~2n
    reads). `EvalStats::cycle_passes` counts the passes of heads that had to re-run
    (including their first); a region settled in its first pass adds nothing.
  - *State per check.* The SCC stack, pass memo and provisional values are cleared at the
    start and end of every `check` and every `check_bulk` item (after an error too); only
    the request memo, holding settled values, is shared between `check_bulk` items.
  - *Backstop.* `EngineConfig::max_check_reads` (default **200 000**) caps store reads per
    `check` (per item for `check_bulk`); exceeding it returns `ReadLimitExceeded{ max }`
    (`06` §1), failing closed. It bounds cost against bugs in this rule and against large
    but legitimate graphs, independently of any proof. 200 000 is twice
    `max_check_fanout`, so one node at full fan-out (each child ≥ 1 read) still fits; at an
    estimated 5–20 µs per indexed SQLite point read (not yet measured) it caps a
    pathological check at roughly 1–4 s.
- **Negation (T1.4b): stratified.** A cycle is `NonMonotonicCycle` only if the path from the
  in-progress node (including its own expression) to the closing edge crosses a `sub` side,
  i.e. its ends differ in negation depth (the two hit rules above). Such a cycle has no
  well-defined least fixed point; we refuse rather than guess. Examples: the self-loop
  `view = viewer - parent->view` with `doc:a#parent@doc:a` (the only frame, `doc:a#view`,
  is at depth 0; the closing edge at depth 1), and a cycle inside a `sub` side whose every
  step crosses a nested exclusion's `sub` side (depths 1, 2, 3, …).
  - *Monotone cycles beneath a `sub` side* (nested banned groups that contain each other)
    have every node at the same depth, so they form an ordinary region: it settles by the
    SCC iteration above, at its least fixed point, before the exclusion negates it.
  - *Why the negated value is settled.* A `low` can only come from a hit at the hit node's
    own depth, and every node in progress at the `sub` side's depth or deeper finished
    inside it, so the `sub` result always has `low = None`. The evaluator checks this and
    fails closed (internal error) if it ever does not; the exclusion's `low` is the base's.
  - So every region that is iterated is monotone, and the iteration's soundness and
    leastness arguments apply to it unchanged. This is stratified negation: on a
    stratifiable schema (`07` §2.1) no data cycle can cross a negation, so the engine never
    returns `NonMonotonicCycle` there (property 1 runs a generator variant with monotone
    recursion beneath `sub` sides to check exactly this). See OQ-018.

### 3.7 Depth

`depth` increments on every `eval` call. Default `max_depth = 50`. Exceeding it is an error
(`MaxDepthExceeded`) carrying the stack as human-readable path strings
(`document:a#view → folder:b#view → ...`) for debugging.

### 3.8 Trace (T4.1)

```rust
fn check_with_trace(&self, req: CheckRequest) -> Result<(CheckResponse, CheckTrace), Error>;
```

**Same evaluator.** `check` and `check_with_trace` run one evaluator, generic over a trace
sink. `check` passes a zero-sized no-op sink whose hooks compile away (every hook is an
empty inlined method, and any work to describe a node sits behind the sink's `ENABLED`
constant or inside a closure the no-op sink never calls). So the trace cannot change an
answer, a read, or an error: `check_with_trace(r).0` equals `check(r)` in everything but
`stats.elapsed`, and returns the same `Err` whenever `check` would. The sink only observes.

**Fail closed.** If evaluation fails, `check_with_trace` returns the `Err` and no trace: a
partial tree could be read as an answer (OQ-042 proposes a separate debugging channel).

**Shape.** `CheckTrace { resource, permission, subject, allowed, revision, stats,
truncated, root: TraceNode }`. The tree is the evaluation in the order it happened: the root
is the requested `resource#permission`, and each node's children are the steps it took, in
evaluation order. Each node:

| Field | Meaning |
|---|---|
| `id` | position in a pre-order walk of the tree, root `0`; what `ref` points at |
| `node` | the thing evaluated: `type:id#member`, or for `direct`/`wildcard` the relationship queried |
| `kind` | how it is evaluated (table below) |
| `expr` | the expression's source text (canonical form) for permission and operator nodes, else `None` |
| `role` | `base` / `subtracted` for the two children of an `exclusion`, else `None` |
| `result` | `Some(bool)`; `None` exactly when `source` is `skipped` |
| `source` | `evaluated`, `memo_hit { ref }`, `cycle { passes }`, `cycle_hit { ref }`, `skipped` |
| `provisional` | `true` only on a `false` that never settled (cycles, below) |
| `tuple` | the stored relationship: the match on a `direct`/`wildcard` that is `true`, the stored subject set on a `subject_set` |
| `duration` | wall time from entering the node to leaving it, children included; zero for hits and skipped nodes |
| `truncated` | some of this node's children were not recorded (cap, below) |
| `children` | the steps, in order |

Kinds. A **member node** is one `eval(o#m)` (§3.2) and is named `type:id#member`; its kind
is `relation` for a relation, `self` for self membership, and for a permission the kind of
its expression's top operator (so `permission view = viewer + owner` is one `union` node,
not a permission node wrapping a union). An operand that is a bare name (`viewer`) appears
as the member node it evaluates; any other operand is an **operator node**, named like the
permission it belongs to, with `expr` set to the operand.

| Kind | Children | `result` |
|---|---|---|
| `relation` | a `direct` leaf, then a `wildcard` leaf if that query ran (§3.3 step 2), then one `subject_set` per stored subject set, in store order | any child `true` |
| `direct` | none; `node` is `o#r@subject` | the exact tuple exists |
| `wildcard` | none; `node` is `o#r@t:*` | the wildcard tuple exists |
| `subject_set` | the member node of the stored set `x:id#y` (`node` is `x:id#y`, `tuple` the stored `o#r@x:id#y`) | its child's |
| `self` | none | `true` (§2 self membership) |
| `computed` | the member node named (`permission edit = owner`) | its child's |
| `union` | operands | any child `true` |
| `intersection` | operands | every evaluated child `true` |
| `exclusion` | `base`, then `subtracted` | `base && !subtracted` |
| `arrow_any` / `arrow_all` | one member node `x:id#p` per related object, in store order, or an `undefined` leaf where `x`'s type lacks `p` | any `true` / at least one child and every evaluated child `true` |
| `undefined` | none; `node` is `x:id#p` | `false` (contributes ∅) |

**Skipped.** Where evaluation short-circuits (§3.4), the siblings it never evaluated still
appear, `source: skipped`, `result: None`, no children: union operands after a `true`,
intersection operands after a `false`, the subtracted side after a `false` base, arrow
objects after the deciding one, and subject sets after a `true` one (already read by the
same query). A skipped bare-name operand or arrow object is named and kinded like the
member node it would have been. Skipped nodes appear only after a sibling that decided its
parent.

**Memo hits (a DAG flattened into a tree).** A node answered from the request memo has
`source: memo_hit { ref }`, no children, and `ref` names the one node where that member is
shown in full. A member is shown in full exactly once; every later occurrence is a
reference, so the trace is linear in the nodes evaluated, never exponential in the paths
to them. `ref` is `None` only when the full node was never recorded (cap). For a
subject-set subject the memo keeps values computed under a subtracted side apart from the
others (§3.3, `Memo`), so one member can be settled once on each side, and evaluated again
on one side after settling on the other; a memo hit refers to the node that settled the
side it was answered from, never to a later evaluation on the other side (found by the
trace property in CI; `a_memo_hit_refers_to_the_node_that_settled_its_side`).

**Cycles (§3.6).** The trace shows only the final pass of each region; passes that were
re-run are discarded, so no intermediate pass's value is shown as an answer. The head of a
region (a member some node reached again while it was in progress) has `source: cycle {
passes }`, `passes ≥ 1`; its `result` is the region's least fixed point. A node reached
again on the stack within a pass has `source: cycle_hit { ref }`, no children, `result` the
value the evaluator used there (its provisional value if in progress, its finished value if
already done this pass), and `ref` names that node.

**Settled values (hard rule).** A node's shown `result` (in the tree, the JSON and the
rendered line) is its settled value, the least fixed point `S(o, m) ∋ subject`, or the node
says it is not. There is exactly one such label, `provisional: true` (rendered
`(provisional: computed from a cycle assumption that did not hold)`), and it only ever sits
on a `false`, because every `true` is sound (§3.6). This holds for member nodes, subject
sets, hits and the operands under them. `07` §2.3 tests it on every generated case against
the reference evaluator, in all three forms. The three cases where a shown value could
differ from the settled one, and what is recorded and drawn for each:

1. *A provisional `false`.* A region that settles by the head-`true` shortcut has values
   from a pass in which some assumption failed. Its `true`s are exact; every `false` that
   depended on an unsettled value (`low` set when the node finished, and every cycle hit) is
   recorded with `provisional: true` when the head settles, and drawn `✗ … (provisional: …)`.
   A region that settles with every assumption confirmed marks nothing: its values are
   exact. In the `A = B + C`, `B = A` example: `A` is `✓ … (cycle: settled after 1 pass)`,
   the hit on `A` inside `B` is `✗ … (cycle: reached again, see above; provisional: …)`,
   and `B` is `✗ … (provisional: …)`. A later evaluation of `B` in the same check (the
   shortcut does not memoise `B`) is shown in full with its real value.
2. *A memo hit on a node computed in a discarded pass.* An inner region (or an acyclic
   node) that settled inside a pass of an outer head that was later re-run is in the memo,
   so the final pass meets it as a memo hit, while its full node was in the discarded pass.
   Its value is settled (only settled values enter the memo), so the trace shows that full
   node, with its subtree, at the first such hit, `source: evaluated` (or `cycle`), exactly
   as if evaluated there; later hits refer to it. Nothing else of the discarded pass is
   shown. In the three-pass case of `tests/cycle_passes.rs`, `node_h:1#rel_k` was evaluated
   in pass 1 and is drawn in full under the final pass.
3. *A cycle reference that cannot be resolved.* Case 2 can show a member `k` of an inner
   region without its head `h`: `k`'s subtree has a `cycle_hit` on `h`, and `h` is reached
   later in the final pass or not at all. The hit's *value* is still settled or labelled
   (it was resolved when `h` settled, in the discarded pass); only its reference is
   unusual. If `h` is shown later, `ref` names it and the line says `(cycle: reached again,
   see below)`. If `h` is not in the trace, `ref` is `None` and the line says `(cycle:
   reached again, unresolved: that node is not in this trace)`. Neither changes `truncated`:
   nothing was left out, and no value is wrong. Not produced by any generated case so far
   (about 15 000 cycle hits in the coverage samples); covered by unit tests that drive the
   recorder through the event sequence (`trace::record::tests`).

**Coverage gap: no cycle on a subtracted side.** The generator built to make heads re-run
(`generate::cyclic_case`, `07` §2.3) never places a cycle on the subtracted side of an
exclusion: every subtracted side it writes is a relation with no dependencies, so cycles
run only through unions, intersections, arrows and exclusion *bases*. That is deliberate.
A data cycle that crosses a subtracted side is `NonMonotonicCycle` (§3.6): an error, so
there is no trace to check, and the reference evaluator refuses such schemas as not
stratifiable (`07` §2.1), so there would be no oracle either. The other case, a monotone
cycle wholly beneath a subtracted side, settles as an ordinary region at its own negation
depth before the exclusion negates it (§3.6, stratified negation); the trace records it by
the same code as any region. It is exercised by property 7 on the `SubSides::Recursive`
generator, but in 600 sampled cases the general generators made no head re-run at all,
so re-runs beneath a subtracted side are covered by reasoning, not by generated cases. The
hand-written `NonMonotonicCycle` tests check that the trace fails closed there.

**Invariants** (tested on every generated case, `07` §2.3, for nodes that are not
`truncated`): the root's `result` is `allowed`; `relation`/`union`/`arrow_any` are `true`
iff some child is; `intersection` iff every evaluated child is; `arrow_all` iff it has a
child and every evaluated child is; `exclusion` = `base && !subtracted` (a skipped
`subtracted` counts as `false`); `computed`/`subject_set` equal their child; skipped nodes
come only after a sibling whose value decided the parent (`true` in an OR, `false` in an
AND, a `false` base); every `direct`/`wildcard` that is `true` carries its `tuple`; every
`ref` names a node shown in full with the same `node`; a `memo_hit`'s names an earlier one
with the same `result`. A `cycle_hit`'s `result` is the value used at that point, which a
region that later grew can exceed (then it is `provisional`), and its `ref` can point down or
be `None` (case 3 above).

**Limits.** `EngineConfig::max_trace_nodes` (default **10 000**) caps the nodes recorded,
counting those of discarded passes. Past it nothing more is recorded but evaluation runs to
the end: the answer, stats and errors are unchanged, `CheckTrace::truncated` is `true`, and
each node that lost children has `truncated: true`. Tracing does not count against
`max_check_reads`; its own time is included in `stats.elapsed`.

**Formats.** `CheckTrace::to_json()` is the public JSON format (`06` §3.1);
`CheckTrace::render()` is the text `pemba check --explain` prints (`06` §3).

**The granting path in the rendering.** The text marks the *granting path*: from the root
down, the children that decided each node's value. A `true` union or `.any` arrow is decided
by its first `true` child; a `false` intersection or `.all` arrow by its first `false` one;
an exclusion that is `false` because its base is `false` by the base alone; an exclusion
that is `false` because the subtracted side removed a granted base by **both** sides, so the
reader sees where the permission was granted and where it was removed; every other node
by all its evaluated children. The top of each evaluated branch off the path is marked
`(not needed)`. `render_ansi()` is the same text with ANSI styling and nothing else
(removing the escapes gives `render()`): the path bold, with green `✓` and red `✗`;
evaluated nodes off it in the terminal's normal colour, `false` or not; dim only for lines
that show no evaluation of their own, skipped nodes and memo hits off the path.

---

## 4. Store access patterns used by `check`

| Step | Store call (`docs/05` §2) |
|---|---|
| exact hit | `exists(rev, resource, relation, subject_exact)` |
| wildcard hit | `exists(rev, resource, relation, Wildcard(t))` |
| subject-set fan-out | `read_subject_sets(rev, resource, relation)` |
| arrow | `read_related_objects(rev, resource, relation)` → distinct `ObjectRef`s, wildcards excluded |

Batching (post-MVP optimisation): `read_related_objects_many(rev, &[resource], relation)` to
fetch an arrow level for many objects in one query.

---

## 5. `lookup_resources` (ADR-007)

```rust
fn lookup_resources(&self, req: LookupResourcesRequest) -> Result<LookupResourcesResponse>;
pub struct LookupResourcesRequest {
    pub resource_type: TypeName,
    pub permission: RelName,
    pub subject: SubjectRef,        // Object or Set
    pub consistency: Consistency,
    pub limit: Option<u32>,         // default/max: max_lookup_results
    pub cursor: Option<ObjectId>,   // exclusive; results ordered by id ascending
}
pub struct LookupResourcesResponse {
    pub resource_ids: Vec<ObjectId>,
    pub next_cursor: Option<ObjectId>,
    pub revision: Revision,
}
```

### 5.1 Phase 1 — candidate generation (backward walk, over-approximate)

Work on nodes `N = (type, id, rel)` meaning "the subject *might* be in `S(type:id, rel)`".
Every operator is treated as a union here; intersection, exclusion and `.all` therefore
over-approximate, which is fine because Phase 2 filters.

Seeds:
- Subject `Object(t:id)`:
  - every visible tuple whose subject is exactly `t:id` (no subject relation) → node `(R, rid, rel)`
  - every visible tuple whose subject is `t:*` → node `(R, rid, rel)`
- Subject `Set(t:id#r)`: node `(t, id, r)` itself (self membership).

Expansion of node `(T, id, r)` (visit each node once; BFS with a `visited` set):
1. **Same-object use**: for each permission `p` on `T` in `used_by[(T, r)]` → `(T, id, p)`.
2. **Subject-set use**: every visible tuple whose subject is exactly `T:id#r` → `(R, rid, rel)`.
   (Only query if `subject_set_uses[(T, r)]` is non-empty.)
3. **Arrow use**: for each `(S, t, p)` in `arrows_into[(T, r)]`: every visible tuple
   `S:*#t@T:id` (any subject relation) → `(S, sid, p)`.

Collect nodes with `type == resource_type && rel == permission` as candidates.
Abort with `Err(LookupTooLarge)` if `visited` exceeds `max_lookup_nodes` (default 200 000).

Store calls: `read_reverse(rev, subject_filter, resource_filter)` (`docs/05` §2).

Implementation notes (T2.4, OQ-028):
- A `Set(t:id#r)` subject also seeds every visible tuple whose subject is exactly `t:id#r`
  (rule 2 on the seed node, run even when `subject_set_uses[(t, r)]` is empty), because
  `check` matches such a tuple by exact lookup whether or not the schema still allows it.
- `read_reverse` rows whose resource type or relation the schema does not define are
  skipped: no schema member reaches them, so `check` cannot pass through them.
- Rule 3 reads once per `(S, t)` pair and fans out to every `p` sharing it.
- `LookupTooLarge` is raised on the attempt to visit node `max_lookup_nodes + 1`. Phase 1
  reads do not count toward `max_check_reads`, which bounds each phase 2 `check`.

### 5.2 Phase 2 — filter

Sort candidate ids ascending (byte-wise), drop ids `<= cursor`. For each, run the §3 evaluator with
**one shared memo** for the whole request (valid: same subject, same revision). Collect
allowed ids until `limit` is reached; `next_cursor` = last returned id if more candidates remain.
Any candidate's `check` error fails the page (fail closed). Both phases of a page read
through one store snapshot (§1); paging consistency is the caller's job (`06` §1).

### 5.3 Correctness argument (put in code comments)

If `s ∈ S(o, m)` there is a finite derivation: a chain from a stored tuple mentioning `s`
(or `s` itself for subject sets) through direct, computed, subject-set and arrow steps to
`(o, m)`. Each step type is inverted by exactly one expansion rule, and every positive
derivation through `Intersection`/`Exclusion`/`All` passes through a positive branch that
the union-approximation includes. Hence candidates ⊇ answer, and Phase 2 makes it exact.
Property-tested against brute force (`docs/07` §2.3).

---

## 6. `lookup_subjects`

```rust
pub struct LookupSubjectsRequest {
    pub resource: ObjectRef,
    pub permission: RelName,
    pub subject_type: TypeName,     // MVP: object subjects only (no subject relation)
    pub consistency: Consistency,
}
pub struct LookupSubjectsResponse {
    pub subject_ids: Vec<ObjectId>,       // sorted ascending; empty if wildcard
    pub wildcard: bool,                   // true = every object of subject_type ...
    pub excluded_ids: Vec<ObjectId>,      // ... except these
    pub revision: Revision,
}
```

### 6.1 Set representation

```rust
enum SubjectSet {
    Ids(BTreeSet<ObjectId>),
    AllExcept(BTreeSet<ObjectId>),   // wildcard minus exclusions
}
```

Algebra (A, B):

| Op | `Ids(a)`,`Ids(b)` | `Ids(a)`,`AllExcept(y)` | `AllExcept(x)`,`Ids(b)` | `AllExcept(x)`,`AllExcept(y)` |
|---|---|---|---|---|
| ∪ | `Ids(a∪b)` | `AllExcept(y\a)` | `AllExcept(x\b)` | `AllExcept(x∩y)` |
| ∩ | `Ids(a∩b)` | `Ids(a\y)` | `Ids(b\x)` | `AllExcept(x∪y)` |
| A \ B | `Ids(a\b)` | `Ids(a∩y)` | `AllExcept(x∪b)` | `Ids(y\x)` |

Unit-test every cell, and property-test the algebra against a finite universe model.

### 6.2 Forward evaluation

`gather(o, m) -> SubjectSet` mirrors §2, for one `subject_type` at one revision:
- `Direct` (`read_tuples`): object subjects of `subject_type` → `Ids`; a wildcard
  `subject_type:*` → `AllExcept(∅)` if the relation allows that wildcard (as `check`, §3.3,
  which ignores a stored wildcard the schema does not allow); stored subject sets `x#y` →
  `gather(x, y)`, resolved and checked for staleness exactly as `check` does
  (`StaleRelationship`); union all. Subjects of other types contribute ∅. Fan-out is checked
  per node as in §3.5.
- `Computed`, `Union`, `Intersection`, `Exclusion`, `Arrow Any/All` via the algebra
  (`All` over zero objects, or over any object whose type lacks the target → `Ids(∅)`).
  An intersection stops at the first `Ids(∅)` operand and an exclusion does not evaluate its
  `sub` side when the base is `Ids(∅)`, as `check` short-circuits on `false`.
- **Cycles: exactly §3.6's SCC fixed-point iteration, with `SubjectSet` values in place of
  booleans.** One implementation serves both (`pemba-core::scc`, generic over the value):
  - *Order.* Values are compared by `≤` (subset of the denoted sets): `Ids(a) ≤ Ids(b)` iff
    `a ⊆ b`; `Ids(a) ≤ AllExcept(x)` iff `a ∩ x = ∅`; `AllExcept(x) ≤ AllExcept(y)` iff
    `y ⊆ x`; `AllExcept(x) ≤ Ids(b)` never (the universe of ids is unbounded). `⊥` is
    `Ids(∅)`, the join is `∪`, and the top `AllExcept(∅)` plays the role of `true`.
    For `bool`, `false < true`.
  - *SCC stack and lowlinks* as §3.6: a node is pushed on first visit and stays until the
    head of its region settles; an outcome carries `low`, the lowest SCC-stack position whose
    unsettled value it used.
  - *Hit on a node in progress*: negation-depth check first (differing depth →
    `NonMonotonicCycle`), else return `provisional[key]` (`⊥` if absent) with `low = p`,
    and record on the entry the value that was assumed.
  - *Hit on a finished member* (the pass memo): return its value and `low`, after the same
    negation-depth check against its region's head.
  - *Finish value*: `computed ∪ provisional[key]`, so never below what any hit assumed.
  - *Member finish / head finish* as §3.6. At a head, the pass is consistent unless some
    entry's finished value **grew** past the value a hit assumed for it: a strict increase in
    `≤` (for `bool`: an assumed `false` that finished `true`). Consistent, or the head's value
    is the top: settle (memoise every entry, or for a top head the head and every member
    whose value is the top) and pop. Otherwise `provisional[x] := provisional[x] ∪ value(x)`
    for every entry, pop the members, and re-run the head.
  - *Request memo*: `(o, m) → SubjectSet`, settled values only. Unsettled values never leave
    the SCC stack.
  - *Termination.* `provisional` values only grow (each is a join with the previous one),
    and a re-run needs some entry to grow strictly. A value is `Ids(a)` or `AllExcept(x)`
    where `a` and `x` contain only ids mentioned in the stored data (every operation of
    §6.1 builds its sets from its operands' sets, and `Direct` only adds stored ids), so
    each node's provisional value climbs a chain of length at most `2·|D| + 2` over the
    data's ids `D` (`Ids` growing by one id at a time, then `AllExcept` shrinking): at most
    `|S| · (2·|D| + 2) + 1` passes per region of `|S|` nodes. The soundness and leastness
    arguments of §3.6 carry over word for word with `≤` for implication: every value is
    `≤ μ` because each pass starts from provisional values `≤ μ` and every operator used
    under positive polarity is monotone; and in the final, consistent pass the values are a
    fixed point reached from below, so they equal `μ`.
  - *Negation.* As §3.6 (stratified): a cycle whose ends differ in negation depth is
    `NonMonotonicCycle`; the `sub` side of an exclusion must come back settled (`low =
    None`) or the request fails closed with an internal error.
- *Cost bound.* `lookup_subjects` counts its store reads against
  `EngineConfig::max_check_reads` (the whole request is one `gather`, evaluated by the same
  per-node, per-pass rules as one `check`, so the same `(|S| + 1)`-passes-style bound and the
  same default apply) and fails closed with `ReadLimitExceeded`. `max_depth` applies as for
  `check`.

### 6.3 Invariant (property test)

For every object `u` of `subject_type` appearing anywhere in the data **and** a fresh id never
written: `check(resource, permission, u) == response contains u`
(contains = `u ∈ ids`, or `wildcard && u ∉ excluded`).

---

## 7. Concurrency

`Engine: Send + Sync`. Evaluation is single-threaded per request in the MVP. Parallel
fan-out (rayon) is a post-MVP optimisation and must keep results deterministic, including
which error is returned.
