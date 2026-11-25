//! Port of `TypeResolver`: discovers the properties of a data class and resolves generic types.

use std::cmp::Ordering;
use std::collections::HashMap;

use indexmap::IndexMap;

use super::util::{self, AT, SCHEMA};
use super::{Ctx, ResolutionStack};
use crate::index::{ClassId, FieldId, Index, Kind, MethodId, Target, Ty, Type};

const JSONB_PROPERTY: &str = "jakarta.json.bind.annotation.JsonbProperty";
const JSONB_PROPERTY_JAVAX: &str = "javax.json.bind.annotation.JsonbProperty";
const JSON_PROPERTY: &str = "com.fasterxml.jackson.annotation.JsonProperty";
const XML_ELEMENT: &str = "jakarta.xml.bind.annotation.XmlElement";
const XML_ATTRIBUTE: &str = "jakarta.xml.bind.annotation.XmlAttribute";
const JSONB_TRANSIENT: &str = "jakarta.json.bind.annotation.JsonbTransient";
const JSON_IGNORE: &str = "com.fasterxml.jackson.annotation.JsonIgnore";
const JSON_UNWRAPPED: &str = "com.fasterxml.jackson.annotation.JsonUnwrapped";

/// `resolveType(type, resolutionStack, resolveWildcards)`.
pub fn resolve_type(t: &Type, stack: &[HashMap<String, Type>], resolve_wildcards: bool) -> Type {
    let mut ty = if resolve_wildcards {
        util::resolve_wildcard(t)
    } else {
        t.clone()
    };
    let mut i = 0;
    while i < stack.len() {
        let var = match &ty.ty {
            Ty::Var { id, .. } => Some(id.clone()),
            _ => None,
        };
        if let Some(v) = var {
            if let Some(r) = stack[i].get(&v) {
                ty = r.clone();
            }
            i += 1;
        } else if ty.kind() == Kind::Parameterized {
            ty = resolve_param(&ty, &stack[i..]);
            break;
        } else {
            i += 1;
        }
    }
    ty
}

fn resolve_param(t: &Type, stack: &[HashMap<String, Type>]) -> Type {
    match &t.ty {
        Ty::Param { name, args, owner } => Type {
            ty: Ty::Param {
                name: name.clone(),
                args: args.iter().map(|a| resolve_type(a, stack, false)).collect(),
                owner: owner
                    .as_ref()
                    .map(|o| Box::new(resolve_type(o, stack, true))),
            },
            annotations: t.annotations.clone(),
        },
        _ => t.clone(),
    }
}

pub fn param_resolution_map(idx: &Index, class: ClassId, t: &Type) -> HashMap<String, Type> {
    let ci = &idx.classes[class];
    let args = t.args();
    if t.kind() != Kind::Parameterized || args.len() != ci.type_params.len() {
        return HashMap::new();
    }
    ci.type_params
        .iter()
        .zip(args.iter())
        .filter_map(|(tp, a)| match &tp.ty {
            Ty::Var { id, .. } => Some((id.clone(), a.clone())),
            _ => None,
        })
        .collect()
}

/// `TypeResolver.forClass`: resolution stack for a resource class.
pub fn for_class(idx: &Index, class: ClassId, leaf: Option<&Type>) -> ResolutionStack {
    let class_type = leaf
        .cloned()
        .unwrap_or_else(|| Type::class(&idx.classes[class].name));
    let mut stack: ResolutionStack = Vec::new();
    for (c, t) in idx.inheritance_chain(class, &class_type) {
        if t.kind() == Kind::Parameterized {
            stack.insert(0, param_resolution_map(idx, c, &t));
        }
        for i in idx.interfaces(c) {
            if i.kind() == Kind::Parameterized
                && let Some(ic) = idx.id(&i.name())
            {
                stack.insert(0, param_resolution_map(idx, ic, &i));
            }
        }
    }
    stack
}

#[derive(Clone, Debug)]
pub struct Prop {
    pub property_name: String,
    pub field: Option<FieldId>,
    pub read_method: Option<MethodId>,
    pub write_method: Option<MethodId>,
    pub ignored: bool,
    exposed: bool,
    pub read_only: bool,
    pub write_only: bool,
    ignored_read: Option<MethodId>,
    ignored_write: Option<MethodId>,
    pub constraint_targets: Vec<Target>,
    pub stack: ResolutionStack,
}

impl Prop {
    fn new(name: &str, field: Option<FieldId>, stack: ResolutionStack) -> Prop {
        Prop {
            property_name: name.to_string(),
            field,
            read_method: None,
            write_method: None,
            ignored: false,
            exposed: false,
            read_only: false,
            write_only: false,
            ignored_read: None,
            ignored_write: None,
            constraint_targets: Vec::new(),
            stack,
        }
    }

    fn targets(&self) -> Vec<Target> {
        let mut t = Vec::new();
        if let Some(f) = self.field {
            t.push(Target::Field(f));
        }
        if let Some(m) = self.read_method {
            t.push(Target::Method(m));
        }
        if let Some(m) = self.write_method {
            t.push(Target::Method(m));
        }
        t
    }

    /// `getAnnotationTarget()`: the head of the priority queue.
    pub fn target(&self, idx: &Index) -> Target {
        let targets = self.targets();
        let mut best = targets[0];
        for t in &targets[1..] {
            if compare_targets(idx, *t, best) == Ordering::Less {
                best = *t;
            }
        }
        best
    }

    pub fn unresolved_type(&self, idx: &Index) -> Type {
        match self.target(idx) {
            Target::Field(f) => idx.field(f).ty.clone(),
            Target::Method(m) if Some(m) == self.write_method => idx.method(m).params[0].ty.clone(),
            Target::Method(m) => idx.method(m).return_type.clone(),
            _ => unreachable!(),
        }
    }

    pub fn resolve_type(&self, idx: &Index) -> Type {
        resolve_type(&self.unresolved_type(idx), &self.stack, true)
    }

    /// `getPropertyName()`.
    pub fn name(&self, idx: &Index) -> String {
        let target = AT::Decl(self.target(idx));
        if let Some(n) = util::ann(idx, &target, SCHEMA).and_then(|a| a.str("name")) {
            return n;
        }
        for jsonb in [JSONB_PROPERTY, JSONB_PROPERTY_JAVAX] {
            if let Some(n) = util::ann(idx, &target, jsonb).and_then(|a| a.str("value")) {
                return n;
            }
        }
        if let Some(n) = util::ann(idx, &target, JSON_PROPERTY).and_then(|a| a.str("value")) {
            return n;
        }
        self.property_name.clone()
    }

    pub fn is_ignored(&self, idx: &Index) -> bool {
        self.ignored
            || (self.read_only && self.read_method.is_none() && self.field.is_none())
            || (self.write_only && self.write_method.is_none() && self.field.is_none())
            || (self.ignored_read.is_some()
                && self.read_method == self.ignored_read
                && self.write_method.is_none()
                && !self.field_explicitly_visible(idx))
            || (self.ignored_write.is_some()
                && self.write_method == self.ignored_write
                && self.read_method.is_none()
                && !self.field_explicitly_visible(idx))
    }

    fn field_explicitly_visible(&self, idx: &Index) -> bool {
        let Some(f) = self.field else { return false };
        let t = AT::Decl(Target::Field(f));
        util::has(idx, &t, "com.fasterxml.jackson.annotation.JsonView")
            || is_unhidden(idx, Target::Field(f))
            || util::has(idx, &t, JSON_PROPERTY)
    }

    fn process_visibility(&mut self, idx: &Index, target: Target) {
        if self.exposed || self.ignored {
            return;
        }
        match visibility(idx, target) {
            Visibility::Exposed => self.exposed = true,
            Visibility::Ignored => {
                if let Target::Method(m) = target {
                    if is_accessor(idx, m) {
                        self.write_only = true;
                        self.ignored_read = Some(m);
                    } else {
                        self.read_only = true;
                        self.ignored_write = Some(m);
                    }
                    if self.read_only && self.write_only {
                        self.ignored = true;
                    }
                } else {
                    self.ignored = true;
                }
            }
            Visibility::Unset => {}
        }
    }

    fn process_access(&mut self, idx: &Index, target: Target) {
        if self.ignored {
            return;
        }
        let access =
            util::ann(idx, &AT::Decl(target), JSON_PROPERTY).and_then(|a| a.enum_value("access"));
        match access.as_deref() {
            Some("READ_ONLY") => self.read_only = true,
            Some("WRITE_ONLY") => self.write_only = true,
            Some("READ_WRITE") => {
                self.read_only = false;
                self.write_only = false;
            }
            _ => {}
        }
    }
}

enum Visibility {
    Ignored,
    Exposed,
    Unset,
}

fn is_unhidden(idx: &Index, target: Target) -> bool {
    match util::ann(idx, &AT::Decl(target), SCHEMA) {
        Some(a) => a.bool("hidden") != Some(true),
        None => false,
    }
}

fn visibility(idx: &Index, target: Target) -> Visibility {
    if is_unhidden(idx, target) {
        return Visibility::Exposed;
    }
    let t = AT::Decl(target);
    if let Some(a) = util::ann(idx, &t, SCHEMA)
        && let Some(hidden) = a.bool("hidden")
    {
        return if hidden {
            Visibility::Ignored
        } else {
            Visibility::Exposed
        };
    }
    if util::has(idx, &t, JSONB_TRANSIENT) {
        return Visibility::Ignored;
    }
    if let Some(a) = util::ann(idx, &t, JSON_IGNORE) {
        return if a.bool("value") == Some(false) {
            Visibility::Exposed
        } else {
            Visibility::Ignored
        };
    }
    Visibility::Unset
}

fn compare_annotation(idx: &Index, t1: Target, t2: Target, name: &str) -> Ordering {
    let a1 = util::has(idx, &AT::Decl(t1), name);
    let a2 = util::has(idx, &AT::Decl(t2), name);
    match (a1, a2) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => Ordering::Equal,
    }
}

fn compare_targets(idx: &Index, t1: Target, t2: Target) -> Ordering {
    for name in [
        SCHEMA,
        JSONB_PROPERTY,
        JSONB_PROPERTY_JAVAX,
        JSON_PROPERTY,
        XML_ELEMENT,
        XML_ATTRIBUTE,
    ] {
        let r = compare_annotation(idx, t1, t2, name);
        if r != Ordering::Equal {
            return r;
        }
    }
    if matches!(t1, Target::Field(_)) {
        return Ordering::Less;
    }
    if matches!(t2, Target::Field(_)) {
        return Ordering::Greater;
    }
    if let (Target::Method(m1), Target::Method(m2)) = (t1, t2)
        && is_accessor(idx, m1)
        && !is_accessor(idx, m2)
    {
        return Ordering::Less;
    }
    Ordering::Equal
}

fn is_supplier(idx: &Index, m: MethodId) -> bool {
    let mi = idx.method(m);
    mi.return_type.kind() != Kind::Void && mi.params.is_empty()
}

fn method_prefix(name: &str) -> &'static str {
    if name.starts_with("get") {
        "get"
    } else if name.starts_with("is") {
        "is"
    } else if name.starts_with("set") {
        "set"
    } else {
        ""
    }
}

fn is_annotated_property(idx: &Index, m: MethodId) -> bool {
    let t = AT::Decl(Target::Method(m));
    [SCHEMA, JSONB_PROPERTY, JSONB_PROPERTY_JAVAX, JSON_PROPERTY]
        .iter()
        .any(|n| util::has(idx, &t, n))
}

pub fn is_accessor(idx: &Index, m: MethodId) -> bool {
    if !is_supplier(idx, m) {
        return false;
    }
    let ci = &idx.classes[m.class];
    let mi = idx.method(m);
    if ci.is_record() && ci.record_component(&mi.name) {
        return true;
    }
    match method_prefix(&mi.name) {
        "get" => true,
        "is" if util::equal_types(&mi.return_type, &Type::class("java.lang.Boolean")) => true,
        _ => is_annotated_property(idx, m),
    }
}

fn is_mutator(idx: &Index, m: MethodId) -> bool {
    let mi = idx.method(m);
    if mi.params.len() != 1 {
        return false;
    }
    if is_annotated_property(idx, m) {
        return true;
    }
    mi.return_type.kind() == Kind::Void && method_prefix(&mi.name) == "set"
}

fn property_name_of(idx: &Index, props: &IndexMap<String, Prop>, m: MethodId) -> Option<String> {
    let ci = &idx.classes[m.class];
    let name = &idx.method(m).name;
    if ci.is_record() && ci.record_component(name) {
        return Some(name.clone());
    }
    if props.get(name).is_some_and(|p| p.field.is_some()) {
        return Some(name.clone());
    }
    let start = method_prefix(name).len();
    if name.len() == start {
        return None;
    }
    if start > 0 {
        let raw = &name[start..];
        if props.get(raw).is_some_and(|p| p.field.is_some()) {
            return Some(raw.to_string());
        }
        let chars: Vec<char> = raw.chars().collect();
        if chars.len() > 1 && chars[1].is_uppercase() && chars[0].is_uppercase() {
            return Some(raw.to_string());
        }
        let mut s: String = chars[0].to_lowercase().collect();
        s.extend(chars[1..].iter());
        return Some(s);
    }
    Some(name.clone())
}

fn has_constraints(idx: &Index, target: Target) -> bool {
    idx.declared(target)
        .iter()
        .any(|a| super::beanval::is_constraint(&a.name))
        || super::beanval::has_composed_constraint(idx, target)
}

fn scan_field(ctx: &Ctx, props: &mut IndexMap<String, Prop>, f: FieldId, stack: &ResolutionStack) {
    let idx = ctx.idx;
    let fi = idx.field(f);
    let name = fi.name.clone();
    if util::has(idx, &AT::Decl(Target::Field(f)), JSON_UNWRAPPED) {
        ctx.unsupported(format!("@JsonUnwrapped ({name})"));
    }
    if let Some(p) = props.get_mut(&name) {
        if p.field.is_none() {
            // private properties are enabled, so any field is taken
            p.field = Some(f);
        }
    } else {
        props.insert(name.clone(), Prop::new(&name, Some(f), stack.clone()));
    }
    let p = props.get_mut(&name).unwrap();
    if has_constraints(idx, Target::Field(f)) {
        p.constraint_targets.push(Target::Field(f));
    }
    p.process_visibility(idx, Target::Field(f));
    p.process_access(idx, Target::Field(f));
}

fn scan_method(
    ctx: &Ctx,
    props: &mut IndexMap<String, Prop>,
    m: MethodId,
    stack: &ResolutionStack,
) {
    let idx = ctx.idx;
    let property_type = if is_accessor(idx, m) {
        Some(resolve_type(&idx.method(m).return_type, stack, true))
    } else if is_mutator(idx, m) {
        Some(idx.method(m).params[0].ty.clone())
    } else {
        None
    };
    let Some(property_type) = property_type else {
        return;
    };
    let Some(name) = property_name_of(idx, props, m) else {
        return;
    };
    if let Some(existing) = props.get(&name) {
        let resolved = resolve_type(&property_type, stack, true);
        if !util::equal_types(&existing.resolve_type(idx), &resolved) {
            let p = props.get_mut(&name).unwrap();
            p.process_visibility(idx, Target::Method(m));
            p.process_access(idx, Target::Method(m));
            return;
        }
    } else {
        props.insert(name.clone(), Prop::new(&name, None, stack.clone()));
    }
    let p = props.get_mut(&name).unwrap();
    let declaring_interface = idx.classes[m.class].is_interface();
    if is_mutator(idx, m) {
        let higher = match p.write_method {
            None => true,
            Some(old) => {
                declaring_interface
                    && compare_targets(idx, Target::Method(m), Target::Method(old))
                        == Ordering::Less
            }
        };
        if higher {
            p.write_method = Some(m);
            p.stack = stack.clone();
        }
    } else {
        let higher = match p.read_method {
            None => true,
            Some(old) => {
                declaring_interface
                    && compare_targets(idx, Target::Method(m), Target::Method(old))
                        == Ordering::Less
            }
        };
        if higher {
            p.read_method = Some(m);
            p.stack = stack.clone();
        }
    }
    if has_constraints(idx, Target::Method(m)) {
        p.constraint_targets.push(Target::Method(m));
    }
    p.process_visibility(idx, Target::Method(m));
    p.process_access(idx, Target::Method(m));
}

fn accept_method(idx: &Index, m: MethodId) -> bool {
    let mi = idx.method(m);
    !mi.flags.static_ && mi.name != "<init>" && mi.name != "getClass"
}

/// `TypeResolver.getAllFields`.
pub fn all_fields(ctx: &Ctx, leaf: &Type, leaf_class: ClassId) -> Vec<Prop> {
    let idx = ctx.idx;
    let chain = idx.inheritance_chain(leaf_class, leaf);
    let mut props: IndexMap<String, Prop> = IndexMap::new();
    let mut stack: ResolutionStack = Vec::new();
    for (c, t) in &chain {
        if util::known_java_type(&idx.classes[*c].name) {
            break;
        }
        if t.kind() == Kind::Parameterized {
            stack.insert(0, param_resolution_map(idx, *c, t));
        }
        let ci = &idx.classes[*c];
        for fi in 0..ci.fields.len() {
            let f = &ci.fields[fi];
            if !f.flags.static_ && !f.enum_constant {
                scan_field(ctx, &mut props, FieldId { class: *c, idx: fi }, &stack);
            }
        }
        for mi in 0..ci.methods.len() {
            let m = MethodId { class: *c, idx: mi };
            if accept_method(idx, m) {
                scan_method(ctx, &mut props, m, &stack);
            }
        }
        for it in idx.interfaces(*c) {
            if util::known_java_type(&it.name()) {
                continue;
            }
            let ic = idx.id(&it.name());
            if let Some(ic) = ic
                && it.kind() == Kind::Parameterized
            {
                stack.insert(0, param_resolution_map(idx, ic, &it));
            }
            if let Some(ic) = ic {
                for mi in 0..idx.classes[ic].methods.len() {
                    scan_method(ctx, &mut props, MethodId { class: ic, idx: mi }, &stack);
                }
            }
        }
    }
    sorted(idx, props, &chain)
}

fn property_order(idx: &Index, class: ClassId) -> Vec<String> {
    let ci = &idx.classes[class];
    for (name, key) in [
        ("jakarta.json.bind.annotation.JsonbPropertyOrder", "value"),
        ("jakarta.xml.bind.annotation.XmlType", "propOrder"),
        (
            "com.fasterxml.jackson.annotation.JsonPropertyOrder",
            "value",
        ),
    ] {
        if let Some(a) = ci.annotation(name) {
            return a.string_array(key).unwrap_or_default();
        }
    }
    Vec::new()
}

fn sorted(idx: &Index, props: IndexMap<String, Prop>, chain: &[(ClassId, Type)]) -> Vec<Prop> {
    let mut classes: Vec<ClassId> = chain.iter().map(|(c, _)| *c).collect();
    classes.reverse();
    let order: Vec<String> = classes
        .iter()
        .flat_map(|c| property_order(idx, *c))
        .collect();
    let class_names: Vec<String> = classes
        .iter()
        .map(|c| idx.classes[*c].name.clone())
        .collect();
    let mut entries: Vec<(String, Prop)> = props.into_iter().collect();
    let key = |(k, p): &(String, Prop)| {
        let mut pi = order.iter().position(|o| *o == p.name(idx));
        if pi.is_none() {
            pi = order.iter().position(|o| o == k);
        }
        let declaring = util::declaring_class(p.target(idx))
            .map(|c| idx.classes[c].name.clone())
            .unwrap_or_default();
        let ci = class_names
            .iter()
            .position(|n| *n == declaring)
            .map(|i| i as i64)
            .unwrap_or(-1);
        (pi, ci)
    };
    entries.sort_by(|a, b| {
        let (p1, c1) = key(a);
        let (p2, c2) = key(b);
        match (p1, p2) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => c1.cmp(&c2),
        }
    });
    entries.into_iter().map(|(_, p)| p).collect()
}
