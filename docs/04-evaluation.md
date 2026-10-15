# 04 — Evaluation: check, lookup_resources, lookup_subjects

Crate: `warden-core::eval`. Inputs: a `CompiledSchema` (`docs/03` §6), a `ReadStore`
(`docs/05` §2) and a resolved `Revision`.

Every algorithm here must agree with the reference evaluator (`docs/07` §2) on every input.

---

## 1. Request lifecycle (all read APIs)

1. Validate the request against the schema (types/relations exist, subject type valid).
   Invalid → `Err(InvalidRequest{..})` — never evaluate a malformed request.
2. Resolve `Consistency` → one `Revision` (`docs/02` §4). Use it for every read.
3. Create a fresh `EvalContext { rev, memo, stack, depth, stats }`. Nothing is shared
   between requests except the immutable schema and the store.
4. Evaluate. Return `{ result, revision, stats }`.

`EvalStats` (returned, also emitted on the tracing span): `store_reads`, `tuples_read`,
`memo_hits`, `max_depth_reached`, `elapsed`.

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
eval(key, depth) -> Result<Outcome>          // Outcome { value: bool, tainted: bool }
  if depth > max_depth:               return Err(MaxDepthExceeded{ path: stack })
  if memo[key] exists:                return Outcome{ memo[key], tainted: false }
  if key in stack:                    return on_cycle(key)            // §3.6
  if subject == Set(key.obj, key.rel): return Outcome{ true, false }   // self membership
  push key to stack
  out = match schema.member(key.ty, key.rel):
          Relation(r)   => eval_direct(key, r, depth)
          Permission(p) => eval_expr(key.obj, p.expr, depth)
  pop stack
  if !out.tainted: memo[key] = out.value
  return out
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
| `Exclusion{a,b}` | `a` false → `false`; else evaluate `b` **inside a negation frame** (§3.6); result `!b` |
| `Arrow{t,p,Any}` | read visible tuples on `o#t`; distinct related objects `x` (skip wildcards, drop subject relation); for each `x` whose type has `p` → `eval(NodeKey{x, p})`; first `true` wins |
| `Arrow{t,p,All}` | same objects; none → `false`; any `x` whose type lacks `p` → `false`; first `false` wins; else `true` |

Taint of a composite = OR of taints of children actually evaluated.

### 3.5 Fan-out guard

If one node reads more than `max_check_fanout` tuples (default 100 000), return
`Err(FanoutExceeded{ node })`. Prevents pathological checks from silently taking seconds.

### 3.6 Cycles (ADR-009)

Data-dependent recursion (`folder:a#parent@folder:b`, `folder:b#parent@folder:a`;
nested groups that contain each other) is legal. Rules:

- The stack holds frames `{ key, negated: bool }`. A frame is pushed with `negated = true`
  while evaluating the `sub` side of an `Exclusion` (and stays true for everything beneath).
- `on_cycle(key)`: find the frame index `i` of `key` in the stack.
  - If any frame from `i` to the top has `negated = true` → `Err(NonMonotonicCycle{ path })`.
    A cycle through negation has no well-defined least fixed point; we refuse rather than guess.
  - Else → `Outcome{ value: false, tainted: true }`. Treating an in-progress node as `false`
    computes the least fixed point for monotone operators (union, intersection, arrows).
- **Tainted results are never memoised**, because they were computed under an assumption.
  Example of why: `A = B + C`, `B = A`. Evaluating A: B sees A in progress → B=false (tainted).
  C=true → A=true. If B=false had been memoised, a later lookup of B in the same request
  would wrongly return false.
- A tainted `true` is still a correct `true` for monotone expressions and is returned as such;
  it simply is not cached (optimisation to cache it is allowed later only with property tests).
- **Current rule (T1.3):** `negated` frames are sticky, and the edge closing the cycle counts
  too: any cycle detected while evaluating beneath a `sub` side is `NonMonotonicCycle`, even
  a monotone cycle that lies wholly inside it (fails closed).
- **Planned rule (T1.4b, decided, not yet implemented):** a cycle is `NonMonotonicCycle` only
  if the path from the in-progress node (including its own expression) to the closing edge
  crosses a `sub` side. A monotone cycle wholly inside a `sub` side is evaluated as a least
  fixed point and then negated (stratified negation). See OQ-018. It builds on T1.4c
  (OQ-021), which makes taint lowlink-style: a result carries the lowest stack index of an
  in-progress node it assumed `false`; a node whose lowlink is not below its own index has a
  settled value and is memoised.

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

### 5.2 Phase 2 — filter

Sort candidate ids ascending, drop ids `<= cursor`. For each, run the §3 evaluator with
**one shared memo** for the whole request (valid: same subject, same revision). Collect
allowed ids until `limit` is reached; `next_cursor` = last returned id if more candidates remain.

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
  `Err(NonMonotonicCycle)`; tainted results not memoised. Same rules as §3.6.

### 6.3 Invariant (property test)

For every object `u` of `subject_type` appearing anywhere in the data **and** a fresh id never
written: `check(resource, permission, u) == response contains u`
(contains = `u ∈ ids`, or `wildcard && u ∉ excluded`).

---

## 7. Concurrency

`Engine: Send + Sync`. Evaluation is single-threaded per request in the MVP. Parallel
fan-out (rayon) is a post-MVP optimisation and must keep results deterministic, including
which error is returned.
