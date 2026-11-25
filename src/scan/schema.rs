//! Port of `SchemaFactory`, `OpenApiDataObjectScanner`, `AnnotationTargetProcessor` and `TypeProcessor`.

use std::rc::Rc;

use super::beanval::{self, Req};
use super::props::{self, Prop};
use super::util::{self, AT, Defaults, MAP, SCHEMA};
use super::{Ctx, RegAction, ResolutionStack, registry_register, registry_register_with};
use crate::index::{AVal, Anno, ClassId, Kind, OBJECT, Target, Ty, Type};
use crate::model::{self, M, MK, V};

const REST_RESPONSE: &str = "org.jboss.resteasy.reactive.RestResponse";

pub fn is_scanner_internal_response(t: &Type) -> bool {
    matches!(
        t.name().as_str(),
        "javax.ws.rs.core.Response" | "jakarta.ws.rs.core.Response"
    )
}

fn is_wrapper_type(t: &Type) -> bool {
    t.name() == REST_RESPONSE && t.kind() == Kind::Parameterized
}

// ---------------------------------------------------------------- SchemaFactory

/// `SchemaFactory.typeToSchema(context, type, schemaAnnotation)`.
pub fn type_to_schema(ctx: &mut Ctx, t: &Type, schema_anno: Option<&Anno>) -> Option<M> {
    let mut from_annotation = None;
    if let Some(a) = schema_anno {
        from_annotation = read_schema_simple(ctx, a);
        from_annotation.as_ref()?;
    }
    let schema = if util::is_wrapped(t) {
        type_to_schema(ctx, &util::unwrap(t), None)
    } else if ctx.jaxrs_active && is_wrapper_type(t) {
        type_to_schema(ctx, &t.args()[0], None)
    } else if util::is_terminal(t) {
        let s = model::schema();
        util::apply_type_attributes(t, &s, schema_anno);
        Some(schema_registration(ctx, t, s))
    } else if t.kind() == Kind::Array {
        Some(array_type_to_schema(ctx, t))
    } else if t.kind() == Kind::Class {
        introspect_class(ctx, t, true)
    } else if t.kind() == Kind::Primitive {
        let s = model::schema();
        util::apply_type_attributes(t, &s, None);
        Some(s)
    } else {
        let s = data_object_process(ctx, t);
        Some(schema_registration(ctx, t, s))
    };
    beanval::apply_to_type(ctx.idx, t, schema.as_ref());
    match from_annotation {
        Some(fa) => model::merge(schema, Some(fa)),
        None => schema,
    }
}

fn array_type_to_schema(ctx: &mut Ctx, t: &Type) -> M {
    let s = model::schema();
    model::add_type(&s, "array");
    let Ty::Array(component, dims) = &t.ty else {
        unreachable!()
    };
    let item = if *dims > 1 {
        type_to_schema(
            ctx,
            &Type::new(Ty::Array(component.clone(), dims - 1)),
            None,
        )
    } else {
        let item = type_to_schema(ctx, component, None);
        item.map(|i| lookup_ref(ctx, component, i))
    };
    model::set(&s, "items", item.map(V::Obj));
    s
}

/// `SchemaFactory.enumToSchema`.
pub fn enum_to_schema(ctx: &mut Ctx, t: &Type) -> M {
    let idx = ctx.idx;
    let ci = idx.class(&util::get_name(t)).expect("enum class");
    if ci
        .methods
        .iter()
        .any(|m| m.annotations.iter().any(|a| a.name.ends_with(".JsonValue")))
    {
        ctx.unsupported(format!(
            "enum {} uses @JsonValue, which needs reflection",
            ci.name
        ));
    }
    let constants: Vec<V> = ci
        .fields
        .iter()
        .filter(|f| f.enum_constant)
        .map(|f| {
            let name = f
                .annotations
                .iter()
                .find(|a| a.name == "com.fasterxml.jackson.annotation.JsonProperty")
                .and_then(|a| a.str("value"))
                .unwrap_or_else(|| f.name.clone());
            V::Str(name)
        })
        .collect();
    let schema_anno = ci.annotation(SCHEMA).cloned();
    let s = model::schema();
    match schema_anno {
        Some(a) => {
            let mut defaults = util::type_defaults(&Type::class("java.lang.String"));
            defaults.enumeration = Some(constants);
            let class_id = idx.id(&ci.name).unwrap();
            read_schema_for_class(ctx, s, &a, class_id, true, &defaults)
        }
        None => {
            util::apply_type_attributes(&Type::class("java.lang.String"), &s, None);
            model::set_v(&s, "enum", V::List(constants));
            s
        }
    }
}

fn introspect_class(ctx: &mut Ctx, t: &Type, ref_supported: bool) -> Option<M> {
    if ctx.jaxrs_active && is_scanner_internal_response(t) {
        return None;
    }
    if let Some(ci) = ctx.idx.class(&t.name())
        && let Some(a) = ci.annotation(SCHEMA)
        && a.bool("hidden") == Some(true)
    {
        return None;
    }
    if ctx.registry.has_schema(t) {
        if ref_supported {
            return Some(ctx.registry.lookup_ref(t));
        }
        return ctx.registry.lookup_schema(t).map(|s| model::deep_copy(&s));
    }
    if ctx.scan_stack.contains(t) {
        return registry_register_with(ctx, t, None, &model::schema(), RegAction::Reference);
    }
    let schema = data_object_process(ctx, t);
    if ref_supported {
        Some(schema_registration(ctx, t, schema))
    } else {
        Some(schema)
    }
}

/// `SchemaFactory.schemaRegistration`.
pub fn schema_registration(ctx: &mut Ctx, t: &Type, schema: M) -> M {
    let allow = super::registration_supported(ctx, t) && !ctx.registry.has_schema(t);
    if allow {
        registry_register(ctx, t, &schema)
    } else {
        lookup_ref(ctx, t, schema)
    }
}

pub fn lookup_ref(ctx: &Ctx, t: &Type, schema: M) -> M {
    if ctx.registry.has_ref(t) {
        ctx.registry.lookup_ref(t)
    } else {
        schema
    }
}

/// `SchemaFactory.readClassSchema`.
pub fn read_class_schema(ctx: &mut Ctx, t: &Type, ref_supported: bool) -> Option<M> {
    if util::is_void(t) {
        return None;
    }
    let n = t.name();
    if n == "org.eclipse.microprofile.openapi.annotations.media.Schema$True"
        || n == "org.eclipse.microprofile.openapi.annotations.media.Schema$False"
    {
        ctx.unsupported(format!("boolean schema {n}"));
        return None;
    }
    match &t.ty {
        Ty::Array(component, dims) => {
            let s = model::schema();
            model::add_type(&s, "array");
            let item = if *dims > 1 {
                read_class_schema(
                    ctx,
                    &Type::new(Ty::Array(component.clone(), dims - 1)),
                    ref_supported,
                )
            } else {
                read_class_schema(ctx, component, ref_supported)
                    .map(|i| lookup_ref(ctx, component, i))
            };
            model::set(&s, "items", item.map(V::Obj));
            Some(s)
        }
        Ty::Prim(_) => {
            let s = model::schema();
            util::apply_type_attributes(t, &s, None);
            Some(s)
        }
        _ => introspect_class(ctx, t, ref_supported),
    }
}

fn read_class_schemas(ctx: &mut Ctx, types: &[Type], remove_current: bool) -> Option<Vec<M>> {
    if types.iter().all(|t| t.kind() == Kind::Void) {
        return None;
    }
    let current = if remove_current {
        ctx.scan_stack.last().cloned()
    } else {
        None
    };
    Some(
        types
            .iter()
            .filter(|t| Some(*t) != current.as_ref())
            .filter_map(|t| read_class_schema(ctx, t, true))
            .collect(),
    )
}

/// `SchemaFactory.readSchema(context, annotation)`.
pub fn read_schema_simple(ctx: &mut Ctx, a: &Anno) -> Option<M> {
    if a.bool("hidden") == Some(true) {
        return None;
    }
    Some(read_schema(ctx, model::schema(), a, &Defaults::default()))
}

/// `SchemaFactory.readSchema(context, schema, annotation, clazz, registerSchema, defaults)`.
pub fn read_schema_for_class(
    ctx: &mut Ctx,
    schema: M,
    a: &Anno,
    class: ClassId,
    register: bool,
    defaults: &Defaults,
) -> M {
    if a.bool("hidden") == Some(true) {
        return schema;
    }
    let schema = read_schema(ctx, schema, a, defaults);
    if register {
        let t = Type::class(&ctx.idx.classes[class].name);
        schema_registration(ctx, &t, schema.clone());
    }
    schema
}

fn str_attr(a: &Anno, key: &str, default: Option<&String>) -> Option<V> {
    a.str(key).or_else(|| default.cloned()).map(V::Str)
}

/// `SchemaFactory.readSchema(context, schema, annotation, defaults)`.
pub fn read_schema(ctx: &mut Ctx, schema: M, a: &Anno, defaults: &Defaults) -> M {
    if a.bool("hidden") == Some(true) {
        return schema;
    }
    let mut defaults = defaults.clone();
    if let Some(r) = a.str("ref") {
        model::set_ref(&schema, Some(&r));
        defaults = Defaults::default();
    }
    let set = |k: &str, v: Option<V>| model::set(&schema, k, v);
    let not = match a.class("not") {
        Some(t) => read_class_schema(ctx, &t, true).map(V::Obj),
        None => None,
    };
    set("not", not);
    for (key, remove_current) in [("oneOf", false), ("anyOf", false), ("allOf", true)] {
        let v = if a.value(key).is_some() {
            read_class_schemas(ctx, &a.class_array(key), remove_current)
                .map(|l| V::List(l.into_iter().map(V::Obj).collect()))
        } else {
            None
        };
        set(key, v);
    }
    for (attr, key) in [
        ("ifSchema", "if"),
        ("thenSchema", "then"),
        ("elseSchema", "else"),
    ] {
        let v = match a.class(attr) {
            Some(t) => read_class_schema(ctx, &t, true).map(V::Obj),
            None => None,
        };
        set(key, v);
    }
    set("title", str_attr(a, "title", None));
    set("multipleOf", a.value("multipleOf").and_then(num_value));
    let maximum = a.str("maximum").filter(|v| v.trim().parse::<f64>().is_ok());
    let minimum = a.str("minimum").filter(|v| v.trim().parse::<f64>().is_ok());
    if let Some(max) = maximum {
        let key = if a.bool("exclusiveMaximum") == Some(true) {
            "exclusiveMaximum"
        } else {
            "maximum"
        };
        set(key, Some(V::Dec(max)));
    }
    if let Some(min) = minimum {
        let key = if a.bool("exclusiveMinimum") == Some(true) {
            "exclusiveMinimum"
        } else {
            "minimum"
        };
        set(key, Some(V::Dec(min)));
    }
    set("maxLength", a.int("maxLength").map(V::Int));
    set("minLength", a.int("minLength").map(V::Int));
    set("pattern", str_attr(a, "pattern", defaults.pattern.as_ref()));
    set("maxProperties", a.int("maxProperties").map(V::Int));
    set("minProperties", a.int("minProperties").map(V::Int));
    set(
        "required",
        a.string_array("requiredProperties")
            .map(|l| V::List(l.into_iter().map(V::Str).collect())),
    );
    set("description", str_attr(a, "description", None));
    set("format", str_attr(a, "format", defaults.format.as_ref()));
    match a.bool("nullable") {
        Some(n) => model::set_nullable(&schema, n),
        None => {
            model::set_private_ext(&schema, "nullable", None);
            model::remove_type(&schema, "null");
        }
    }
    set("readOnly", a.bool("readOnly").map(V::Bool));
    set("writeOnly", a.bool("writeOnly").map(V::Bool));
    let ext_docs = match a.nested("externalDocs") {
        Some(d) => {
            let m = model::new(MK::ExternalDocs);
            model::set(&m, "description", d.str("description").map(V::Str));
            model::set(&m, "url", d.str("url").map(V::Str));
            Some(V::Obj(m))
        }
        None => None,
    };
    set("externalDocs", ext_docs);
    set("deprecated", a.bool("deprecated").map(V::Bool));
    let ty: Option<String> = match a.enum_value("type").or_else(|| a.str("type")) {
        Some(t) => Some(util::schema_type_name(&t).to_string()),
        None => match &defaults.ty {
            Some(t) => Some(t.clone()),
            None => model::non_null_type(&schema),
        },
    };
    model::set_type(&schema, ty.as_deref());
    let example = a.str("example").map(V::Str);
    let examples: Option<Vec<V>> = match a.string_array("examples") {
        Some(e) => Some(e.into_iter().map(V::Str).collect()),
        None if example.is_none() => defaults
            .examples
            .clone()
            .map(|e| e.into_iter().map(V::Str).collect()),
        None => None,
    };
    let examples = match (examples, example) {
        (Some(mut e), Some(x)) => {
            e.push(x);
            Some(e)
        }
        (Some(e), None) => Some(e),
        (None, Some(x)) => Some(vec![x]),
        (None, None) => None,
    };
    set("examples", examples.map(V::List));
    set("default", a.str("defaultValue").map(V::Str));
    let discriminator = read_discriminator(ctx, a);
    set("discriminator", discriminator.map(V::Obj));
    set("maxItems", a.int("maxItems").map(V::Int));
    set("minItems", a.int("minItems").map(V::Int));
    set("uniqueItems", a.bool("uniqueItems").map(V::Bool));
    if a.value("extensions").is_some() {
        ctx.unsupported("@Schema(extensions)".into());
    }
    set("$comment", str_attr(a, "comment", None));
    set("const", str_attr(a, "constValue", None));
    let properties = a.nested_array("properties");
    if !properties.is_empty() {
        let mut map = indexmap::IndexMap::new();
        for p in properties {
            let key = p.str("name").unwrap_or_default();
            let v = read_schema(ctx, model::schema(), &p, &Defaults::default());
            map.insert(key, V::Obj(v));
        }
        set("properties", Some(V::Map(map)));
    } else {
        set("properties", None);
    }
    let additional = match a.class("additionalProperties") {
        Some(t) => read_class_schema(ctx, &t, true).map(V::Obj),
        None => None,
    };
    set("additionalProperties", additional);
    for key in [
        "dependentRequired",
        "dependentSchemas",
        "contentSchema",
        "contains",
        "prefixItems",
        "propertyNames",
        "patternProperties",
    ] {
        if a.value(key).is_some() {
            ctx.unsupported(format!("@Schema({key})"));
        }
    }
    set("dependentRequired", None);
    set("dependentSchemas", None);
    set("contentEncoding", str_attr(a, "contentEncoding", None));
    set("contentMediaType", str_attr(a, "contentMediaType", None));
    set("contentSchema", None);
    set("contains", None);
    set("maxContains", a.int("maxContains").map(V::Int));
    set("minContains", a.int("minContains").map(V::Int));
    set("prefixItems", None);
    set("propertyNames", None);
    set("patternProperties", None);
    let enumeration: Option<Vec<V>> = match a.string_array("enumeration") {
        Some(e) => Some(e.into_iter().map(V::Str).collect()),
        None => defaults.enumeration.clone(),
    };
    if let Some(e) = enumeration
        && !e.is_empty()
    {
        model::set_v(&schema, "enum", V::List(e));
    }
    let named_component = model::private_ext(&schema, "name").is_some();
    let implementation = a.class("implementation");
    if util::is_simple_class_schema(a) {
        let impl_schema =
            read_class_schema(ctx, implementation.as_ref().unwrap(), !named_component);
        return model::merge(impl_schema, Some(schema.clone())).unwrap();
    } else if util::is_simple_array_schema(a) {
        let impl_schema =
            read_class_schema(ctx, implementation.as_ref().unwrap(), !named_component);
        model::set(&schema, "items", impl_schema.map(V::Obj));
        return schema;
    }
    let declared: Vec<String> = a
        .enum_value("type")
        .map(|t| vec![util::schema_type_name(&t).to_string()])
        .unwrap_or_default();
    include_type_schema(ctx, schema, implementation.as_ref(), &declared)
}

fn num_value(v: &AVal) -> Option<V> {
    match v {
        AVal::Int(i) => Some(V::Dec(format!("{i}.0"))),
        AVal::Float(f) => Some(V::Dec(f.clone())),
        _ => None,
    }
}

/// `SchemaFactory.includeTypeSchema`.
pub fn include_type_schema(ctx: &mut Ctx, schema: M, t: Option<&Type>, declared: &[String]) -> M {
    if let Some(t) = t
        && let Some(impl_schema) = read_class_schema(ctx, t, false)
    {
        if declared.iter().any(|d| d == "array") {
            let r = lookup_ref(ctx, t, impl_schema);
            model::set_v(&schema, "items", V::Obj(r));
            return schema;
        }
        return model::merge(Some(impl_schema), Some(schema)).unwrap();
    }
    schema
}

fn read_discriminator(ctx: &mut Ctx, a: &Anno) -> Option<M> {
    let property = a.str("discriminatorProperty");
    let mapping = a
        .value("discriminatorMapping")
        .map(|_| a.nested_array("discriminatorMapping"));
    if property.is_none() && mapping.is_none() {
        return None;
    }
    let d = model::new(MK::Discriminator);
    if let Some(p) = property {
        model::set_v(&d, "propertyName", V::Str(p));
    }
    if let Some(mapping) = mapping {
        for m in mapping {
            let mut value = m.str("value");
            let schema_ref = match m.class("schema") {
                Some(t) => type_to_schema(ctx, &t, None).and_then(|s| model::get_ref(&s)),
                None => None,
            };
            if value.is_none()
                && let Some(r) = &schema_ref
            {
                value = Some(r.rsplit('/').next().unwrap_or(r).to_string());
            }
            if let (Some(v), Some(r)) = (value, schema_ref) {
                model::put_map(&d, "mapping", &v, V::Str(r));
            }
        }
    }
    Some(d)
}

// ---------------------------------------------------------------- OpenApiDataObjectScanner

struct PathEntry {
    parent: Option<Rc<PathEntryCell>>,
    target: Option<Target>,
    class: Option<ClassId>,
    /// Name of the class if the index contains it (also for indexed JDK classes), used for cycle detection.
    key: Option<String>,
    ty: Type,
}

/// A path entry with its (replaceable) schema.
pub struct PathEntryCell {
    entry: PathEntry,
    schema: std::cell::RefCell<M>,
}

impl PathEntryCell {
    fn schema(&self) -> M {
        self.schema.borrow().clone()
    }
    fn same(&self, other: &PathEntry) -> bool {
        if self.entry.key != other.key {
            return false;
        }
        if self.entry.ty.kind() == Kind::Parameterized && other.ty.kind() == Kind::Parameterized {
            return self.entry.ty.args() == other.ty.args();
        }
        true
    }
    fn has_parent(self: &Rc<Self>, candidate: &PathEntry) -> bool {
        let mut test = Some(self.clone());
        while let Some(t) = test {
            if t.same(candidate) {
                return true;
            }
            test = t.entry.parent.clone();
        }
        false
    }
}

pub struct ObjectStack {
    path: Vec<Rc<PathEntryCell>>,
}

impl ObjectStack {
    fn push_entry(&mut self, e: Rc<PathEntryCell>) {
        self.path.push(e);
    }
    /// `DataObjectDeque.push(annotationTarget, parent, type, schema)` with cycle detection.
    fn push(
        &mut self,
        ctx: &Ctx,
        target: Option<Target>,
        parent: &Rc<PathEntryCell>,
        ty: &Type,
        schema: &M,
    ) {
        let name = util::get_name(ty);
        let class = ctx.idx.id(&name);
        let key = if ctx.idx.contains(&name) {
            Some(name)
        } else {
            None
        };
        let entry = PathEntry {
            parent: Some(parent.clone()),
            target,
            class,
            key,
            ty: ty.clone(),
        };
        if parent.has_parent(&entry) {
            if !model::has(schema, "description") {
                let name = entry.key.clone().unwrap_or_default();
                model::set_v(
                    schema,
                    "description",
                    V::Str(format!("Cyclic reference to {name}")),
                );
            }
        } else {
            self.push_entry(Rc::new(PathEntryCell {
                entry,
                schema: std::cell::RefCell::new(schema.clone()),
            }));
        }
    }
}

enum Standin {
    Iterable,
    Map,
    Stream,
}

/// `OpenApiDataObjectScanner.process(context, type)`.
pub fn data_object_process(ctx: &mut Ctx, t: &Type) -> M {
    ctx.scan_stack.push(t.clone());
    let result = data_object_scan(ctx, t);
    ctx.scan_stack.pop();
    result
}

fn data_object_scan(ctx: &mut Ctx, t: &Type) -> M {
    let idx = ctx.idx;
    if util::is_terminal(t) {
        let s = model::schema();
        util::apply_type_attributes(t, &s, None);
        return s;
    }
    if util::is_a(idx, t, util::ENUM)
        && idx.contains(&util::get_name(t))
        && idx.class(&util::get_name(t)).is_some()
    {
        return enum_to_schema(ctx, t);
    }
    let standin = if util::is_a(idx, t, util::ITERABLE) {
        Some(Standin::Iterable)
    } else if util::is_a(idx, t, util::MAP) {
        Some(Standin::Map)
    } else if util::is_a(idx, t, util::BASE_STREAM) {
        Some(Standin::Stream)
    } else {
        None
    };
    let root_class = if standin.is_some() {
        None
    } else {
        idx.id(&util::get_name(t))
    };
    if standin.is_none() && root_class.is_none() {
        let s = model::schema();
        model::add_type(&s, "object");
        return s;
    }
    let root_schema = model::schema();
    let root = Rc::new(PathEntryCell {
        entry: PathEntry {
            parent: None,
            target: None,
            class: root_class,
            key: if standin.is_some() {
                None
            } else {
                Some(util::get_name(t)).filter(|n| idx.contains(n))
            },
            ty: t.clone(),
        },
        schema: std::cell::RefCell::new(root_schema.clone()),
    });
    let mut stack = ObjectStack { path: Vec::new() };
    let mut result = root_schema;
    if let Some(standin) = standin {
        result = resolve_special(ctx, &mut stack, &root, t, standin);
    } else {
        stack.push_entry(root.clone());
    }
    dfs(ctx, &mut stack);
    result
}

/// `OpenApiDataObjectScanner.resolveSpecial`: containers are scanned through stand-in classes
/// (`IterableStandin<E>`, `MapStandin<K, V>`, `StreamStandin<E, S>`) whose `value` property has the container type.
fn resolve_special(
    ctx: &mut Ctx,
    stack: &mut ObjectStack,
    root: &Rc<PathEntryCell>,
    t: &Type,
    standin: Standin,
) -> M {
    let (seek, standin_params) = match standin {
        Standin::Iterable => (util::ITERABLE, 1),
        Standin::Map => (util::MAP, 2),
        Standin::Stream => (util::BASE_STREAM, 2),
    };
    let mut ty = t.clone();
    if (t.kind() != Kind::Parameterized || t.args().len() != standin_params)
        && let Some(p) = resolve_parameterized_ancestor(ctx, t, seek)
    {
        ty = p;
    }
    let resolver = FieldResolver {
        stack: Vec::new(),
        prop: None,
        name: "value".into(),
    };
    let annotation_target = ctx.idx.id(&ty.name()).map(Target::Class);
    process_field(ctx, stack, root, &resolver, annotation_target, ty)
}

/// `TypeResolver.resolveParameterizedAncestor`.
fn resolve_parameterized_ancestor(ctx: &Ctx, t: &Type, seek: &str) -> Option<Type> {
    let idx = ctx.idx;
    let mut cursor = t.clone();
    let mut seek_continue = true;
    let mut guard = 0;
    while seek_continue && guard < 32 {
        guard += 1;
        let Some(cid) = idx.id(&cursor.name()) else {
            break;
        };
        let ci = &idx.classes[cid];
        let resolution = props::param_resolution_map(idx, cid, &cursor);
        let interfaces: Vec<Type> = ci
            .interfaces
            .iter()
            .filter(|i| util::is_a(idx, i, seek))
            .cloned()
            .collect();
        for it in &interfaces {
            cursor = create_param(it, &resolution);
            if it.name() == seek {
                seek_continue = false;
                break;
            }
        }
        if interfaces.is_empty() {
            match &ci.super_type {
                Some(st) if util::is_a(idx, st, seek) => cursor = create_param(st, &resolution),
                _ => seek_continue = false,
            }
        }
    }
    if cursor.kind() == Kind::Parameterized {
        Some(cursor)
    } else {
        None
    }
}

fn create_param(t: &Type, resolution: &std::collections::HashMap<String, Type>) -> Type {
    let args = t
        .args()
        .iter()
        .map(|a| match &a.ty {
            Ty::Var { id, .. } => resolution.get(id).cloned().unwrap_or_else(|| a.clone()),
            Ty::Param { .. } => create_param(a, resolution),
            _ => a.clone(),
        })
        .collect();
    Type::new(Ty::Param {
        name: t.name(),
        args,
        owner: None,
    })
}

fn dfs(ctx: &mut Ctx, stack: &mut ObjectStack) {
    let idx = ctx.idx;
    while let Some(entry) = stack.path.pop() {
        let current_type = entry.entry.ty.clone();
        let entry_schema = entry.schema();
        // configuresVisibility(currentTarget) is only true for @JsonIgnoreProperties, which is not supported
        let allow_registration = true;
        if allow_registration && ctx.registry.has_schema(&current_type) {
            let r = ctx.registry.lookup_ref(&current_type);
            model::set_ref(&entry_schema, model::get_ref(&r).as_deref());
            continue;
        }
        let current_class = entry.entry.class;
        let mut current_schema = entry.schema();
        current_schema = read_klass(
            ctx,
            current_class,
            &current_type,
            current_schema,
            allow_registration,
        );
        if let Some(c) = current_class {
            util::map_deprecated(idx, Some(Target::Class(c)), &current_schema);
        }
        *entry.schema.borrow_mut() = current_schema.clone();
        if model::get_ref(&current_schema).is_some() {
            continue;
        }
        if model::non_null_type(&current_schema).is_none()
            && !is_implementation_void(ctx, entry.entry.target, current_class)
        {
            model::set_type(&current_schema, Some("object"));
        } else if allow_registration {
            maybe_register_schema(ctx, &current_type, &current_schema, &entry_schema);
        }
        let types = model::types(&current_schema).unwrap_or_default();
        if types.iter().any(|t| t == "object")
            && let Some(class) = current_class
        {
            let properties = props::all_fields(ctx, &current_type, class);
            for p in properties {
                if !p.is_ignored(idx) {
                    let resolver = FieldResolver {
                        stack: p.stack.clone(),
                        name: String::new(),
                        prop: Some(p.clone()),
                    };
                    let target = Some(p.target(idx));
                    let entity = p.unresolved_type(idx);
                    process_field(ctx, stack, &entry, &resolver, target, entity);
                }
            }
            // inheritance: only `@Schema(allOf)` handling, auto-inheritance is NONE
        }
    }
}

fn is_implementation_void(ctx: &Ctx, target: Option<Target>, class: Option<ClassId>) -> bool {
    let candidates = [target, class.map(Target::Class)];
    for t in candidates.into_iter().flatten() {
        if let Some(a) = util::ann(ctx.idx, &AT::Decl(t), SCHEMA)
            && let Some(i) = a.class("implementation")
        {
            return util::is_void(&i);
        }
    }
    false
}

fn maybe_register_schema(ctx: &mut Ctx, t: &Type, current: &M, entry_schema: &M) {
    let r = schema_registration(ctx, t, current.clone());
    let ty = model::non_null_type(current);
    if !Rc::ptr_eq(&r, current) && ty.as_deref() != Some("object") && model::get_ref(&r).is_some() {
        let target = ctx.registry.lookup_schema(t);
        if !target.is_some_and(|ts| Rc::ptr_eq(&ts, entry_schema)) {
            let keys: Vec<String> = entry_schema.borrow().props.keys().cloned().collect();
            for k in keys {
                model::set(entry_schema, &k, None);
            }
            model::set_ref(entry_schema, model::get_ref(&r).as_deref());
        }
    }
}

fn read_klass(ctx: &mut Ctx, class: Option<ClassId>, t: &Type, schema: M, register: bool) -> M {
    let Some(c) = class else { return schema };
    let anno = ctx.idx.classes[c].annotation(SCHEMA).cloned();
    if let Some(a) = anno {
        read_schema_for_class(ctx, schema, &a, c, register, &Defaults::default())
    } else if util::is_a(ctx.idx, t, util::ENUM) {
        enum_to_schema(ctx, t)
    } else {
        schema
    }
}

// ---------------------------------------------------------------- AnnotationTargetProcessor / TypeProcessor

/// The property being processed, or a stand-in value.
pub struct FieldResolver {
    stack: ResolutionStack,
    prop: Option<Prop>,
    name: String,
}

impl FieldResolver {
    fn resolve(&self, t: &Type) -> Type {
        props::resolve_type(t, &self.stack, true)
    }
    fn property_name(&self, ctx: &Ctx) -> String {
        match &self.prop {
            Some(p) => p.name(ctx.idx),
            None => self.name.clone(),
        }
    }
}

struct TypeProc<'s> {
    schema: M,
    ty: Type,
    target: Option<Target>,
    resolver: &'s FieldResolver,
    inputs: Vec<(Type, M)>,
}

impl TypeProc<'_> {
    fn process_type(&mut self, ctx: &mut Ctx) -> Type {
        let idx = ctx.idx;
        if util::is_terminal(&self.ty) {
            registry_register_with(
                ctx,
                &self.ty.clone(),
                Some(&self.resolver.stack),
                &self.schema,
                RegAction::Check,
            );
            return self.ty.clone();
        }
        self.ty = self.maybe_resolve(ctx, &self.ty.clone());
        let ty = self.ty.clone();
        if ty.kind() == Kind::Array && self.is_array_type(ctx) {
            let schema = self.schema.clone();
            return self.read_array(ctx, &ty, &schema);
        }
        if util::is_a(idx, &ty, util::ENUM) && idx.class(&util::get_name(&ty)).is_some() {
            let e = enum_to_schema(ctx, &ty);
            model::merge(Some(self.schema.clone()), Some(e));
            self.inputs.push((ty, self.schema.clone()));
            return Type::class("java.lang.String");
        }
        if ty.kind() == Kind::Parameterized {
            let schema = self.schema.clone();
            return self.read_parameterized(ctx, &ty, &schema);
        }
        for (seek, fallback) in [
            (util::ITERABLE, true),
            (util::BASE_STREAM, true),
            (util::MAP, false),
        ] {
            if util::is_a(idx, &ty, seek) {
                return match resolve_parameterized_ancestor(ctx, &ty, seek) {
                    Some(p) => {
                        let schema = self.schema.clone();
                        self.read_parameterized(ctx, &p, &schema)
                    }
                    None if fallback => Type::new(Ty::Array(Box::new(Type::class(MAP)), 1)),
                    None => Type::class(MAP),
                };
            }
        }
        if idx.contains(&util::get_name(&ty)) {
            self.inputs.push((ty.clone(), self.schema.clone()));
        }
        ty
    }

    fn is_array_type(&self, ctx: &Ctx) -> bool {
        if let Some(t) = self.target
            && let Some(a) = util::ann(ctx.idx, &AT::Decl(t), SCHEMA)
            && let Some(st) = a.enum_value("type")
        {
            return st == "ARRAY";
        }
        true
    }

    fn maybe_resolve(&mut self, ctx: &mut Ctx, t: &Type) -> Type {
        let mut t = t.clone();
        if t.kind() == Kind::Wildcard {
            t = util::resolve_wildcard(&t);
        }
        if t.kind() == Kind::TypeVariable {
            let schema = self.schema.clone();
            t = self.resolve_type_variable(ctx, &schema, &t, false);
        }
        if util::is_wrapped(&t) {
            t = util::unwrap(&t);
        }
        t
    }

    fn resolve_type_variable(&mut self, ctx: &mut Ctx, schema: &M, t: &Type, push: bool) -> Type {
        let resolved = self.resolver.resolve(t);
        if util::is_terminal(&resolved) || !ctx.idx.contains(&util::get_name(&resolved)) {
            util::apply_type_attributes(&resolved, schema, None);
        } else if push {
            self.inputs.push((resolved.clone(), schema.clone()));
        }
        resolved
    }

    fn read_array(&mut self, ctx: &mut Ctx, t: &Type, array_schema: &M) -> Type {
        let Ty::Array(component, dims) = &t.ty else {
            unreachable!()
        };
        let mut item = model::schema();
        model::add_type(array_schema, "array");
        let mut component = self.resolver.resolve(component);
        let optional = util::is_optional(&component);
        if optional {
            component = util::unwrap(&component);
        }
        util::apply_type_attributes(&component, &item, None);
        if !util::is_terminal(&component) && ctx.idx.contains(&util::get_name(&component)) {
            self.inputs.push((component.clone(), item.clone()));
            item = registry_register_with(
                ctx,
                &component,
                Some(&self.resolver.stack),
                &item,
                RegAction::Reference,
            )
            .unwrap();
        } else {
            item = registry_register_with(
                ctx,
                &component,
                Some(&self.resolver.stack),
                &item,
                RegAction::Check,
            )
            .unwrap();
        }
        let mut d = *dims;
        while d > 1 {
            let parent = model::schema();
            model::add_type(&parent, "array");
            model::set_v(&parent, "items", V::Obj(item));
            item = parent;
            d -= 1;
        }
        if optional {
            item = wrap_optional_item(item);
        }
        model::set_v(array_schema, "items", V::Obj(item));
        Type::new(Ty::Array(Box::new(component.clone()), d))
    }

    fn read_parameterized(&mut self, ctx: &mut Ctx, p: &Type, schema: &M) -> Type {
        let idx = ctx.idx;
        let seek = if util::is_a(idx, p, util::ITERABLE) {
            Some(util::ITERABLE)
        } else if util::is_a(idx, p, util::MAP) {
            Some(util::MAP)
        } else if util::is_a(idx, p, util::BASE_STREAM) {
            Some(util::BASE_STREAM)
        } else {
            None
        };
        match seek {
            Some(s) if s != util::MAP => {
                model::set_type(schema, Some("array"));
                let ancestor =
                    resolve_parameterized_ancestor(ctx, p, s).unwrap_or_else(|| p.clone());
                if util::is_a(idx, p, util::SET) {
                    model::set_v(schema, "uniqueItems", V::Bool(true));
                }
                let mut value_type = ancestor
                    .args()
                    .first()
                    .cloned()
                    .unwrap_or_else(|| Type::class(OBJECT));
                let optional = util::is_optional(&value_type);
                if optional {
                    value_type = util::unwrap(&value_type);
                }
                let mut value_schema = self.read_generic_value_type(ctx, &value_type);
                if optional {
                    value_schema = wrap_optional_item(value_schema);
                }
                model::set_v(schema, "items", V::Obj(value_schema));
                Type::new(Ty::Array(Box::new(Type::class(MAP)), 1))
            }
            Some(_) => {
                model::set_type(schema, Some("object"));
                let ancestor =
                    resolve_parameterized_ancestor(ctx, p, util::MAP).unwrap_or_else(|| p.clone());
                if ancestor.args().len() == 2 {
                    let value_type = ancestor.args()[1].clone();
                    let v = self.read_generic_value_type(ctx, &value_type);
                    model::set_v(schema, "additionalProperties", V::Obj(v));
                }
                if util::allow_registration(idx, p) {
                    let resolved = self.resolver.resolve(p);
                    self.inputs.push((resolved, schema.clone()));
                }
                Type::class(MAP)
            }
            None => {
                if idx.contains(&util::get_name(p)) {
                    let resolved = self.resolver.resolve(p);
                    self.inputs.push((resolved, schema.clone()));
                }
                p.clone()
            }
        }
    }

    fn read_generic_value_type(&mut self, ctx: &mut Ctx, value_type: &Type) -> M {
        let mut value_schema = model::schema();
        if util::is_terminal(value_type) {
            util::apply_type_attributes(value_type, &value_schema, None);
        } else if value_type.kind() == Kind::Parameterized {
            let vs = value_schema.clone();
            self.read_parameterized(ctx, value_type, &vs);
        } else {
            value_schema = self.resolve_parameterized_type(ctx, value_type, value_schema);
        }
        beanval::apply_to_type(ctx.idx, value_type, Some(&value_schema));
        value_schema
    }

    fn resolve_parameterized_type(
        &mut self,
        ctx: &mut Ctx,
        value_type: &Type,
        props_schema: M,
    ) -> M {
        let idx = ctx.idx;
        match value_type.kind() {
            Kind::TypeVariable | Kind::Wildcard => {
                let resolved = self.resolve_type_variable(ctx, &props_schema, value_type, true);
                if idx.contains(&util::get_name(&resolved)) {
                    model::set_type(&props_schema, Some("object"));
                    return registry_register_with(
                        ctx,
                        value_type,
                        Some(&self.resolver.stack),
                        &props_schema,
                        RegAction::Reference,
                    )
                    .unwrap();
                }
                props_schema
            }
            Kind::Array => {
                let ps = props_schema.clone();
                self.read_array(ctx, value_type, &ps);
                props_schema
            }
            _ if idx.contains(&util::get_name(value_type)) => {
                let mut ps = props_schema;
                if util::is_a(idx, value_type, util::ENUM) {
                    ps = enum_to_schema(ctx, value_type);
                } else {
                    model::set_type(&ps, Some("object"));
                }
                if ctx.registry.has_schema(&self.resolver.resolve(value_type)) {
                    return ctx.registry.lookup_ref(value_type);
                }
                self.inputs.push((value_type.clone(), ps.clone()));
                registry_register_with(
                    ctx,
                    value_type,
                    Some(&self.resolver.stack),
                    &ps,
                    RegAction::Reference,
                )
                .unwrap()
            }
            _ => props_schema,
        }
    }

    fn push_inputs(&mut self, ctx: &Ctx, stack: &mut ObjectStack, parent: &Rc<PathEntryCell>) {
        for (t, s) in std::mem::take(&mut self.inputs) {
            stack.push(ctx, self.target, parent, &t, &s);
        }
    }
}

fn wrap_optional_item(item: M) -> M {
    let s = model::schema();
    model::add_any_of(&s, model::null_schema());
    model::add_any_of(&s, item);
    s
}

const ASSERTIONS: &[&str] = &[
    "additionalProperties",
    "allOf",
    "anyOf",
    "discriminator",
    "enum",
    "exclusiveMaximum",
    "exclusiveMinimum",
    "format",
    "items",
    "maximum",
    "maxItems",
    "maxLength",
    "maxProperties",
    "minimum",
    "minItems",
    "minLength",
    "minProperties",
    "multipleOf",
    "not",
    "oneOf",
    "pattern",
    "properties",
    "$ref",
    "required",
    "uniqueItems",
    "xml",
];
const ANNOTATION_PROPS: &[&str] = &[
    "default",
    "deprecated",
    "description",
    "example",
    "examples",
    "externalDocs",
    "readOnly",
    "title",
    "writeOnly",
];

fn assertion_conflicts(field: &M, ty: &M) -> bool {
    ASSERTIONS
        .iter()
        .any(|k| match (model::get(field, k), model::get(ty, k)) {
            (Some(f), Some(t)) => f != t,
            _ => false,
        })
}

fn assertions_override(field: &M, ty: &M) -> bool {
    ASSERTIONS.iter().any(|k| match model::get(field, k) {
        Some(f) => Some(f) != model::get(ty, k),
        None => false,
    })
}

fn specifies_annotation(field: &M) -> bool {
    ANNOTATION_PROPS.iter().any(|k| model::has(field, k))
        || model::extensions(field)
            .iter()
            .any(|(k, _)| !k.starts_with(model::PRIVATE_EXT))
}

fn is_nullable(s: &M) -> bool {
    model::get_nullable(s) == Some(true)
}

/// `AnnotationTargetProcessor.processField`.
fn process_field(
    ctx: &mut Ctx,
    stack: &mut ObjectStack,
    parent: &Rc<PathEntryCell>,
    resolver: &FieldResolver,
    target: Option<Target>,
    entity_type: Type,
) -> M {
    let idx = ctx.idx;
    let schema_anno = target.and_then(|t| util::ann(idx, &AT::Decl(t), SCHEMA));
    let property_key = resolver.property_name(ctx);
    let mut type_proc: Option<TypeProc> = None;
    let type_schema: Option<M>;
    let field_type: Type;
    let registration_type: Option<Type>;
    let registration_candidate: bool;
    if let Some(a) = schema_anno.as_ref().filter(|a| util::has_implementation(a)) {
        type_schema = None;
        field_type = a.class("implementation").unwrap();
        registration_type = None;
        registration_candidate = false;
    } else {
        let mut tp = TypeProc {
            schema: model::schema(),
            ty: entity_type.clone(),
            target,
            resolver,
            inputs: Vec::new(),
        };
        field_type = tp.process_type(ctx);
        let init = tp.schema.clone();
        if !util::is_type_overridden(&field_type, schema_anno.as_ref()) {
            util::apply_type_attributes(&field_type, &init, schema_anno.as_ref());
        }
        let reg_type = if util::is_wrapped(&entity_type) {
            field_type.clone()
        } else {
            entity_type.clone()
        };
        let candidate = !util::is_ref_anno(schema_anno.as_ref())
            && registry_register_with(
                ctx,
                &reg_type,
                Some(&resolver.stack),
                &init,
                RegAction::None,
            )
            .is_none_or(|r| !Rc::ptr_eq(&r, &init));
        let resolved_reg = resolver.resolve(&reg_type);
        type_schema = if candidate && ctx.registry.has_schema(&resolved_reg) {
            ctx.registry.lookup_schema(&resolved_reg)
        } else {
            Some(init)
        };
        registration_type = Some(reg_type);
        registration_candidate = candidate;
        type_proc = Some(tp);
    }

    let parent_schema = parent.schema();
    let mut field_schema = if let Some(a) = &schema_anno {
        read_schema_annotated_field(ctx, &parent_schema, &property_key, a, &field_type)
    } else if registration_candidate {
        let s = model::schema();
        model::set(
            &s,
            "type",
            model::get(type_schema.as_ref().unwrap(), "type"),
        );
        s
    } else {
        model::merge(Some(model::schema()), type_schema.clone()).unwrap()
    };

    if let Some(p) = &resolver.prop {
        for ct in &p.constraint_targets {
            beanval::apply(
                idx,
                &AT::Decl(*ct),
                &field_schema,
                Some(&property_key),
                &Req::Parent {
                    parent: &parent_schema,
                },
            );
        }
        if model::get_nullable(&field_schema).is_none() && util::is_optional(&entity_type) {
            model::set_nullable(&field_schema, true);
        }
        if !model::has(&field_schema, "readOnly") && p.read_only {
            model::set_v(&field_schema, "readOnly", V::Bool(true));
        }
        if !model::has(&field_schema, "writeOnly") && p.write_only {
            model::set_v(&field_schema, "writeOnly", V::Bool(true));
        }
    } else if model::get_nullable(&field_schema).is_none() && util::is_optional(&entity_type) {
        model::set_nullable(&field_schema, true);
    }
    util::map_deprecated(idx, target, &field_schema);

    if let Some(existing) = model::property(&parent_schema, &property_key) {
        field_schema = model::merge(Some(field_schema), Some(existing)).unwrap();
    }

    if registration_candidate {
        let ts = type_schema.clone().unwrap();
        let reg_type = registration_type.clone().unwrap();
        if assertion_conflicts(&field_schema, &ts) {
            let declared = model::types(&field_schema).unwrap_or_default();
            field_schema = include_type_schema(ctx, field_schema, Some(&field_type), &declared);
        } else {
            if let Some(tp) = type_proc.as_mut() {
                tp.push_inputs(ctx, stack, parent);
            }
            let type_list = model::types(&ts);
            let registered = if type_list
                .as_ref()
                .is_some_and(|l| !l.iter().any(|t| t == "array"))
            {
                registry_register_with(
                    ctx,
                    &reg_type,
                    Some(&resolver.stack),
                    &ts,
                    RegAction::Reference,
                )
                .unwrap()
            } else {
                registry_register_with(ctx, &reg_type, Some(&resolver.stack), &ts, RegAction::Check)
                    .unwrap()
            };
            let field_nullable = is_nullable(&field_schema);
            if model::get_ref(&field_schema).is_none()
                && (assertions_override(&field_schema, &ts) || specifies_annotation(&field_schema))
            {
                util::clear_matching_defaults(&field_schema, &ts);
                model::set_ref(&field_schema, model::get_ref(&registered).as_deref());
                model::add_type_observer(&ts, &field_schema);
            } else {
                field_schema = registered;
            }
            if field_nullable && !is_nullable(&ts) {
                let ref_schema = model::schema_ref(&model::get_ref(&field_schema).unwrap());
                model::set_ref(&field_schema, None);
                if !model::has(&field_schema, "anyOf") {
                    model::add_any_of(&field_schema, ref_schema);
                    model::add_any_of(&field_schema, model::null_schema());
                } else {
                    let any = model::schema();
                    model::add_any_of(&any, ref_schema);
                    model::add_any_of(&any, model::null_schema());
                    model::add_all_of(&field_schema, any);
                }
            }
        }
    } else if !util::is_ref_anno(schema_anno.as_ref()) {
        if let Some(tp) = type_proc.as_mut() {
            tp.push_inputs(ctx, stack, parent);
        }
        field_schema = model::merge(type_schema.clone(), Some(field_schema)).unwrap();
    }
    model::add_property(&parent_schema, &property_key, field_schema.clone());
    field_schema
}

fn read_schema_annotated_field(
    ctx: &mut Ctx,
    parent: &M,
    key: &str,
    a: &Anno,
    field_type: &Type,
) -> M {
    if a.bool("required") == Some(true) {
        model::add_required(parent, key);
    }
    let defaults = if util::is_boolean_schema(a)
        || util::is_array_schema(a)
        || util::is_type_overridden(field_type, Some(a))
    {
        Defaults::default()
    } else {
        util::type_defaults(field_type)
    };
    read_schema(ctx, model::schema(), a, &defaults)
}
