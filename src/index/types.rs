//! Jandex-like type and annotation model.

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Ty {
    Class(String),
    Prim(&'static str),
    /// Non-array constituent and number of dimensions.
    Array(Box<Type>, u32),
    Param {
        name: String,
        args: Vec<Type>,
        owner: Option<Box<Type>>,
    },
    Var {
        id: String,
        bounds: Vec<Type>,
    },
    Wildcard {
        extends: Option<Box<Type>>,
        super_: Option<Box<Type>>,
    },
    Void,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Type {
    pub ty: Ty,
    pub annotations: Vec<Anno>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Class,
    Primitive,
    Array,
    Parameterized,
    TypeVariable,
    Wildcard,
    Void,
}

pub const OBJECT: &str = "java.lang.Object";

impl Type {
    pub fn new(ty: Ty) -> Type {
        Type {
            ty,
            annotations: Vec::new(),
        }
    }
    pub fn class(name: &str) -> Type {
        Type::new(Ty::Class(name.to_string()))
    }
    pub fn kind(&self) -> Kind {
        match &self.ty {
            Ty::Class(_) => Kind::Class,
            Ty::Prim(_) => Kind::Primitive,
            Ty::Array(..) => Kind::Array,
            Ty::Param { .. } => Kind::Parameterized,
            Ty::Var { .. } => Kind::TypeVariable,
            Ty::Wildcard { .. } => Kind::Wildcard,
            Ty::Void => Kind::Void,
        }
    }
    /// Jandex `Type.name()`.
    pub fn name(&self) -> String {
        match &self.ty {
            Ty::Class(n) => n.clone(),
            Ty::Prim(p) => p.to_string(),
            Ty::Array(c, dims) => {
                let mut s = "[".repeat(*dims as usize);
                match &c.ty {
                    Ty::Prim(p) => s.push(prim_descriptor(p)),
                    _ => {
                        s.push('L');
                        s.push_str(&c.name());
                        s.push(';');
                    }
                }
                s
            }
            Ty::Param { name, .. } => name.clone(),
            Ty::Var { bounds, .. } => bounds
                .first()
                .map(|b| b.name())
                .unwrap_or_else(|| OBJECT.to_string()),
            Ty::Wildcard { extends, .. } => extends
                .as_ref()
                .map(|b| b.name())
                .unwrap_or_else(|| OBJECT.to_string()),
            Ty::Void => "void".to_string(),
        }
    }
    /// Jandex `DotName.local()` of `name()` for indexed classes: innermost simple name.
    pub fn local_name(&self) -> String {
        local_of(&self.name())
    }
    pub fn args(&self) -> &[Type] {
        match &self.ty {
            Ty::Param { args, .. } => args,
            _ => &[],
        }
    }
}

pub fn local_of(name: &str) -> String {
    let after_dot = name.rsplit('.').next().unwrap_or(name);
    after_dot
        .rsplit('$')
        .next()
        .unwrap_or(after_dot)
        .to_string()
}

fn prim_descriptor(p: &str) -> char {
    match p {
        "boolean" => 'Z',
        "byte" => 'B',
        "char" => 'C',
        "short" => 'S',
        "int" => 'I',
        "long" => 'J',
        "float" => 'F',
        "double" => 'D',
        _ => 'V',
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.ty {
            Ty::Param { name, args, .. } => {
                write!(f, "{name}<")?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{a}")?;
                }
                write!(f, ">")
            }
            Ty::Array(c, d) => write!(f, "{c}{}", "[]".repeat(*d as usize)),
            Ty::Var { id, .. } => write!(f, "{id}"),
            Ty::Wildcard { extends, super_ } => match (extends, super_) {
                (_, Some(s)) => write!(f, "? super {s}"),
                (Some(e), _) => write!(f, "? extends {e}"),
                _ => write!(f, "?"),
            },
            _ => write!(f, "{}", self.name()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AVal {
    Str(String),
    Bool(bool),
    Int(i64),
    /// Floating point value, kept as text.
    Float(String),
    Char(char),
    Class(Type),
    Enum {
        ty: String,
        value: String,
    },
    Nested(Box<Anno>),
    Array(Vec<AVal>),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Anno {
    pub name: String,
    pub values: Vec<(String, AVal)>,
}

impl Anno {
    pub fn value(&self, key: &str) -> Option<&AVal> {
        self.values.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub fn str(&self, key: &str) -> Option<String> {
        self.value(key).and_then(|v| v.as_str())
    }
    pub fn bool(&self, key: &str) -> Option<bool> {
        match self.value(key) {
            Some(AVal::Bool(b)) => Some(*b),
            _ => None,
        }
    }
    pub fn int(&self, key: &str) -> Option<i64> {
        match self.value(key) {
            Some(AVal::Int(i)) => Some(*i),
            Some(AVal::Char(c)) => Some(*c as i64),
            _ => None,
        }
    }
    pub fn string_array(&self, key: &str) -> Option<Vec<String>> {
        self.value(key).map(|v| v.as_string_array())
    }
    pub fn class(&self, key: &str) -> Option<Type> {
        match self.value(key) {
            Some(AVal::Class(t)) => Some(t.clone()),
            _ => None,
        }
    }
    pub fn enum_value(&self, key: &str) -> Option<String> {
        match self.value(key) {
            Some(AVal::Enum { value, .. }) => Some(value.clone()),
            _ => None,
        }
    }
    pub fn nested(&self, key: &str) -> Option<&Anno> {
        match self.value(key) {
            Some(AVal::Nested(a)) => Some(a),
            _ => None,
        }
    }
    pub fn nested_array(&self, key: &str) -> Vec<Anno> {
        match self.value(key) {
            Some(AVal::Array(items)) => items
                .iter()
                .filter_map(|i| match i {
                    AVal::Nested(a) => Some((**a).clone()),
                    _ => None,
                })
                .collect(),
            Some(AVal::Nested(a)) => vec![(**a).clone()],
            _ => Vec::new(),
        }
    }
    pub fn class_array(&self, key: &str) -> Vec<Type> {
        match self.value(key) {
            Some(AVal::Array(items)) => items
                .iter()
                .filter_map(|i| match i {
                    AVal::Class(t) => Some(t.clone()),
                    _ => None,
                })
                .collect(),
            Some(AVal::Class(t)) => vec![t.clone()],
            _ => Vec::new(),
        }
    }
}

impl AVal {
    pub fn as_str(&self) -> Option<String> {
        match self {
            AVal::Str(s) => Some(s.clone()),
            AVal::Array(items) if items.len() == 1 => items[0].as_str(),
            AVal::Enum { value, .. } => Some(value.clone()),
            _ => None,
        }
    }
    pub fn as_string_array(&self) -> Vec<String> {
        match self {
            AVal::Array(items) => items.iter().filter_map(|i| i.as_str()).collect(),
            other => other.as_str().into_iter().collect(),
        }
    }
}
