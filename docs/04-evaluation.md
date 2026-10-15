# 04 — Evaluation: check, lookup_resources, lookup_subjects

Crate: `warden-core::eval`. Inputs: a `CompiledSchema` (`docs/03` §6), a `ReadStore`
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
   (same type, id, and subject relation or lack of it)? → `true`.
2. **Wildcard hit** — only if subject is `Object(t:id)` and `t:*` is an allowed type on `r`:
   does `o#r@t:*` exist? → `true`. (Wildcards never match subject *sets*.)
3. **Subject-set fan-out** — read visible tuples `o#r@x:*#y` (i.e. subject_relation
   non-empty), and for each stored subject set `x:id#y` in store order: if `x#y` is not an
   allowed type of `r` (left behind by a schema change, or `x`/`y` no longer defined) →
   `Err(StaleRelationship{ tuple, relation })`; else evaluate `eval(NodeKey{x, id, y},
   depth+1)`; return `true` on the first `true`.
4. Else `false`. Taint = OR of taints of fan-out children evaluated.

Step 3's query runs even when the relation allows no subject sets, so stale ones are
always seen. Skipping them would silently shrink `S(o, r)`, which on the subtracted side
of an exclusion grants access (OQ-019). Once T2.6 refuses schema changes that orphan
tuples, the query may again be skipped for relations without subject-set types.

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
    `check` (per item for `check_bulk`); exceeding it returns `CheckReadsExceeded{ max }`
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

### 3.8 Trace (debug feature)

`check_with_trace` (M3 stretch) returns `CheckTrace`: a tree of
`{ node: "document:x#view", result, cached, children, duration }`. Same evaluator, with a
trace sink enabled; off by default (zero cost when off — use a generic `Sink` type param or
`Option`).

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

`gather(o, m) -> SubjectSet` mirrors §2:
- `Direct`: object subjects of `subject_type` → `Ids`; wildcard `subject_type:*` →
  `AllExcept(∅)`; stored subject sets `x#y` → `gather(x, y)`; union all.
- `Computed`, `Union`, `Intersection`, `Exclusion`, `Arrow Any/All` via the algebra
  (`All` over zero objects → `Ids(∅)`).
- Memo per `(o, m)`; cycles: in-progress → `Ids(∅)` tainted; through negation →
  `Err(NonMonotonicCycle)`; tainted results not memoised. Same rules as §3.6 (whose SCC
  iteration, added for `check` in T1.4c, should be reused here when this is built).

### 6.3 Invariant (property test)

For every object `u` of `subject_type` appearing anywhere in the data **and** a fresh id never
written: `check(resource, permission, u) == response contains u`
(contains = `u ∈ ids`, or `wildcard && u ∉ excluded`).

---

## 7. Concurrency

`Engine: Send + Sync`. Evaluation is single-threaded per request in the MVP. Parallel
fan-out (rayon) is a post-MVP optimisation and must keep results deterministic, including
which error is returned.
