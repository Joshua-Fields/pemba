//! Syntax tree produced by [`crate::parse`]. Every node carries the byte [`Span`] it was
//! parsed from.
//!
//! The tree is faithful to the source: names are not yet checked against the identifier
//! rules, references are not resolved, and same-operator nesting through parentheses
//! (`(a + b) + c`) is kept. Semantic validation and flattening happen when the AST is
//! compiled to IR (`docs/03-schema-language.md` §5–6).

use crate::span::Span;

/// A whole schema file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Schema {
    /// Definitions in source order.
    pub definitions: Vec<Definition>,
}

/// A `/** ... */` comment attached to the node that follows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocComment {
    /// Comment text with the delimiters, leading `*` gutters and surrounding blank lines
    /// removed. Consecutive doc comments are joined with `\n`.
    pub text: String,
    /// From the first `/**` to the last `*/`.
    pub span: Span,
}

/// A relation, permission or type-name segment as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ident {
    /// The name.
    pub name: String,
    /// Where it was written.
    pub span: Span,
}

/// A possibly prefixed type name such as `user` or `docs/document`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeName {
    /// The full name including any `/` prefixes.
    pub name: String,
    /// Where it was written.
    pub span: Span,
}

/// `definition <name> { ... }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Definition {
    /// Attached doc comment.
    pub doc: Option<DocComment>,
    /// The type being defined.
    pub name: TypeName,
    /// Relations and permissions in source order.
    pub members: Vec<Member>,
    /// From `definition` to the closing `}` (the doc comment is not included).
    pub span: Span,
}

/// A member of a definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Member {
    /// `relation ...`.
    Relation(Relation),
    /// `permission ...`.
    Permission(Permission),
}

/// `relation <name>: <allowed> | <allowed> ...`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relation {
    /// Attached doc comment.
    pub doc: Option<DocComment>,
    /// Relation name.
    pub name: Ident,
    /// Allowed subject types, in source order; never empty.
    pub allowed: Vec<AllowedType>,
    /// From `relation` to the end of the last allowed type.
    pub span: Span,
}

/// One allowed subject type on a relation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllowedType {
    /// The subject's object type.
    pub type_name: TypeName,
    /// Plain object, subject set or wildcard.
    pub kind: AllowedKind,
    /// The whole allowed type, such as `group#member`.
    pub span: Span,
}

/// The form of an [`AllowedType`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AllowedKind {
    /// `user`: a concrete object of the type.
    Object,
    /// `group#member`: a subject set; holds the subject relation.
    SubjectSet(Ident),
    /// `user:*`: every object of the type.
    Wildcard,
}

/// `permission <name> = <expr>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Permission {
    /// Attached doc comment.
    pub doc: Option<DocComment>,
    /// Permission name.
    pub name: Ident,
    /// The permission's definition.
    pub expr: Expr,
    /// From `permission` to the end of the expression.
    pub span: Span,
}

/// A permission expression node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expr {
    /// What the node is.
    pub kind: ExprKind,
    /// Source range; for a parenthesised expression it includes the parentheses.
    pub span: Span,
}

/// The kinds of [`Expr`] (`docs/03-schema-language.md` §4.1).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExprKind {
    /// `r`: a relation or permission on the same object.
    Computed(Ident),
    /// `t->p`, `t.any(p)` or `t.all(p)`.
    Arrow {
        /// The relation walked to reach related objects.
        tupleset: Ident,
        /// The relation or permission evaluated on each related object.
        computed: Ident,
        /// Whether any or all related objects must grant it.
        mode: ArrowMode,
    },
    /// `a + b + ...`, at least two operands.
    Union(Vec<Expr>),
    /// `a & b & ...`, at least two operands.
    Intersection(Vec<Expr>),
    /// `base - sub`; `a - b - c` nests to the left.
    Exclusion {
        /// Subjects to start from.
        base: Box<Expr>,
        /// Subjects to remove.
        sub: Box<Expr>,
    },
}

/// How an arrow combines its related objects.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ArrowMode {
    /// `->` or `.any()`: union over related objects.
    Any,
    /// `.all()`: intersection over related objects.
    All,
}
