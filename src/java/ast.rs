//! Declaration-level syntax tree of a Java compilation unit. Method bodies and initializers are not kept,
//! except the initializer expressions of fields (needed to evaluate constants used in annotations).

#[derive(Debug, Clone)]
pub struct CompilationUnit {
    pub package: String,
    pub imports: Vec<Import>,
    pub types: Vec<TypeDecl>,
}

#[derive(Debug, Clone)]
pub struct Import {
    pub is_static: bool,
    pub name: String,
    pub wildcard: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeKind {
    Class,
    Interface,
    Enum,
    Record,
    Annotation,
}

#[derive(Debug, Clone, Default)]
pub struct Modifiers {
    pub flags: Vec<String>,
    pub annotations: Vec<Annotation>,
}

impl Modifiers {
    pub fn has(&self, flag: &str) -> bool {
        self.flags.iter().any(|f| f == flag)
    }
}

#[derive(Debug, Clone)]
pub struct TypeDecl {
    pub kind: TypeKind,
    pub name: String,
    pub modifiers: Modifiers,
    pub type_params: Vec<TypeParam>,
    pub extends: Vec<TypeRef>,
    pub implements: Vec<TypeRef>,
    pub record_components: Vec<Param>,
    pub enum_constants: Vec<EnumConstant>,
    pub fields: Vec<FieldDecl>,
    pub methods: Vec<MethodDecl>,
    pub types: Vec<TypeDecl>,
    /// Declaration order of fields and methods, as (is_field, index).
    pub member_order: Vec<(bool, usize)>,
}

#[derive(Debug, Clone)]
pub struct TypeParam {
    pub name: String,
    pub bounds: Vec<TypeRef>,
}

#[derive(Debug, Clone)]
pub struct EnumConstant {
    pub name: String,
    pub annotations: Vec<Annotation>,
}

#[derive(Debug, Clone)]
pub struct FieldDecl {
    pub modifiers: Modifiers,
    pub ty: TypeRef,
    pub name: String,
    pub init: Option<Expr>,
}

#[derive(Debug, Clone)]
pub struct MethodDecl {
    pub modifiers: Modifiers,
    pub type_params: Vec<TypeParam>,
    /// `None` for constructors.
    pub return_type: Option<TypeRef>,
    pub name: String,
    pub params: Vec<Param>,
    pub has_body: bool,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub modifiers: Modifiers,
    pub ty: TypeRef,
    pub name: String,
}

/// A type as written in source.
#[derive(Debug, Clone)]
pub enum TypeRef {
    /// Possibly qualified class type, e.g. `Map.Entry<K, V>`; each segment may have type arguments.
    Class {
        segments: Vec<(String, Vec<TypeArg>)>,
        annotations: Vec<Annotation>,
    },
    Primitive {
        name: String,
        annotations: Vec<Annotation>,
    },
    Array {
        component: Box<TypeRef>,
        annotations: Vec<Annotation>,
    },
    Void,
}

#[derive(Debug, Clone)]
pub enum TypeArg {
    Type(TypeRef),
    Wildcard {
        extends: Option<TypeRef>,
        super_: Option<TypeRef>,
        annotations: Vec<Annotation>,
    },
}

#[derive(Debug, Clone)]
pub struct Annotation {
    pub name: String,
    /// Element name/value pairs. A single unnamed value is stored as `value`.
    pub args: Vec<(String, Expr)>,
}

#[derive(Debug, Clone)]
pub enum Expr {
    Str(String),
    Char(char),
    Num(String),
    Bool(bool),
    Null,
    /// Dotted name, e.g. `MediaType.APPLICATION_JSON` or `FOO`.
    Name(String),
    ClassLit(TypeRef),
    Array(Vec<Expr>),
    Annotation(Box<Annotation>),
    Binary(Box<Expr>, &'static str, Box<Expr>),
    Unary(&'static str, Box<Expr>),
    /// Cast to a primitive type or `String`; the value is kept as is.
    Cast(Box<Expr>),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    /// Anything we do not model (method calls, `new`, lambdas, ...).
    Other,
}
