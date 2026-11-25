//! Class index built from Java sources, mirroring what Jandex would read from the compiled classes.

pub mod known;
pub mod types;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::java::ast::{self, Expr, TypeArg, TypeDecl, TypeKind, TypeRef};
use known::ElementKind;
pub use types::*;

pub type ClassId = usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MethodId {
    pub class: ClassId,
    pub idx: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FieldId {
    pub class: ClassId,
    pub idx: usize,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Flags {
    pub public: bool,
    pub private: bool,
    pub static_: bool,
    pub abstract_: bool,
    pub final_: bool,
    pub interface: bool,
}

#[derive(Clone, Debug)]
pub struct ClassInfo {
    pub name: String,
    pub simple_name: String,
    pub kind: TypeKind,
    pub flags: Flags,
    pub type_params: Vec<Type>,
    pub super_type: Option<Type>,
    pub interfaces: Vec<Type>,
    pub annotations: Vec<Anno>,
    pub fields: Vec<FieldInfo>,
    pub methods: Vec<MethodInfo>,
    pub record_components: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct FieldInfo {
    pub name: String,
    pub ty: Type,
    pub flags: Flags,
    pub annotations: Vec<Anno>,
    pub enum_constant: bool,
}

#[derive(Clone, Debug)]
pub struct ParamInfo {
    pub name: String,
    pub ty: Type,
    pub annotations: Vec<Anno>,
}

#[derive(Clone, Debug)]
pub struct MethodInfo {
    pub name: String,
    pub params: Vec<ParamInfo>,
    pub return_type: Type,
    pub flags: Flags,
    pub annotations: Vec<Anno>,
}

impl ClassInfo {
    pub fn is_record(&self) -> bool {
        self.kind == TypeKind::Record
    }
    pub fn is_interface(&self) -> bool {
        matches!(self.kind, TypeKind::Interface | TypeKind::Annotation)
    }
    pub fn annotation(&self, name: &str) -> Option<&Anno> {
        self.annotations.iter().find(|a| a.name == name)
    }
    pub fn record_component(&self, name: &str) -> bool {
        self.record_components.iter().any(|c| c == name)
    }
}

/// Where an annotation instance is placed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    Class(ClassId),
    Field(FieldId),
    Method(MethodId),
    Param(MethodId, usize),
}

struct Unit {
    cu: ast::CompilationUnit,
}

/// Locates a type declaration inside a compilation unit.
#[derive(Clone)]
struct DeclLoc {
    unit: usize,
    path: Vec<usize>,
    enclosing: Option<ClassId>,
}

pub struct Index {
    pub classes: Vec<ClassInfo>,
    by_name: HashMap<String, ClassId>,
    /// Classes from outside the sources that the index should report as present (e.g. JDK types Quarkus
    /// happens to index), with their kind.
    extra: HashSet<String>,
    units: Vec<Unit>,
    locs: Vec<DeclLoc>,
    source_names: HashMap<String, String>,
    const_cache: RefCell<HashMap<(ClassId, String), Option<AVal>>>,
    const_in_progress: RefCell<HashSet<(ClassId, String)>>,
    pub warnings: RefCell<Vec<String>>,
}

#[derive(Clone)]
struct Scope<'a> {
    unit: usize,
    class: Option<ClassId>,
    method_type_params: &'a [ast::TypeParam],
}

fn java_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?
        .flatten()
        .map(|e| e.path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            java_files(&p, out)?;
        } else if p.extension().is_some_and(|e| e == "java") {
            out.push(p);
        }
    }
    Ok(())
}

const JAVA_LANG: &[&str] = &[
    "Object",
    "String",
    "Integer",
    "Long",
    "Short",
    "Byte",
    "Double",
    "Float",
    "Boolean",
    "Character",
    "Void",
    "Number",
    "Enum",
    "Record",
    "Iterable",
    "Comparable",
    "Deprecated",
    "Override",
    "FunctionalInterface",
    "SuppressWarnings",
    "SafeVarargs",
    "Class",
    "Exception",
    "RuntimeException",
    "Throwable",
    "Error",
    "CharSequence",
    "StringBuilder",
    "StringBuffer",
    "Math",
    "System",
    "Thread",
    "Runnable",
    "AutoCloseable",
    "Cloneable",
    "IllegalArgumentException",
    "IllegalStateException",
    "UnsupportedOperationException",
    "NullPointerException",
    "IndexOutOfBoundsException",
    "ClassCastException",
    "ArithmeticException",
    "InterruptedException",
    "SecurityException",
    "Process",
    "ProcessBuilder",
    "Runtime",
    "ThreadLocal",
    "StackWalker",
    "Character",
    "Appendable",
    "Readable",
];

impl Index {
    pub fn load(roots: &[PathBuf], extra: &[&str]) -> Result<Index, String> {
        let mut files = Vec::new();
        for root in roots {
            java_files(root, &mut files).map_err(|e| format!("{}: {e}", root.display()))?;
        }
        let mut units = Vec::new();
        for f in &files {
            let src = std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?;
            let cu = crate::java::parse(&src).map_err(|e| format!("{}: {e}", f.display()))?;
            units.push(Unit { cu });
        }
        let mut index = Index {
            classes: Vec::new(),
            by_name: HashMap::new(),
            extra: extra.iter().map(|s| s.to_string()).collect(),
            units,
            locs: Vec::new(),
            source_names: HashMap::new(),
            const_cache: RefCell::new(HashMap::new()),
            const_in_progress: RefCell::new(HashSet::new()),
            warnings: RefCell::new(Vec::new()),
        };
        // first pass: names
        for (u, unit) in index.units.iter().enumerate() {
            let pkg = unit.cu.package.clone();
            for (i, decl) in unit.cu.types.iter().enumerate() {
                let prefix = if pkg.is_empty() {
                    String::new()
                } else {
                    format!("{pkg}.")
                };
                register_names(
                    decl,
                    &format!("{prefix}{}", decl.name),
                    &format!("{prefix}{}", decl.name),
                    u,
                    (vec![i], None),
                    &mut index.locs,
                    &mut index.by_name,
                    &mut index.source_names,
                );
            }
        }
        // placeholders so that ids are stable while resolving
        let count = index.locs.len();
        let mut names = vec![String::new(); count];
        for (k, v) in &index.by_name {
            names[*v] = k.clone();
        }
        for (id, name) in names.into_iter().enumerate() {
            let decl = index.decl(id).clone();
            index.classes.push(ClassInfo {
                simple_name: decl.name.clone(),
                name,
                kind: decl.kind,
                flags: Flags::default(),
                type_params: Vec::new(),
                super_type: None,
                interfaces: Vec::new(),
                annotations: Vec::new(),
                fields: Vec::new(),
                methods: Vec::new(),
                record_components: decl
                    .record_components
                    .iter()
                    .map(|c| c.name.clone())
                    .collect(),
            });
        }
        // second pass: types, members, annotations
        for id in 0..count {
            let info = index.build_class(id);
            index.classes[id] = info;
        }
        Ok(index)
    }

    fn decl(&self, id: ClassId) -> &TypeDecl {
        let loc = &self.locs[id];
        let mut decl = &self.units[loc.unit].cu.types[loc.path[0]];
        for i in &loc.path[1..] {
            decl = &decl.types[*i];
        }
        decl
    }

    fn enclosing(&self, id: ClassId) -> Option<ClassId> {
        self.locs[id].enclosing
    }

    /// Adds classes from outside the sources that the index should report as present.
    pub fn add_extra(&mut self, names: &[String]) {
        self.extra.extend(names.iter().cloned());
    }

    pub fn id(&self, name: &str) -> Option<ClassId> {
        self.by_name.get(name).copied()
    }

    pub fn class(&self, name: &str) -> Option<&ClassInfo> {
        self.id(name).map(|id| &self.classes[id])
    }

    /// `IndexView.getClassByName != null`, including extra (non-source) classes.
    pub fn contains(&self, name: &str) -> bool {
        self.by_name.contains_key(name) || self.extra.contains(name)
    }

    pub fn method(&self, id: MethodId) -> &MethodInfo {
        &self.classes[id.class].methods[id.idx]
    }

    pub fn field(&self, id: FieldId) -> &FieldInfo {
        &self.classes[id.class].fields[id.idx]
    }

    pub fn warn(&self, msg: String) {
        self.warnings.borrow_mut().push(msg);
    }

    // ---------------------------------------------------------------- class building

    fn build_class(&self, id: ClassId) -> ClassInfo {
        let decl = self.decl(id);
        let unit = self.locs[id].unit;
        let scope = Scope {
            unit,
            class: Some(id),
            method_type_params: &[],
        };
        let enclosing = self.enclosing(id);
        let enclosing_is_interface =
            enclosing.is_some_and(|e| self.decl(e).kind == TypeKind::Interface);
        let mut flags = flags_of(&decl.modifiers);
        // nested interfaces, enums, records and members of interfaces are implicitly static
        if (decl.kind != TypeKind::Class || enclosing_is_interface) && enclosing.is_some() {
            flags.static_ = true;
        }
        if enclosing_is_interface {
            flags.public = true;
        }
        match decl.kind {
            TypeKind::Interface | TypeKind::Annotation => {
                flags.interface = true;
                flags.abstract_ = true;
            }
            TypeKind::Record | TypeKind::Enum => flags.final_ = true,
            _ => {}
        }
        let type_params = decl
            .type_params
            .iter()
            .map(|tp| self.type_var(tp, &scope, 0))
            .collect();
        let super_type = match decl.kind {
            TypeKind::Class => Some(
                decl.extends
                    .first()
                    .map(|t| self.resolve_type(t, &scope))
                    .unwrap_or_else(|| Type::class(OBJECT)),
            ),
            TypeKind::Enum => Some(Type::new(Ty::Param {
                name: "java.lang.Enum".into(),
                args: vec![Type::class(&self.classes[id].name)],
                owner: None,
            })),
            TypeKind::Record => Some(Type::class("java.lang.Record")),
            TypeKind::Interface | TypeKind::Annotation => Some(Type::class(OBJECT)),
        };
        let interfaces: Vec<Type> = if decl.kind == TypeKind::Interface {
            decl.extends
                .iter()
                .map(|t| self.resolve_type(t, &scope))
                .collect()
        } else {
            decl.implements
                .iter()
                .map(|t| self.resolve_type(t, &scope))
                .collect()
        };
        let (annotations, _) =
            self.split_annotations(&decl.modifiers.annotations, ElementKind::Type, &scope);

        let mut fields = Vec::new();
        let mut methods = Vec::new();
        // enum constants come first in the class file
        for c in &decl.enum_constants {
            let (annos, _) = self.split_annotations(&c.annotations, ElementKind::Field, &scope);
            fields.push(FieldInfo {
                name: c.name.clone(),
                ty: Type::class(&self.classes[id].name),
                flags: Flags {
                    public: true,
                    static_: true,
                    final_: true,
                    ..Flags::default()
                },
                annotations: annos,
                enum_constant: true,
            });
        }
        // record components become private final fields
        let mut component_method_annos = Vec::new();
        for c in &decl.record_components {
            let ty = self.resolve_type(&c.ty, &scope);
            let (field_annos, type_annos) =
                self.split_annotations(&c.modifiers.annotations, ElementKind::Field, &scope);
            let (method_annos, _) =
                self.split_annotations(&c.modifiers.annotations, ElementKind::Method, &scope);
            let ty = with_type_annotations(ty, type_annos.clone());
            fields.push(FieldInfo {
                name: c.name.clone(),
                ty: ty.clone(),
                flags: Flags {
                    private: true,
                    final_: true,
                    ..Flags::default()
                },
                annotations: field_annos,
                enum_constant: false,
            });
            component_method_annos.push((c.name.clone(), ty, method_annos));
        }
        for (is_field, idx) in &decl.member_order {
            if *is_field {
                let f = &decl.fields[*idx];
                let ty = self.resolve_type(&f.ty, &scope);
                let (annos, type_annos) =
                    self.split_annotations(&f.modifiers.annotations, ElementKind::Field, &scope);
                let mut fl = flags_of(&f.modifiers);
                if decl.kind == TypeKind::Interface || decl.kind == TypeKind::Annotation {
                    fl.public = true;
                    fl.static_ = true;
                    fl.final_ = true;
                }
                fields.push(FieldInfo {
                    name: f.name.clone(),
                    ty: with_type_annotations(ty, type_annos),
                    flags: fl,
                    annotations: annos,
                    enum_constant: false,
                });
            } else {
                let m = &decl.methods[*idx];
                let mscope = Scope {
                    unit,
                    class: Some(id),
                    method_type_params: &m.type_params,
                };
                let mut fl = flags_of(&m.modifiers);
                if decl.kind == TypeKind::Interface || decl.kind == TypeKind::Annotation {
                    if !fl.private {
                        fl.public = true;
                    }
                    if !m.has_body && !fl.static_ {
                        fl.abstract_ = true;
                    }
                }
                let (annos, type_annos) = if m.return_type.is_some() {
                    self.split_annotations(&m.modifiers.annotations, ElementKind::Method, &mscope)
                } else {
                    self.split_annotations(
                        &m.modifiers.annotations,
                        ElementKind::Constructor,
                        &mscope,
                    )
                };
                let return_type = match &m.return_type {
                    Some(t) => with_type_annotations(self.resolve_type(t, &mscope), type_annos),
                    None => Type::new(Ty::Void),
                };
                let params = m
                    .params
                    .iter()
                    .map(|p| {
                        let ty = self.resolve_type(&p.ty, &mscope);
                        let (annos, type_annos) = self.split_annotations(
                            &p.modifiers.annotations,
                            ElementKind::Parameter,
                            &mscope,
                        );
                        ParamInfo {
                            name: p.name.clone(),
                            ty: with_type_annotations(ty, type_annos),
                            annotations: annos,
                        }
                    })
                    .collect();
                methods.push(MethodInfo {
                    name: m.name.clone(),
                    params,
                    return_type,
                    flags: fl,
                    annotations: annos,
                });
            }
        }
        // implicit record accessors
        for (name, ty, annos) in component_method_annos {
            let explicit = methods
                .iter()
                .any(|m| m.name == name && m.params.is_empty());
            if !explicit {
                methods.push(MethodInfo {
                    name,
                    params: Vec::new(),
                    return_type: ty,
                    flags: Flags {
                        public: true,
                        ..Flags::default()
                    },
                    annotations: annos,
                });
            }
        }
        ClassInfo {
            name: self.classes[id].name.clone(),
            simple_name: decl.name.clone(),
            kind: decl.kind,
            flags,
            type_params,
            super_type,
            interfaces,
            annotations,
            fields,
            methods,
            record_components: decl
                .record_components
                .iter()
                .map(|c| c.name.clone())
                .collect(),
        }
    }

    /// Splits source annotations into declaration annotations (for `kind`) and type annotations.
    fn split_annotations(
        &self,
        annos: &[ast::Annotation],
        kind: ElementKind,
        scope: &Scope,
    ) -> (Vec<Anno>, Vec<Anno>) {
        let mut decl = Vec::new();
        let mut type_use = Vec::new();
        for a in annos {
            let name = self.resolve_class_name(&a.name, scope);
            let targets = self.annotation_targets(&name);
            let (is_decl, is_type) = match &targets {
                None => (true, false),
                Some(t) => (t.contains(&kind), t.contains(&ElementKind::TypeUse)),
            };
            if !is_decl && !is_type {
                continue;
            }
            let converted = self.convert_annotation(a, &name, scope);
            if is_decl {
                decl.push(converted.clone());
            }
            if is_type {
                type_use.push(converted);
            }
        }
        (decl, type_use)
    }

    fn annotation_targets(&self, name: &str) -> Option<Vec<ElementKind>> {
        if let Some(id) = self.id(name) {
            let decl = self.decl(id);
            let target = decl
                .modifiers
                .annotations
                .iter()
                .find(|a| a.name == "Target" || a.name.ends_with(".Target"))?;
            let mut kinds = Vec::new();
            let mut collect = |e: &Expr| {
                if let Expr::Name(n) = e {
                    let last = n.rsplit('.').next().unwrap_or(n);
                    kinds.push(match last {
                        "TYPE" => ElementKind::Type,
                        "FIELD" => ElementKind::Field,
                        "METHOD" => ElementKind::Method,
                        "PARAMETER" => ElementKind::Parameter,
                        "CONSTRUCTOR" => ElementKind::Constructor,
                        "RECORD_COMPONENT" => ElementKind::RecordComponent,
                        "TYPE_USE" => ElementKind::TypeUse,
                        "ANNOTATION_TYPE" => ElementKind::AnnotationType,
                        _ => return,
                    });
                }
            };
            for (_, v) in &target.args {
                match v {
                    Expr::Array(items) => items.iter().for_each(&mut collect),
                    other => collect(other),
                }
            }
            return Some(kinds);
        }
        known::annotation_targets(name).map(|t| t.to_vec())
    }

    fn convert_annotation(&self, a: &ast::Annotation, name: &str, scope: &Scope) -> Anno {
        let mut values = Vec::new();
        for (k, e) in &a.args {
            match self.eval(e, scope) {
                Some(v) => values.push((k.clone(), v)),
                None => self.warn(format!("cannot evaluate @{}({k} = {e:?})", a.name)),
            }
        }
        Anno {
            name: name.to_string(),
            values,
        }
    }

    // ---------------------------------------------------------------- type resolution

    fn type_var(&self, tp: &ast::TypeParam, scope: &Scope, depth: usize) -> Type {
        let bounds = if depth > 2 {
            Vec::new()
        } else {
            tp.bounds
                .iter()
                .map(|b| self.resolve_type_depth(b, scope, depth + 1))
                .collect()
        };
        Type::new(Ty::Var {
            id: tp.name.clone(),
            bounds,
        })
    }

    fn resolve_type(&self, t: &TypeRef, scope: &Scope) -> Type {
        self.resolve_type_depth(t, scope, 0)
    }

    fn resolve_type_depth(&self, t: &TypeRef, scope: &Scope, depth: usize) -> Type {
        match t {
            TypeRef::Void => Type::new(Ty::Void),
            TypeRef::Primitive { name, annotations } => {
                let p: &'static str = match name.as_str() {
                    "boolean" => "boolean",
                    "byte" => "byte",
                    "char" => "char",
                    "short" => "short",
                    "int" => "int",
                    "long" => "long",
                    "float" => "float",
                    "double" => "double",
                    _ => "void",
                };
                Type {
                    ty: Ty::Prim(p),
                    annotations: self.type_annos(annotations, scope),
                }
            }
            TypeRef::Array {
                component,
                annotations,
            } => {
                let c = self.resolve_type_depth(component, scope, depth);
                let annos = self.type_annos(annotations, scope);
                match c.ty {
                    Ty::Array(inner, dims) => Type {
                        ty: Ty::Array(inner, dims + 1),
                        annotations: annos,
                    },
                    _ => Type {
                        ty: Ty::Array(Box::new(c), 1),
                        annotations: annos,
                    },
                }
            }
            TypeRef::Class {
                segments,
                annotations,
            } => {
                let annos = self.type_annos(annotations, scope);
                // type variable
                if segments.len() == 1 && segments[0].1.is_empty() {
                    let n = &segments[0].0;
                    if let Some(tp) = scope.method_type_params.iter().find(|tp| &tp.name == n) {
                        let mut v = self.type_var(tp, scope, depth);
                        v.annotations = annos;
                        return v;
                    }
                    let mut cls = scope.class;
                    while let Some(c) = cls {
                        let decl = self.decl(c);
                        if let Some(tp) = decl.type_params.iter().find(|tp| &tp.name == n) {
                            let cscope = Scope {
                                unit: scope.unit,
                                class: Some(c),
                                method_type_params: &[],
                            };
                            let mut v = self.type_var(tp, &cscope, depth);
                            v.annotations = annos;
                            return v;
                        }
                        if decl.modifiers.has("static") || decl.kind != TypeKind::Class {
                            break;
                        }
                        cls = self.enclosing(c);
                    }
                }
                let names: Vec<&str> = segments.iter().map(|(n, _)| n.as_str()).collect();
                let name = self.resolve_segments(&names, scope);
                let args_ref = &segments.last().unwrap().1;
                if args_ref.is_empty() {
                    Type {
                        ty: Ty::Class(name),
                        annotations: annos,
                    }
                } else {
                    let args = args_ref
                        .iter()
                        .map(|a| match a {
                            TypeArg::Type(t) => self.resolve_type_depth(t, scope, depth),
                            TypeArg::Wildcard {
                                extends,
                                super_,
                                annotations,
                            } => {
                                let ext = extends
                                    .as_ref()
                                    .map(|t| self.resolve_type_depth(t, scope, depth))
                                    .unwrap_or_else(|| Type::class(OBJECT));
                                let (ext, sup) = match super_ {
                                    Some(s) => (
                                        Some(Box::new(Type::class(OBJECT))),
                                        Some(Box::new(self.resolve_type_depth(s, scope, depth))),
                                    ),
                                    None => (Some(Box::new(ext)), None),
                                };
                                Type {
                                    ty: Ty::Wildcard {
                                        extends: ext,
                                        super_: sup,
                                    },
                                    annotations: self.type_annos(annotations, scope),
                                }
                            }
                        })
                        .collect();
                    Type {
                        ty: Ty::Param {
                            name,
                            args,
                            owner: None,
                        },
                        annotations: annos,
                    }
                }
            }
        }
    }

    fn type_annos(&self, annos: &[ast::Annotation], scope: &Scope) -> Vec<Anno> {
        annos
            .iter()
            .map(|a| {
                let name = self.resolve_class_name(&a.name, scope);
                self.convert_annotation(a, &name, scope)
            })
            .collect()
    }

    /// Resolves a (possibly dotted) class name as written in source to a binary class name.
    fn resolve_class_name(&self, name: &str, scope: &Scope) -> String {
        let segs: Vec<&str> = name.split('.').collect();
        self.resolve_segments(&segs, scope)
    }

    fn resolve_segments(&self, segs: &[&str], scope: &Scope) -> String {
        if let Some(first) = self.resolve_simple(segs[0], scope) {
            let mut name = first;
            for s in &segs[1..] {
                name = self
                    .member_type(&name, s)
                    .unwrap_or_else(|| format!("{name}${s}"));
            }
            return name;
        }
        // fully qualified: find the longest package prefix that names a class
        for i in (1..segs.len()).rev() {
            let candidate = segs[..=i].join(".");
            if let Some(bin) = self.source_names.get(&candidate) {
                let mut name = bin.clone();
                for s in &segs[i + 1..] {
                    name = self
                        .member_type(&name, s)
                        .unwrap_or_else(|| format!("{name}${s}"));
                }
                return name;
            }
        }
        segs.join(".")
    }

    /// Nested type `simple` declared in `class` or inherited from its supertypes.
    fn member_type(&self, class: &str, simple: &str) -> Option<String> {
        let mut seen = HashSet::new();
        self.member_type_rec(class, simple, &mut seen)
    }

    fn member_type_rec(
        &self,
        class: &str,
        simple: &str,
        seen: &mut HashSet<String>,
    ) -> Option<String> {
        if !seen.insert(class.to_string()) {
            return None;
        }
        let candidate = format!("{class}${simple}");
        if self.by_name.contains_key(&candidate) {
            return Some(candidate);
        }
        let id = self.id(class)?;
        let decl = self.decl(id);
        let unit = self.locs[id].unit;
        let scope = Scope {
            unit,
            class: self.enclosing(id),
            method_type_params: &[],
        };
        let supers: Vec<&TypeRef> = decl.extends.iter().chain(decl.implements.iter()).collect();
        for s in supers {
            if let TypeRef::Class { segments, .. } = s {
                let names: Vec<&str> = segments.iter().map(|(n, _)| n.as_str()).collect();
                // resolve the supertype name in the scope of the declaring class (without recursion into members)
                let sname = self.resolve_segments_no_members(&names, &scope, id);
                if let Some(found) = self.member_type_rec(&sname, simple, seen) {
                    return Some(found);
                }
            }
        }
        None
    }

    fn resolve_segments_no_members(&self, segs: &[&str], scope: &Scope, from: ClassId) -> String {
        // supertypes are resolved from the enclosing scope of `from`, plus imports and package
        let unit = &self.units[scope.unit].cu;
        let first = segs[0];
        let mut base = None;
        if let Some(enc) = self.enclosing(from) {
            base = self.member_type(&self.classes[enc].name.clone(), first);
        }
        if base.is_none() {
            base = self.import_or_package(first, unit);
        }
        let mut name = base.unwrap_or_else(|| segs[0].to_string());
        for s in &segs[1..] {
            name = format!("{name}${s}");
            if !self.by_name.contains_key(&name) {
                name = segs.join(".");
                break;
            }
        }
        name
    }

    fn import_or_package(&self, simple: &str, unit: &ast::CompilationUnit) -> Option<String> {
        for imp in &unit.imports {
            if !imp.is_static && !imp.wildcard && imp.name.rsplit('.').next() == Some(simple) {
                return Some(
                    self.source_names
                        .get(&imp.name)
                        .cloned()
                        .unwrap_or_else(|| imp.name.clone()),
                );
            }
        }
        let candidate = if unit.package.is_empty() {
            simple.to_string()
        } else {
            format!("{}.{simple}", unit.package)
        };
        if self.by_name.contains_key(&candidate) {
            return Some(candidate);
        }
        // on-demand imports of source packages (or of classes, for their member types)
        for imp in &unit.imports {
            if imp.wildcard && !imp.is_static {
                let candidate = format!("{}.{simple}", imp.name);
                if let Some(bin) = self.source_names.get(&candidate) {
                    return Some(bin.clone());
                }
            }
        }
        None
    }

    /// On-demand imports of library packages, after `java.lang`.
    fn external_on_demand(&self, simple: &str, unit: &ast::CompilationUnit) -> Option<String> {
        let wildcards: Vec<&ast::Import> = unit
            .imports
            .iter()
            .filter(|i| i.wildcard && !i.is_static && !self.source_names.contains_key(&i.name))
            .filter(|i| {
                !self
                    .by_name
                    .keys()
                    .any(|k| k.starts_with(&format!("{}.", i.name)))
            })
            .collect();
        for imp in &wildcards {
            if known::package_members(&imp.name).is_some_and(|m| m.contains(&simple)) {
                return Some(format!("{}.{simple}", imp.name));
            }
        }
        let unknown: Vec<&&ast::Import> = wildcards
            .iter()
            .filter(|i| known::package_members(&i.name).is_none())
            .collect();
        if unknown.len() == 1 {
            return Some(format!("{}.{simple}", unknown[0].name));
        }
        if !unknown.is_empty() {
            self.warn(format!(
                "ambiguous on-demand import for {simple}, assuming {}",
                unknown[0].name
            ));
            return Some(format!("{}.{simple}", unknown[0].name));
        }
        None
    }

    fn resolve_simple(&self, simple: &str, scope: &Scope) -> Option<String> {
        // member types of the current class and its enclosing classes
        let mut cls = scope.class;
        while let Some(c) = cls {
            if self
                .classes
                .get(c)
                .is_some_and(|ci| ci.simple_name == simple)
                && self.decl(c).name == simple
            {
                return Some(self.class_name_of(c));
            }
            if let Some(found) = self.member_type(&self.class_name_of(c), simple) {
                return Some(found);
            }
            cls = self.enclosing(c);
        }
        let unit = &self.units[scope.unit].cu;
        if let Some(found) = self.import_or_package(simple, unit) {
            return Some(found);
        }
        // static imports of member types
        for imp in &unit.imports {
            if imp.is_static
                && !imp.wildcard
                && imp.name.rsplit('.').next() == Some(simple)
                && let Some(bin) = self.source_names.get(&imp.name)
            {
                return Some(bin.clone());
            }
        }
        if JAVA_LANG.contains(&simple) {
            return Some(format!("java.lang.{simple}"));
        }
        if let Some(found) = self.external_on_demand(simple, unit) {
            return Some(found);
        }
        if simple.chars().next().is_some_and(|c| c.is_uppercase()) {
            return Some(format!("java.lang.{simple}"));
        }
        None
    }

    fn class_name_of(&self, id: ClassId) -> String {
        // during the second pass `classes[id].name` is already final
        self.classes[id].name.clone()
    }

    // ---------------------------------------------------------------- constant evaluation

    fn eval(&self, e: &Expr, scope: &Scope) -> Option<AVal> {
        match e {
            Expr::Str(s) => Some(AVal::Str(s.clone())),
            Expr::Char(c) => Some(AVal::Char(*c)),
            Expr::Bool(b) => Some(AVal::Bool(*b)),
            Expr::Num(n) => Some(parse_number(n)),
            Expr::Null | Expr::Other => None,
            Expr::ClassLit(t) => Some(AVal::Class(self.resolve_type(t, scope))),
            Expr::Array(items) => Some(AVal::Array(
                items.iter().filter_map(|i| self.eval(i, scope)).collect(),
            )),
            Expr::Annotation(a) => {
                let name = self.resolve_class_name(&a.name, scope);
                Some(AVal::Nested(Box::new(
                    self.convert_annotation(a, &name, scope),
                )))
            }
            Expr::Name(n) => self.eval_name(n, scope),
            Expr::Unary(op, inner) => {
                let v = self.eval(inner, scope)?;
                match (op, v) {
                    (&"-", AVal::Int(i)) => Some(AVal::Int(-i)),
                    (&"-", AVal::Float(f)) => Some(AVal::Float(format!("-{f}"))),
                    (&"+", v) => Some(v),
                    (&"!", AVal::Bool(b)) => Some(AVal::Bool(!b)),
                    (&"~", AVal::Int(i)) => Some(AVal::Int(!i)),
                    _ => None,
                }
            }
            Expr::Cast(inner) => self.eval(inner, scope),
            Expr::Cond(c, a, b) => match self.eval(c, scope)? {
                AVal::Bool(true) => self.eval(a, scope),
                AVal::Bool(false) => self.eval(b, scope),
                _ => None,
            },
            Expr::Binary(l, op, r) => {
                let lv = self.eval(l, scope)?;
                let rv = self.eval(r, scope)?;
                match (*op, lv, rv) {
                    ("+", AVal::Str(a), b) => Some(AVal::Str(a + &const_to_string(&b))),
                    ("+", a, AVal::Str(b)) => Some(AVal::Str(const_to_string(&a) + &b)),
                    ("+", AVal::Int(a), AVal::Int(b)) => Some(AVal::Int(a + b)),
                    ("-", AVal::Int(a), AVal::Int(b)) => Some(AVal::Int(a - b)),
                    ("*", AVal::Int(a), AVal::Int(b)) => Some(AVal::Int(a * b)),
                    ("/", AVal::Int(a), AVal::Int(b)) if b != 0 => Some(AVal::Int(a / b)),
                    ("%", AVal::Int(a), AVal::Int(b)) if b != 0 => Some(AVal::Int(a % b)),
                    ("<<", AVal::Int(a), AVal::Int(b)) => Some(AVal::Int(a << b)),
                    ("|", AVal::Int(a), AVal::Int(b)) => Some(AVal::Int(a | b)),
                    ("&", AVal::Int(a), AVal::Int(b)) => Some(AVal::Int(a & b)),
                    ("&&", AVal::Bool(a), AVal::Bool(b)) => Some(AVal::Bool(a && b)),
                    ("||", AVal::Bool(a), AVal::Bool(b)) => Some(AVal::Bool(a || b)),
                    ("==", a, b) => Some(AVal::Bool(a == b)),
                    ("!=", a, b) => Some(AVal::Bool(a != b)),
                    _ => None,
                }
            }
        }
    }

    fn eval_name(&self, n: &str, scope: &Scope) -> Option<AVal> {
        let segs: Vec<&str> = n.split('.').collect();
        if segs.len() == 1 {
            // field of the current class, its enclosing classes or supertypes
            let mut cls = scope.class;
            while let Some(c) = cls {
                if let Some(v) = self.class_constant(c, segs[0]) {
                    return Some(v);
                }
                cls = self.enclosing(c);
            }
            let unit = &self.units[scope.unit].cu;
            for imp in &unit.imports {
                if imp.is_static && !imp.wildcard && imp.name.rsplit('.').next() == Some(segs[0]) {
                    let class = &imp.name[..imp.name.len() - segs[0].len() - 1];
                    let bin = self
                        .source_names
                        .get(class)
                        .cloned()
                        .unwrap_or_else(|| class.to_string());
                    return self.qualified_constant(&bin, segs[0]);
                }
            }
            for imp in &unit.imports {
                if imp.is_static && imp.wildcard {
                    let bin = self
                        .source_names
                        .get(&imp.name)
                        .cloned()
                        .unwrap_or_else(|| imp.name.clone());
                    if let Some(id) = self.id(&bin)
                        && let Some(v) = self.class_constant(id, segs[0])
                    {
                        return Some(v);
                    }
                    if let Some(v) = known::external_constant(&bin, segs[0]) {
                        return Some(AVal::Str(v.to_string()));
                    }
                }
            }
            return None;
        }
        let field = segs[segs.len() - 1];
        let class = self.resolve_segments(&segs[..segs.len() - 1], scope);
        self.qualified_constant(&class, field)
    }

    fn qualified_constant(&self, class: &str, field: &str) -> Option<AVal> {
        if let Some(id) = self.id(class) {
            return self.class_constant(id, field);
        }
        if let Some(v) = known::external_constant(class, field) {
            return Some(AVal::Str(v.to_string()));
        }
        // assume an enum constant of a library enum
        Some(AVal::Enum {
            ty: class.to_string(),
            value: field.to_string(),
        })
    }

    /// Value of a constant field (or enum constant) declared in `id` or inherited from its supertypes.
    fn class_constant(&self, id: ClassId, field: &str) -> Option<AVal> {
        let key = (id, field.to_string());
        if let Some(v) = self.const_cache.borrow().get(&key) {
            return v.clone();
        }
        if !self.const_in_progress.borrow_mut().insert(key.clone()) {
            return None;
        }
        let result = self.class_constant_uncached(id, field);
        self.const_in_progress.borrow_mut().remove(&key);
        self.const_cache.borrow_mut().insert(key, result.clone());
        result
    }

    fn class_constant_uncached(&self, id: ClassId, field: &str) -> Option<AVal> {
        let decl = self.decl(id);
        if decl.enum_constants.iter().any(|c| c.name == field) {
            return Some(AVal::Enum {
                ty: self.classes[id].name.clone(),
                value: field.to_string(),
            });
        }
        let scope = Scope {
            unit: self.locs[id].unit,
            class: Some(id),
            method_type_params: &[],
        };
        if let Some(f) = decl.fields.iter().find(|f| f.name == field) {
            let v = self.eval(f.init.as_ref()?, &scope)?;
            // apply the declared primitive type of the constant
            return Some(match (&f.ty, v) {
                (TypeRef::Primitive { name, .. }, AVal::Int(i)) if name == "char" => {
                    AVal::Char(char::from_u32(i as u32).unwrap_or('\0'))
                }
                (_, v) => v,
            });
        }
        for s in decl.extends.iter().chain(decl.implements.iter()) {
            let st = self.resolve_type(
                s,
                &Scope {
                    unit: scope.unit,
                    class: self.enclosing(id).or(Some(id)),
                    method_type_params: &[],
                },
            );
            if let Some(sid) = self.id(&st.name())
                && let Some(v) = self.class_constant(sid, field)
            {
                return Some(v);
            }
        }
        None
    }

    // ---------------------------------------------------------------- queries

    pub fn is_a(&self, subject: &str, object: &str) -> bool {
        if subject == object {
            return true;
        }
        let mut seen = HashSet::new();
        self.is_a_rec(subject, object, &mut seen)
    }

    fn is_a_rec(&self, subject: &str, object: &str, seen: &mut HashSet<String>) -> bool {
        if !seen.insert(subject.to_string()) {
            return false;
        }
        let supers: Vec<String> = if let Some(ci) = self.class(subject) {
            ci.super_type
                .iter()
                .chain(ci.interfaces.iter())
                .map(|t| t.name())
                .collect()
        } else if let Some(s) = known::jdk_supertypes(subject) {
            s.iter().map(|s| s.to_string()).collect()
        } else {
            return false;
        };
        supers
            .iter()
            .any(|s| s == object || self.is_a_rec(s, object, seen))
    }

    /// `AugmentedIndexView.inheritanceChain`: the class and its indexed superclasses with their types.
    pub fn inheritance_chain(&self, class: ClassId, ty: &Type) -> Vec<(ClassId, Type)> {
        let mut chain = vec![(class, ty.clone())];
        let mut current = class;
        while let Some(st) = &self.classes[current].super_type {
            match self.id(&st.name()) {
                Some(next) if !chain.iter().any(|(c, _)| *c == next) => {
                    chain.push((next, st.clone()));
                    current = next;
                }
                _ => break,
            }
        }
        chain
    }

    /// `AugmentedIndexView.interfaces`: transitive interfaces in declaration order, deduplicated.
    pub fn interfaces(&self, class: ClassId) -> Vec<Type> {
        let mut out: Vec<Type> = Vec::new();
        for t in &self.classes[class].interfaces {
            if !out.contains(t) {
                out.push(t.clone());
            }
            if let Some(id) = self.id(&t.name()) {
                for i in self.interfaces(id) {
                    if !out.contains(&i) {
                        out.push(i);
                    }
                }
            }
        }
        out
    }

    /// Jandex `ClassInfo.methods()`: sorted by name, then parameter type names, then count.
    pub fn methods_sorted(&self, class: ClassId) -> Vec<MethodId> {
        let ci = &self.classes[class];
        let mut ids: Vec<MethodId> = (0..ci.methods.len())
            .map(|idx| MethodId { class, idx })
            .collect();
        ids.sort_by(|a, b| {
            let ma = self.method(*a);
            let mb = self.method(*b);
            ma.name.as_bytes().cmp(mb.name.as_bytes()).then_with(|| {
                for (pa, pb) in ma.params.iter().zip(mb.params.iter()) {
                    let c = pa.ty.name().cmp(&pb.ty.name());
                    if c != std::cmp::Ordering::Equal {
                        return c;
                    }
                }
                ma.params.len().cmp(&mb.params.len())
            })
        });
        ids
    }

    /// Jandex `ClassInfo.method(MethodInfo)`: same name and parameter type names.
    pub fn find_method_like(&self, class: ClassId, m: &MethodInfo) -> Option<MethodId> {
        self.classes[class]
            .methods
            .iter()
            .position(|o| {
                o.name == m.name
                    && o.params.len() == m.params.len()
                    && o.params
                        .iter()
                        .zip(m.params.iter())
                        .all(|(a, b)| a.ty.name() == b.ty.name())
            })
            .map(|idx| MethodId { class, idx })
    }

    pub fn all_known_implementors(&self, iface: &str) -> Vec<ClassId> {
        let mut out = Vec::new();
        for (id, ci) in self.classes.iter().enumerate() {
            if ci.name != iface && !ci.is_interface() && self.is_a(&ci.name, iface) {
                out.push(id);
            }
        }
        out
    }

    /// All annotation instances with the given name, with their targets.
    pub fn annotations(&self, name: &str) -> Vec<(Target, Anno)> {
        let mut out = Vec::new();
        for (id, ci) in self.classes.iter().enumerate() {
            for a in &ci.annotations {
                if a.name == name {
                    out.push((Target::Class(id), a.clone()));
                }
            }
            for (fi, f) in ci.fields.iter().enumerate() {
                for a in &f.annotations {
                    if a.name == name {
                        out.push((Target::Field(FieldId { class: id, idx: fi }), a.clone()));
                    }
                }
            }
            for (mi, m) in ci.methods.iter().enumerate() {
                let mid = MethodId { class: id, idx: mi };
                for a in &m.annotations {
                    if a.name == name {
                        out.push((Target::Method(mid), a.clone()));
                    }
                }
                for (pi, p) in m.params.iter().enumerate() {
                    for a in &p.annotations {
                        if a.name == name {
                            out.push((Target::Param(mid, pi), a.clone()));
                        }
                    }
                }
            }
        }
        out
    }

    /// Declared annotations of a target.
    pub fn declared(&self, target: Target) -> &[Anno] {
        match target {
            Target::Class(c) => &self.classes[c].annotations,
            Target::Field(f) => &self.field(f).annotations,
            Target::Method(m) => &self.method(m).annotations,
            Target::Param(m, p) => &self.method(m).params[p].annotations,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn register_names(
    decl: &TypeDecl,
    binary: &str,
    source: &str,
    unit: usize,
    (path, enclosing): (Vec<usize>, Option<ClassId>),
    locs: &mut Vec<DeclLoc>,
    by_name: &mut HashMap<String, ClassId>,
    source_names: &mut HashMap<String, String>,
) {
    let id = locs.len();
    locs.push(DeclLoc {
        unit,
        path: path.clone(),
        enclosing,
    });
    by_name.insert(binary.to_string(), id);
    source_names.insert(source.to_string(), binary.to_string());
    for (i, nested) in decl.types.iter().enumerate() {
        let mut p = path.clone();
        p.push(i);
        register_names(
            nested,
            &format!("{binary}${}", nested.name),
            &format!("{source}.{}", nested.name),
            unit,
            (p, Some(id)),
            locs,
            by_name,
            source_names,
        );
    }
}

fn flags_of(m: &ast::Modifiers) -> Flags {
    Flags {
        public: m.has("public"),
        private: m.has("private"),
        static_: m.has("static"),
        abstract_: m.has("abstract"),
        final_: m.has("final"),
        interface: false,
    }
}

fn with_type_annotations(mut ty: Type, annos: Vec<Anno>) -> Type {
    if annos.is_empty() {
        return ty;
    }
    // for arrays, a declaration-position type annotation applies to the element type
    if let Ty::Array(c, d) = ty.ty {
        let mut c = *c;
        c.annotations.extend(annos);
        return Type {
            ty: Ty::Array(Box::new(c), d),
            annotations: ty.annotations,
        };
    }
    ty.annotations.extend(annos);
    ty
}

fn parse_number(n: &str) -> AVal {
    let lower = n.to_ascii_lowercase();
    if lower.starts_with("0x") {
        let digits = lower.trim_start_matches("0x").trim_end_matches('l');
        return AVal::Int(i64::from_str_radix(digits, 16).unwrap_or(0));
    }
    if lower.contains('.') || lower.contains('e') || lower.ends_with('f') || lower.ends_with('d') {
        return AVal::Float(n.trim_end_matches(['f', 'F', 'd', 'D']).to_string());
    }
    let digits = lower.trim_end_matches('l');
    if digits.len() > 1 && digits.starts_with('0') && digits.chars().all(|c| c.is_ascii_digit()) {
        return AVal::Int(i64::from_str_radix(digits, 8).unwrap_or(0));
    }
    AVal::Int(digits.parse().unwrap_or(0))
}

fn const_to_string(v: &AVal) -> String {
    match v {
        AVal::Str(s) => s.clone(),
        AVal::Int(i) => i.to_string(),
        AVal::Float(f) => f.clone(),
        AVal::Bool(b) => b.to_string(),
        AVal::Char(c) => c.to_string(),
        AVal::Enum { value, .. } => value.clone(),
        _ => String::new(),
    }
}
