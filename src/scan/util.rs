//! Port of SmallRye's `TypeUtil` and `Annotations` helpers.

use std::collections::HashSet;

use crate::index::{Anno, ClassId, Index, Kind, OBJECT, Target, Ty, Type, known};
use crate::model::{self, M, V};

pub const SCHEMA: &str = "org.eclipse.microprofile.openapi.annotations.media.Schema";
pub const ENUM: &str = "java.lang.Enum";
pub const ITERABLE: &str = "java.lang.Iterable";
pub const MAP: &str = "java.util.Map";
pub const SET: &str = "java.util.Set";
pub const BASE_STREAM: &str = "java.util.stream.BaseStream";
pub const DEPRECATED: &str = "java.lang.Deprecated";

/// A target of annotation lookups: a declaration or a type (for type-use annotations).
#[derive(Clone, Debug)]
pub enum AT<'a> {
    Decl(Target),
    Type(&'a Type),
}

/// `Annotations.getAnnotation`: declared annotation, or one composed via an indexed annotation type.
pub fn ann(idx: &Index, target: &AT, name: &str) -> Option<Anno> {
    let declared: &[Anno] = match target {
        AT::Decl(t) => idx.declared(*t),
        AT::Type(t) => &t.annotations,
    };
    if let Some(a) = declared.iter().find(|a| a.name == name) {
        return Some(a.clone());
    }
    let mut scanned = HashSet::new();
    find_composed(idx, declared, name, &mut scanned)
}

/// Annotation `name` declared on an application annotation type that is used on the target.
fn find_composed(
    idx: &Index,
    declared: &[Anno],
    name: &str,
    scanned: &mut HashSet<String>,
) -> Option<Anno> {
    for a in declared {
        if a.name.starts_with("java.")
            || a.name.starts_with("jakarta.")
            || a.name.starts_with("kotlin.")
        {
            continue;
        }
        let Some(ci) = idx.class(&a.name) else {
            continue;
        };
        if !scanned.insert(ci.name.clone()) {
            continue;
        }
        if let Some(found) = ci.annotations.iter().find(|x| x.name == name) {
            return Some(found.clone());
        }
        if let Some(found) = find_composed(idx, &ci.annotations, name, scanned) {
            return Some(found);
        }
    }
    None
}

pub fn has(idx: &Index, target: &AT, name: &str) -> bool {
    ann(idx, target, name).is_some()
}

pub fn ann_any(idx: &Index, target: &AT, names: &[&str]) -> Option<Anno> {
    names.iter().find_map(|n| ann(idx, target, n))
}

// ---------------------------------------------------------------- type formats

pub struct Format {
    pub ty: Option<&'static str>,
    pub format: Option<&'static str>,
    pub pattern: Option<&'static str>,
    pub example: Option<&'static str>,
    pub ext_docs: Option<(&'static str, &'static str)>,
    pub opaque: bool,
}

const UUID_PATTERN: &str =
    "[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}";

impl Format {
    const fn of(ty: &'static str) -> Format {
        Format {
            ty: Some(ty),
            format: None,
            pattern: None,
            example: None,
            ext_docs: None,
            opaque: false,
        }
    }
    const fn fmt(ty: &'static str, format: &'static str) -> Format {
        Format {
            ty: Some(ty),
            format: Some(format),
            pattern: None,
            example: None,
            ext_docs: None,
            opaque: false,
        }
    }
    /// Number of entries of `TypeWithFormat.getProperties()`.
    pub fn property_count(&self) -> usize {
        1 + self.format.is_some() as usize
            + self.pattern.is_some() as usize
            + self.example.is_some() as usize
            + self.ext_docs.is_some() as usize
    }
    pub fn is_array_or_object(&self) -> bool {
        matches!(self.ty, Some("array") | Some("object"))
    }
}

pub fn type_format(t: &Type) -> Format {
    let name = if t.kind() == Kind::Array {
        t.name()
    } else {
        get_name(t)
    };
    match format_for(&name) {
        Some(f) => f,
        None if t.kind() == Kind::Array => Format::of("array"),
        None => Format::of("object"),
    }
}

fn format_for(name: &str) -> Option<Format> {
    Some(match name {
        "java.lang.Object" | "java.lang.Void" => Format {
            ty: None,
            format: None,
            pattern: None,
            example: None,
            ext_docs: None,
            opaque: false,
        },
        "java.lang.String"
        | "java.lang.StringBuffer"
        | "java.lang.StringBuilder"
        | "java.lang.CharSequence"
        | "java.net.URL" => Format::of("string"),
        "java.net.URI" => Format::fmt("string", "uri"),
        "java.util.UUID" => Format {
            pattern: Some(UUID_PATTERN),
            ..Format::fmt("string", "uuid")
        },
        "java.lang.Character" | "char" => Format::fmt("string", "char"),
        "[B"
        | "java.io.File"
        | "java.io.InputStream"
        | "org.jboss.resteasy.reactive.multipart.FileUpload" => Format::fmt("string", "binary"),
        "java.lang.Number" | "java.math.BigDecimal" => Format::of("number"),
        "java.lang.Double" | "double" => Format::fmt("number", "double"),
        "java.lang.Float" | "float" => Format::fmt("number", "float"),
        "java.math.BigInteger" => Format::of("integer"),
        "java.lang.Integer" | "int" => Format::fmt("integer", "int32"),
        "java.lang.Long" | "long" => Format::fmt("integer", "int64"),
        "java.lang.Short" | "short" => Format::fmt("integer", "int16"),
        "java.lang.Byte" | "byte" => Format::fmt("integer", "int8"),
        "java.lang.Boolean" | "boolean" => Format::of("boolean"),
        "java.util.Date" | "java.sql.Date" | "java.time.LocalDate" => Format {
            example: Some("2022-03-10"),
            ..Format::fmt("string", "date")
        },
        "java.time.LocalDateTime" => Format {
            example: Some("2022-03-10T12:15:50"),
            ..Format::fmt("string", "date-time")
        },
        "java.time.ZonedDateTime" | "java.time.OffsetDateTime" => Format {
            example: Some("2022-03-10T12:15:50-04:00"),
            ..Format::fmt("string", "date-time")
        },
        "java.time.Instant" => Format {
            example: Some("2022-03-10T16:15:50Z"),
            ..Format::fmt("string", "date-time")
        },
        "java.time.Duration" | "java.time.Period" => Format {
            example: Some("P1D"),
            ..Format::fmt("string", "duration")
        },
        "java.time.LocalTime" => Format {
            example: Some("13:45:30.123456789"),
            ext_docs: Some((
                "As defined by 'partial-time' in RFC3339",
                "https://www.rfc-editor.org/rfc/rfc3339.html#section-5.6",
            )),
            ..Format::fmt("string", "local-time")
        },
        "java.time.OffsetTime" => Format {
            example: Some("13:45:30.123456789+02:00"),
            ext_docs: Some((
                "As defined by 'full-time' in RFC3339",
                "https://www.rfc-editor.org/rfc/rfc3339.html#section-5.6",
            )),
            ..Format::fmt("string", "time")
        },
        "jakarta.json.JsonArray" | "javax.json.JsonArray" => Format {
            opaque: true,
            ..Format::of("array")
        },
        "jakarta.json.JsonNumber" | "javax.json.JsonNumber" => Format::of("number"),
        "jakarta.json.JsonObject" | "javax.json.JsonObject" => Format {
            opaque: true,
            ..Format::of("object")
        },
        "jakarta.json.JsonString" | "javax.json.JsonString" => Format::of("string"),
        _ => return None,
    })
}

/// `TypeUtil.getName`.
pub fn get_name(t: &Type) -> String {
    match &t.ty {
        Ty::Array(c, _) => c.name(),
        Ty::Wildcard { .. } => resolve_wildcard(t).name(),
        _ => t.name(),
    }
}

pub fn resolve_wildcard(t: &Type) -> Type {
    match &t.ty {
        Ty::Wildcard { extends, super_ } => match (extends, super_) {
            (Some(e), _) => (**e).clone(),
            (None, Some(s)) => (**s).clone(),
            _ => Type::class(OBJECT),
        },
        _ => t.clone(),
    }
}

pub fn is_terminal(t: &Type) -> bool {
    match t.kind() {
        Kind::Primitive | Kind::Void => true,
        Kind::TypeVariable => false,
        Kind::Wildcard => is_terminal(&resolve_wildcard(t)),
        Kind::Class if t.name() == OBJECT => true,
        _ => !type_format(t).is_array_or_object(),
    }
}

pub fn known_java_type(name: &str) -> bool {
    known::known_java_type(name)
}

/// `TypeUtil.allowRegistration`.
pub fn allow_registration(idx: &Index, t: &Type) -> bool {
    let f = type_format(t);
    if f.is_array_or_object() {
        return !f.opaque && !known_java_type(&t.name()) && idx.contains(&t.name());
    }
    f.property_count() > 2
}

pub fn is_optional(t: &Type) -> bool {
    matches!(
        t.name().as_str(),
        "java.util.Optional"
            | "java.util.OptionalDouble"
            | "java.util.OptionalInt"
            | "java.util.OptionalLong"
    )
}

pub fn optional_type(t: &Type) -> Type {
    match t.name().as_str() {
        "java.util.Optional" => t
            .args()
            .first()
            .cloned()
            .unwrap_or_else(|| Type::class(OBJECT)),
        "java.util.OptionalDouble" => Type::new(Ty::Prim("double")),
        "java.util.OptionalInt" => Type::new(Ty::Prim("int")),
        "java.util.OptionalLong" => Type::new(Ty::Prim("long")),
        _ => t.clone(),
    }
}

const WRAPPERS: &[&str] = &[
    "jakarta.xml.bind.JAXBElement",
    "javax.xml.bind.JAXBElement",
    "io.smallrye.mutiny.Uni",
    "java.util.concurrent.CompletionStage",
    "java.util.concurrent.CompletableFuture",
    "io.reactivex.Single",
];

pub fn is_wrapped(t: &Type) -> bool {
    is_optional(t) || WRAPPERS.contains(&t.name().as_str())
}

pub fn unwrap(t: &Type) -> Type {
    if is_optional(t) {
        return optional_type(t);
    }
    if WRAPPERS.contains(&t.name().as_str()) {
        return t.args().first().cloned().unwrap_or_else(|| t.clone());
    }
    t.clone()
}

pub fn is_void(t: &Type) -> bool {
    match t.kind() {
        Kind::Void => true,
        Kind::Class => t.name() == "java.lang.Void",
        _ => false,
    }
}

/// `TypeUtil.isA`.
pub fn is_a(idx: &Index, subject: &Type, object: &str) -> bool {
    let sname = get_name_for_isa(subject);
    if sname == object {
        return true;
    }
    if subject.kind() == Kind::Primitive {
        return false;
    }
    idx.is_a(&sname, object)
}

fn get_name_for_isa(t: &Type) -> String {
    get_name(t)
}

fn primitive_wrapper(p: &str) -> &'static str {
    match p {
        "boolean" => "java.lang.Boolean",
        "byte" => "java.lang.Byte",
        "char" => "java.lang.Character",
        "short" => "java.lang.Short",
        "int" => "java.lang.Integer",
        "long" => "java.lang.Long",
        "float" => "java.lang.Float",
        "double" => "java.lang.Double",
        _ => "",
    }
}

pub fn equal_types(a: &Type, b: &Type) -> bool {
    if a.name() == b.name() {
        return true;
    }
    let wrapped = |p: &Type, w: &Type| match (&p.ty, &w.ty) {
        (Ty::Prim(pn), Ty::Class(wn)) => primitive_wrapper(pn) == wn,
        _ => false,
    };
    wrapped(a, b) || wrapped(b, a)
}

/// `TypeUtil.applyTypeAttributes`.
pub fn apply_type_attributes(t: &Type, schema: &M, schema_anno: Option<&Anno>) {
    let f = type_format(t);
    model::set_type(schema, f.ty);
    model::set(schema, "format", f.format.map(|s| V::Str(s.into())));
    model::set(schema, "pattern", f.pattern.map(|s| V::Str(s.into())));
    if schema_anno.is_none_or(|a| a.value("example").is_none()) {
        model::set(
            schema,
            "examples",
            f.example.map(|e| V::List(vec![V::Str(e.into())])),
        );
    }
    model::set(
        schema,
        "externalDocs",
        f.ext_docs.map(|(d, u)| V::Obj(ext_docs(d, u))),
    );
}

pub fn ext_docs(description: &str, url: &str) -> M {
    let d = model::new(model::MK::ExternalDocs);
    model::set_v(&d, "description", V::Str(description.into()));
    model::set_v(&d, "url", V::Str(url.into()));
    d
}

/// `TypeUtil.getTypeAttributes` as `readSchema` defaults.
pub fn type_defaults(t: &Type) -> Defaults {
    let f = type_format(t);
    Defaults {
        ty: f.ty.map(|s| s.to_string()),
        format: f.format.map(|s| s.to_string()),
        pattern: f.pattern.map(|s| s.to_string()),
        examples: f.example.map(|e| vec![e.to_string()]),
        enumeration: None,
    }
}

#[derive(Default, Clone)]
pub struct Defaults {
    pub ty: Option<String>,
    pub format: Option<String>,
    pub pattern: Option<String>,
    pub examples: Option<Vec<String>>,
    pub enumeration: Option<Vec<V>>,
}

pub fn clear_matching_defaults(field: &M, ty: &M) {
    for key in ["format", "pattern", "examples", "example", "externalDocs"] {
        if model::get(field, key) == model::get(ty, key) {
            model::set(field, key, None);
        }
    }
}

/// `TypeUtil.isTypeOverridden`.
pub fn is_type_overridden(t: &Type, schema_anno: Option<&Anno>) -> bool {
    let Some(a) = schema_anno else { return false };
    match a.enum_value("type") {
        Some(provided) => type_format(t).ty != Some(schema_type_name(&provided)),
        None => false,
    }
}

pub fn schema_type_name(enum_value: &str) -> &'static str {
    match enum_value {
        "INTEGER" => "integer",
        "NUMBER" => "number",
        "BOOLEAN" => "boolean",
        "STRING" => "string",
        "OBJECT" => "object",
        "ARRAY" => "array",
        "NULL" => "null",
        _ => "string",
    }
}

pub fn has_implementation(a: &Anno) -> bool {
    a.value("implementation").is_some()
}

pub fn is_ref_anno(a: Option<&Anno>) -> bool {
    a.is_some_and(|a| a.value("ref").is_some())
}

pub fn is_array_schema(a: &Anno) -> bool {
    has_implementation(a) && a.enum_value("type").as_deref() == Some("ARRAY")
}

pub fn is_boolean_schema(a: &Anno) -> bool {
    match a.class("implementation") {
        Some(t) => {
            let n = t.name();
            n == "org.eclipse.microprofile.openapi.annotations.media.Schema$True"
                || n == "org.eclipse.microprofile.openapi.annotations.media.Schema$False"
        }
        None => false,
    }
}

fn display_values(a: &Anno) -> usize {
    a.values
        .iter()
        .filter(|(k, _)| !matches!(k.as_str(), "implementation" | "name" | "required"))
        .count()
}

pub fn is_simple_class_schema(a: &Anno) -> bool {
    display_values(a) == 0 && has_implementation(a)
}

pub fn is_simple_array_schema(a: &Anno) -> bool {
    display_values(a) == 1 && is_array_schema(a)
}

/// Class of a declaration target (`TypeUtil.getDeclaringClass`).
pub fn declaring_class(t: Target) -> Option<ClassId> {
    match t {
        Target::Field(f) => Some(f.class),
        Target::Method(m) => Some(m.class),
        Target::Param(m, _) => Some(m.class),
        Target::Class(_) => None,
    }
}

/// `TypeUtil.mapDeprecated`.
pub fn map_deprecated(idx: &Index, target: Option<Target>, m: &M) {
    if model::has(m, "deprecated") {
        return;
    }
    if let Some(t) = target
        && idx.declared(t).iter().any(|a| a.name == DEPRECATED)
    {
        model::set_v(m, "deprecated", V::Bool(true));
    }
}
