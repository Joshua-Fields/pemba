//! Compiled schema IR (`docs/03-schema-language.md` §6): names interned to [`TypeId`] /
//! [`RelId`], references resolved, and the reverse indexes that lookups walk
//! (`docs/04-evaluation.md` §5).
//!
//! Produced only by [`crate::compile`], so every id in it is valid for the schema it came
//! from: indexing `types` with a [`TypeId`], or a type's `members` with a [`RelId`] taken
//! from this schema, is in bounds.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

pub use crate::ast::ArrowMode;

/// Index of a type in [`CompiledSchema::types`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TypeId(pub(crate) usize);

impl TypeId {
    /// Position in [`CompiledSchema::types`].
    pub const fn index(self) -> usize {
        self.0
    }
}

/// Index of a relation or permission in its type's [`TypeDef::members`]. Only meaningful
/// together with the [`TypeId`] it belongs to.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RelId(pub(crate) usize);

impl RelId {
    /// Position in [`TypeDef::members`].
    pub const fn index(self) -> usize {
        self.0
    }
}

/// `(source type, tupleset relation, permission containing the arrow)`: one arrow use
/// recorded in [`CompiledSchema::arrows_into`].
pub type ArrowUse = (TypeId, RelId, RelId);

/// A validated, compiled schema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompiledSchema {
    /// Types in source order, indexed by [`TypeId`].
    pub types: Vec<TypeDef>,
    type_by_name: HashMap<Arc<str>, TypeId>,
    /// The original text, for storage and round-trip.
    pub source: Arc<str>,
    used_by: HashMap<(TypeId, RelId), Vec<(TypeId, RelId)>>,
    arrows_into: HashMap<(TypeId, RelId), Vec<ArrowUse>>,
    subject_set_uses: HashMap<(TypeId, RelId), Vec<(TypeId, RelId)>>,
}

impl CompiledSchema {
    /// Builds the name table and derived indexes. `types` must be fully resolved.
    pub(crate) fn new(types: Vec<TypeDef>, source: Arc<str>) -> Self {
        let type_by_name = types.iter().map(|t| (t.name.clone(), t.id)).collect();
        let mut used_by: HashMap<_, Vec<_>> = HashMap::new();
        let mut arrows_into: HashMap<_, Vec<_>> = HashMap::new();
        let mut subject_set_uses: HashMap<_, Vec<_>> = HashMap::new();
        for t in &types {
            for (index, member) in t.members.iter().enumerate() {
                let here = (t.id, RelId(index));
                match member {
                    Member::Relation(r) => {
                        for allowed in &r.allowed {
                            if let AllowedSubject::Set(st, sr) = *allowed {
                                subject_set_uses.entry((st, sr)).or_default().push(here);
                            }
                        }
                    }
                    Member::Permission(p) => p.expr.visit(&mut |e| match e {
                        Expr::Computed(rel) => {
                            used_by.entry((t.id, *rel)).or_default().push(here);
                        }
                        Expr::Arrow {
                            tupleset, targets, ..
                        } => {
                            for (&tt, &tr) in targets {
                                arrows_into
                                    .entry((tt, tr))
                                    .or_default()
                                    .push((t.id, *tupleset, here.1));
                            }
                        }
                        _ => {}
                    }),
                }
            }
        }
        // Sorted and deduplicated so lookups see a deterministic order.
        for list in used_by.values_mut().chain(subject_set_uses.values_mut()) {
            list.sort_unstable();
            list.dedup();
        }
        for list in arrows_into.values_mut() {
            list.sort_unstable();
            list.dedup();
        }
        Self {
            types,
            type_by_name,
            source,
            used_by,
            arrows_into,
            subject_set_uses,
        }
    }

    /// The id of the type named `name` (including any `/` prefixes).
    pub fn type_id(&self, name: &str) -> Option<TypeId> {
        self.type_by_name.get(name).copied()
    }

    /// The type with id `id`.
    pub fn type_def(&self, id: TypeId) -> Option<&TypeDef> {
        self.types.get(id.0)
    }

    /// Permissions on `ty` whose expression contains `Computed(rel)` anywhere.
    pub fn used_by(&self, ty: TypeId, rel: RelId) -> &[(TypeId, RelId)] {
        self.used_by.get(&(ty, rel)).map_or(&[], Vec::as_slice)
    }

    /// Every arrow `tupleset->rel` (or `.any`/`.all`) where `ty` is allowed on the tupleset
    /// and has `rel`, as `(source type, tupleset, permission containing the arrow)`.
    pub fn arrows_into(&self, ty: TypeId, rel: RelId) -> &[ArrowUse] {
        self.arrows_into.get(&(ty, rel)).map_or(&[], Vec::as_slice)
    }

    /// Relations whose allowed subjects include the subject set `ty#rel`, as
    /// `(resource type, relation)`.
    pub fn subject_set_uses(&self, ty: TypeId, rel: RelId) -> &[(TypeId, RelId)] {
        self.subject_set_uses
            .get(&(ty, rel))
            .map_or(&[], Vec::as_slice)
    }
}

/// One object type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeDef {
    /// This type's id.
    pub id: TypeId,
    /// Full type name, including any `/` prefixes.
    pub name: Arc<str>,
    /// Doc comment text.
    pub doc: Option<String>,
    /// Relations and permissions in source order, indexed by [`RelId`].
    pub members: Vec<Member>,
    member_by_name: HashMap<Arc<str>, RelId>,
}

impl TypeDef {
    pub(crate) fn new(
        id: TypeId,
        name: Arc<str>,
        doc: Option<String>,
        members: Vec<Member>,
    ) -> Self {
        let member_by_name = members
            .iter()
            .enumerate()
            .map(|(i, m)| (m.name_arc().clone(), RelId(i)))
            .collect();
        Self {
            id,
            name,
            doc,
            members,
            member_by_name,
        }
    }

    /// The id of the relation or permission named `name`.
    pub fn member_id(&self, name: &str) -> Option<RelId> {
        self.member_by_name.get(name).copied()
    }

    /// The member with id `id`.
    pub fn member(&self, id: RelId) -> Option<&Member> {
        self.members.get(id.0)
    }
}

/// A relation or permission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Member {
    /// Stored relationships.
    Relation(RelationDef),
    /// Computed from an expression.
    Permission(PermissionDef),
}

impl Member {
    /// The member's name.
    pub fn name(&self) -> &str {
        self.name_arc()
    }

    fn name_arc(&self) -> &Arc<str> {
        match self {
            Self::Relation(r) => &r.name,
            Self::Permission(p) => &p.name,
        }
    }

    /// The member's doc comment text.
    pub fn doc(&self) -> Option<&str> {
        match self {
            Self::Relation(r) => r.doc.as_deref(),
            Self::Permission(p) => p.doc.as_deref(),
        }
    }
}

/// `relation name: allowed | ...`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelationDef {
    /// Relation name.
    pub name: Arc<str>,
    /// Doc comment text.
    pub doc: Option<String>,
    /// Allowed subjects in source order; never empty, no duplicates.
    pub allowed: Vec<AllowedSubject>,
}

/// One allowed subject form on a relation.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum AllowedSubject {
    /// `user`: a concrete object.
    Object(TypeId),
    /// `group#member`: a subject set.
    Set(TypeId, RelId),
    /// `user:*`: every object of the type.
    Wildcard(TypeId),
}

/// `permission name = expr`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionDef {
    /// Permission name.
    pub name: Arc<str>,
    /// Doc comment text.
    pub doc: Option<String>,
    /// The permission's definition.
    pub expr: Expr,
}

/// A resolved permission expression (`docs/03-schema-language.md` §4.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    /// A relation or permission on the same object.
    Computed(RelId),
    /// Walk `tupleset` to related objects and evaluate `computed` on each.
    Arrow {
        /// A relation (never a permission) on the same type.
        tupleset: RelId,
        /// Name evaluated on each related object; resolves per related type.
        computed: Arc<str>,
        /// Union (`->`, `.any`) or intersection (`.all`) over related objects.
        mode: ArrowMode,
        /// For each type allowed on `tupleset` that has `computed`, its id there. Related
        /// objects of other types contribute nothing (§4.2).
        targets: BTreeMap<TypeId, RelId>,
    },
    /// Union of at least two operands, none of them a union.
    Union(Vec<Expr>),
    /// Intersection of at least two operands, none of them an intersection.
    Intersection(Vec<Expr>),
    /// `base` minus `sub`; never flattened.
    Exclusion {
        /// Subjects to start from.
        base: Box<Expr>,
        /// Subjects to remove.
        sub: Box<Expr>,
    },
}

impl Expr {
    /// Calls `f` on this node and every node below it, parents before children.
    pub fn visit<'a>(&'a self, f: &mut impl FnMut(&'a Expr)) {
        f(self);
        match self {
            Self::Computed(_) | Self::Arrow { .. } => {}
            Self::Union(xs) | Self::Intersection(xs) => {
                for x in xs {
                    x.visit(f);
                }
            }
            Self::Exclusion { base, sub } => {
                base.visit(f);
                sub.visit(f);
            }
        }
    }
}
