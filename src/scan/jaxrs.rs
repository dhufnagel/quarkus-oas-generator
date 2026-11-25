//! Port of `JaxRsAnnotationScanner` and the `AnnotationScanner` defaults it uses.

use super::beanval::{self, Req};
use super::params::{self, ParamProc, ResourceParameters};
use super::schema;
use super::util::{self, AT, SCHEMA};
use super::{Ctx, props};
use crate::index::{Anno, ClassId, Kind, MethodId, Target, Type};
use crate::model::{self, M, MK, V};

const TAG: &str = "org.eclipse.microprofile.openapi.annotations.tags.Tag";
const TAGS: &str = "org.eclipse.microprofile.openapi.annotations.tags.Tags";
const OPERATION: &str = "org.eclipse.microprofile.openapi.annotations.Operation";
const API_RESPONSE: &str = "org.eclipse.microprofile.openapi.annotations.responses.APIResponse";
const API_RESPONSES: &str = "org.eclipse.microprofile.openapi.annotations.responses.APIResponses";
const REQUEST_BODY: &str = "org.eclipse.microprofile.openapi.annotations.parameters.RequestBody";
const EXTENSION: &str = "org.eclipse.microprofile.openapi.annotations.extensions.Extension";
const EXTENSIONS: &str = "org.eclipse.microprofile.openapi.annotations.extensions.Extensions";
const PRODUCES: &str = "jakarta.ws.rs.Produces";
const CONSUMES: &str = "jakarta.ws.rs.Consumes";
pub const METHOD_REF_EXT: &str = "x-quarkus-openapi-method-ref";

/// HTTP methods in `PathItem.HttpMethod` order (the order of `JaxRsConstants.HTTP_METHOD_ANNOTATIONS`).
const HTTP_METHODS: &[(&str, &str)] = &[
    ("DELETE", "delete"),
    ("GET", "get"),
    ("HEAD", "head"),
    ("OPTIONS", "options"),
    ("PATCH", "patch"),
    ("POST", "post"),
    ("PUT", "put"),
];

/// Callback for Quarkus' `OperationHandler`: records which class/method produced an operation.
pub type MethodRefs = Vec<(ClassId, MethodId)>;

pub fn scan(ctx: &mut Ctx, refs: &mut MethodRefs) {
    ctx.jaxrs_active = true;
    let idx = ctx.idx;
    let mut resources: Vec<ClassId> = (0..idx.classes.len())
        .filter(|c| {
            let ci = &idx.classes[*c];
            params::path_annotation(&ci.annotations).is_some()
                && has_implementation_or_included(ctx, *c)
        })
        .collect();
    resources.sort_by(|a, b| idx.classes[*a].name.cmp(&idx.classes[*b].name));
    for class in resources {
        let stack = props::for_class(idx, class, None);
        ctx.resolver_stack.push(stack);
        let tags = resource_class_tags(ctx, class);
        process_resource_methods(ctx, class, &tags, refs);
        ctx.resolver_stack.pop();
    }
}

fn has_implementation_or_included(ctx: &Ctx, c: ClassId) -> bool {
    let ci = &ctx.idx.classes[c];
    if !ci.flags.abstract_ {
        return true;
    }
    ctx.idx
        .all_known_implementors(&ci.name)
        .iter()
        .any(|i| !ctx.idx.classes[*i].flags.abstract_)
}

fn read_tags(ctx: &mut Ctx, annos: &[Anno]) -> Option<Vec<String>> {
    let mut tag_annos: Vec<Anno> = annos.iter().filter(|a| a.name == TAG).cloned().collect();
    if let Some(container) = annos.iter().find(|a| a.name == TAGS) {
        tag_annos.extend(container.nested_array("value"));
    }
    if tag_annos.is_empty() {
        return None;
    }
    let mut names: Vec<String> = Vec::new();
    for a in tag_annos {
        if a.value("ref").is_some() && a.str("name").is_none() {
            ctx.unsupported("@Tag(ref) is not supported".into());
            continue;
        }
        let Some(name) = a.str("name") else { continue };
        if !names.contains(&name) {
            names.push(name.clone());
        }
        let tag = model::new(MK::Tag);
        model::set_v(&tag, "name", V::Str(name.clone()));
        model::set(&tag, "description", a.str("description").map(V::Str));
        add_tag(ctx, tag);
    }
    Some(names)
}

fn add_tag(ctx: &Ctx, tag: M) {
    let name = model::get_str(&tag, "name");
    let tags = model::get_obj_list(&ctx.openapi, "tags").unwrap_or_default();
    match tags.iter().find(|t| model::get_str(t, "name") == name) {
        Some(existing) => {
            model::merge(Some(existing.clone()), Some(tag));
        }
        None => model::add_list(&ctx.openapi, "tags", V::Obj(tag)),
    }
}

fn resource_class_tags(ctx: &mut Ctx, class: ClassId) -> Vec<String> {
    let mut c = Some(class);
    while let Some(cid) = c {
        let annos = ctx.idx.classes[cid].annotations.clone();
        if let Some(tags) = read_tags(ctx, &annos) {
            return tags;
        }
        c = ctx.idx.classes[cid]
            .super_type
            .as_ref()
            .and_then(|s| ctx.idx.id(&s.name()));
    }
    Vec::new()
}

/// `getResourceMethods`: methods of the class chain and its interfaces, minus hidden ones; then reversed.
fn resource_methods(ctx: &Ctx, class: ClassId) -> Vec<MethodId> {
    let idx = ctx.idx;
    let mut methods = Vec::new();
    for (c, _) in idx.inheritance_chain(class, &Type::class(&idx.classes[class].name)) {
        methods.extend(idx.methods_sorted(c));
        for it in idx.interfaces(c) {
            if util::known_java_type(&it.name()) {
                continue;
            }
            if let Some(ic) = idx.id(&it.name()) {
                methods.extend(idx.methods_sorted(ic));
            }
        }
    }
    let hidden: Vec<MethodId> = methods
        .iter()
        .copied()
        .filter(|m| is_hidden(ctx, *m))
        .collect();
    let mut hidden_all = hidden.clone();
    for h in &hidden {
        hidden_all.extend(params::ancestry(ctx, *h));
    }
    methods.retain(|m| !hidden_all.contains(m));
    methods.reverse();
    methods
}

fn is_hidden(ctx: &Ctx, m: MethodId) -> bool {
    ctx.idx
        .method(m)
        .annotations
        .iter()
        .any(|a| a.name == OPERATION && a.bool("hidden") == Some(true))
}

fn process_resource_methods(ctx: &mut Ctx, class: ClassId, tags: &[String], refs: &mut MethodRefs) {
    for m in resource_methods(ctx, class) {
        let annos = ctx.idx.method(m).annotations.clone();
        let mut count = 0;
        for (http, key) in HTTP_METHODS {
            if annos.iter().any(|a| {
                a.name == format!("jakarta.ws.rs.{http}") || a.name == format!("javax.ws.rs.{http}")
            }) {
                count += 1;
                process_resource_method(ctx, class, m, key, tags, refs);
            }
        }
        if count == 0 && params::path_annotation(&annos).is_some() {
            ctx.unsupported(format!(
                "sub-resource locators are not supported ({})",
                ctx.idx.method(m).name
            ));
        }
    }
}

fn media_types(
    ctx: &Ctx,
    m: MethodId,
    annotation: &str,
    default: &[String],
) -> Option<Vec<String>> {
    let idx = ctx.idx;
    let javax = annotation.replace("jakarta.", "javax.");
    for a in params::ancestry(ctx, m) {
        let mi = idx.method(a);
        let anno = mi
            .annotations
            .iter()
            .find(|x| x.name == annotation || x.name == javax)
            .or_else(|| {
                idx.classes[a.class]
                    .annotations
                    .iter()
                    .find(|x| x.name == annotation || x.name == javax)
            });
        if let Some(anno) = anno {
            return Some(match anno.value("value") {
                Some(v) => v
                    .as_string_array()
                    .iter()
                    .flat_map(|s| s.split(','))
                    .map(|s| s.trim().to_string())
                    .collect(),
                None => default.to_vec(),
            });
        }
    }
    None
}

const PRIMITIVE_OBJECTS: &[&str] = &[
    "java.lang.String",
    "java.lang.Integer",
    "java.lang.Short",
    "java.lang.Long",
    "java.lang.Float",
    "java.lang.Double",
    "java.lang.Boolean",
    "java.lang.Character",
    "java.math.BigDecimal",
    "java.math.BigInteger",
];
const STREAM_OBJECTS: &[&str] = &[
    "java.io.File",
    "java.nio.file.Path",
    "java.io.InputStream",
    "java.io.Reader",
    "java.lang.Byte",
    "[B",
    "io.vertx.core.file.AsyncFile",
    "io.vertx.core.buffer.Buffer",
];

fn is_primitive(t: &Type) -> bool {
    t.kind() == Kind::Primitive
        || PRIMITIVE_OBJECTS.contains(&t.name().as_str())
        || (is_rest_response(t) && is_primitive(&t.args()[0]))
        || (util::is_wrapped(t) && is_primitive(&util::unwrap(t)))
}

fn is_streaming(t: &Type) -> bool {
    (t.kind() == Kind::Primitive && t.name() == "byte")
        || STREAM_OBJECTS.contains(&t.name().as_str())
        || (is_rest_response(t) && is_streaming(&t.args()[0]))
        || (util::is_wrapped(t) && is_streaming(&util::unwrap(t)))
}

fn is_rest_response(t: &Type) -> bool {
    t.name() == "org.jboss.resteasy.reactive.RestResponse" && t.kind() == Kind::Parameterized
}

fn default_produces(ctx: &Ctx, m: MethodId) -> Vec<String> {
    let rt = &ctx.idx.method(m).return_type;
    let any = || vec!["*/*".to_string()];
    if is_streaming(rt) {
        ctx.cfg
            .default_streaming_produces
            .clone()
            .unwrap_or_else(any)
    } else if is_primitive(rt) {
        ctx.cfg
            .default_primitives_produces
            .clone()
            .unwrap_or_else(any)
    } else {
        ctx.cfg.default_produces.clone().unwrap_or_else(any)
    }
}

fn default_consumes(ctx: &Ctx, m: MethodId, rp: &ResourceParameters) -> Vec<String> {
    let any = || vec!["*/*".to_string()];
    match request_body_param_type(ctx, m, rp) {
        Some(t) if is_streaming(&t) => ctx
            .cfg
            .default_streaming_consumes
            .clone()
            .unwrap_or_else(any),
        Some(t) if is_primitive(&t) => ctx
            .cfg
            .default_primitives_consumes
            .clone()
            .unwrap_or_else(any),
        Some(_) => ctx.cfg.default_consumes.clone().unwrap_or_else(any),
        None => Vec::new(),
    }
}

const CONTEXTS: &[&str] = &[
    "Application",
    "UriInfo",
    "Request",
    "HttpHeaders",
    "SecurityContext",
];

fn is_framework_context(t: &Type) -> bool {
    let n = t.name();
    CONTEXTS
        .iter()
        .any(|c| n == format!("jakarta.ws.rs.core.{c}") || n == format!("javax.ws.rs.core.{c}"))
}

/// `getRequestBodyParameterClassType`.
fn request_body_param_position(ctx: &Ctx, m: MethodId, rp: &ResourceParameters) -> Option<usize> {
    let mi = ctx.idx.method(m);
    mi.params.iter().enumerate().position(|(_, p)| {
        if p.ty.name() == "kotlin.coroutines.Continuation" || is_framework_context(&p.ty) {
            return false;
        }
        // naked path parameters
        let is_path = rp.all().iter().any(|x| {
            model::get_str(x, "name").as_deref() == Some(&p.name)
                && model::get_str(x, "in").as_deref() == Some("path")
        });
        if is_path {
            return false;
        }
        p.annotations.is_empty() || !params::contains_jaxrs_annotations(&p.annotations)
    })
}

fn request_body_param_type(ctx: &Ctx, m: MethodId, rp: &ResourceParameters) -> Option<Type> {
    request_body_param_position(ctx, m, rp).map(|i| ctx.idx.method(m).params[i].ty.clone())
}

fn read_operation(ctx: &mut Ctx, m: MethodId) -> M {
    let op = model::new(MK::Operation);
    if let Some(a) = ctx
        .idx
        .method(m)
        .annotations
        .iter()
        .find(|a| a.name == OPERATION)
        .cloned()
    {
        model::set(&op, "summary", a.str("summary").map(V::Str));
        model::set(&op, "description", a.str("description").map(V::Str));
        model::set(&op, "operationId", a.str("operationId").map(V::Str));
        if a.bool("deprecated") == Some(true) {
            model::set_v(&op, "deprecated", V::Bool(true));
        }
    }
    op
}

fn read_extensions(annos: &[Anno]) -> Vec<(String, V)> {
    let mut list: Vec<Anno> = annos
        .iter()
        .filter(|a| a.name == EXTENSION)
        .cloned()
        .collect();
    if let Some(c) = annos.iter().find(|a| a.name == EXTENSIONS) {
        list.extend(c.nested_array("value"));
    }
    list.iter()
        .filter_map(|a| {
            let name = a.str("name")?;
            let name = if name.starts_with("x-") {
                name
            } else {
                format!("x-{name}")
            };
            Some((name, V::Str(a.str("value").unwrap_or_default())))
        })
        .collect()
}

fn process_resource_method(
    ctx: &mut Ctx,
    class: ClassId,
    m: MethodId,
    http: &str,
    tags: &[String],
    refs: &mut MethodRefs,
) {
    let idx = ctx.idx;
    let mut pp = ParamProc::new(class, m);
    let mut rp = pp.process(ctx);
    let dc = default_consumes(ctx, m, &rp);
    ctx.default_consumes = Some(dc.clone());
    ctx.current_consumes = media_types(ctx, m, CONSUMES, &dc);
    let dp = default_produces(ctx, m);
    ctx.default_produces = Some(dp.clone());
    ctx.current_produces = media_types(ctx, m, PRODUCES, &dp);
    rp.form_body_content = pp.form_body_content(ctx);

    if is_hidden(ctx, m) {
        return;
    }
    let op = read_operation(ctx, m);
    refs.push((class, m));
    model::set_private_ext(
        &op,
        "method-ref",
        Some(V::Str(format!("{class}:{}", m.idx))),
    );
    util::map_deprecated(idx, Some(Target::Method(m)), &op);
    util::map_deprecated(idx, Some(Target::Class(class)), &op);
    if ctx.cfg.operation_id_strategy.as_deref() == Some("METHOD") && !model::has(&op, "operationId")
    {
        model::set_v(&op, "operationId", V::Str(idx.method(m).name.clone()));
    }
    model::set_v(&op, METHOD_REF_EXT, V::Str(format!("{}", refs.len() - 1)));
    if let Some(id) = model::get_str(&op, "operationId") {
        save_operation_id(ctx, &id, class, m);
    }

    // tags
    let method_annos = idx.method(m).annotations.clone();
    match read_tags(ctx, &method_annos) {
        None => {
            if !tags.is_empty() {
                model::set_v(
                    &op,
                    "tags",
                    V::List(tags.iter().map(|t| V::Str(t.clone())).collect()),
                );
            }
        }
        Some(t) if !t.is_empty() => {
            model::set_v(&op, "tags", V::List(t.into_iter().map(V::Str).collect()))
        }
        _ => {}
    }
    // extensions
    let mut exts = read_extensions(&method_annos);
    if exts.is_empty() {
        exts = read_extensions(&idx.classes[m.class].annotations);
    }
    for (k, v) in &exts {
        model::set_v(&op, k, v.clone());
    }
    let profiles: Vec<String> = exts
        .iter()
        .filter_map(|(k, _)| k.strip_prefix("x-smallrye-profile-").map(|s| s.to_string()))
        .collect();
    if !ctx.cfg.scan_exclude_profiles.is_empty() {
        if ctx
            .cfg
            .scan_exclude_profiles
            .iter()
            .any(|p| profiles.contains(p))
        {
            return;
        }
    } else if !ctx.cfg.scan_profiles.is_empty()
        && !ctx.cfg.scan_profiles.iter().any(|p| profiles.contains(p))
    {
        return;
    }

    model::set(
        &op,
        "parameters",
        rp.operation_params
            .clone()
            .map(|l| V::List(l.into_iter().map(V::Obj).collect())),
    );
    let path_item = model::new(MK::PathItem);
    model::set(
        &path_item,
        "parameters",
        rp.path_item_params
            .clone()
            .map(|l| V::List(l.into_iter().map(V::Obj).collect())),
    );
    if let Some(rb) = process_request_body(ctx, m, &rp) {
        model::set_v(&op, "requestBody", V::Obj(rb));
    }
    process_response(ctx, class, m, &op);
    model::set_v(&path_item, http, V::Obj(op));

    let paths = rp.full_operation_paths();
    let path = create_path(&["", "", paths.first().map(|s| s.as_str()).unwrap_or("")]);
    let paths_model = match model::get_obj(&ctx.openapi, "paths") {
        Some(p) => p,
        None => {
            let p = model::new(MK::Paths);
            model::set_v(&ctx.openapi, "paths", V::Obj(p.clone()));
            p
        }
    };
    match model::entry(&paths_model, &path) {
        None => model::put_entry(&paths_model, &path, path_item),
        Some(existing) => {
            model::merge(Some(existing), Some(path_item));
        }
    }
}

fn save_operation_id(ctx: &mut Ctx, id: &str, class: ClassId, m: MethodId) {
    match ctx.operation_ids.get(id).copied() {
        None => {
            ctx.operation_ids.insert(id.to_string(), m);
        }
        Some(conflict) => {
            if params::ancestry(ctx, m).contains(&conflict) {
                ctx.operation_ids.insert(id.to_string(), m);
            } else {
                eprintln!(
                    "warning: Duplicate operationId: {id} produced by Class: {}, Method: {} and Class: {}, Method: {}",
                    ctx.idx.classes[class].name,
                    ctx.idx.method(m).name,
                    ctx.idx.classes[conflict.class].name,
                    ctx.idx.method(conflict).name
                );
            }
        }
    }
}

fn create_path(segments: &[&str]) -> String {
    let mut b = String::new();
    for s in segments {
        let s = s.strip_prefix('/').unwrap_or(s);
        let s = s.strip_suffix('/').unwrap_or(s);
        if s.is_empty() {
            continue;
        }
        b.push('/');
        b.push_str(s);
    }
    if b.is_empty() { "/".into() } else { b }
}

fn set_request_body_schema(rb: &M, schema: &M, media_types: &[String]) {
    let content = match model::get_obj(rb, "content") {
        Some(c) => c,
        None => {
            let c = model::new(MK::Content);
            model::set_v(rb, "content", V::Obj(c.clone()));
            c
        }
    };
    if model::entries(&content).is_empty() {
        let types: Vec<String> = if media_types.is_empty() {
            vec!["*/*".into()]
        } else {
            media_types.to_vec()
        };
        for t in types {
            let mt = model::new(MK::MediaType);
            model::set_v(&mt, "schema", V::Obj(schema.clone()));
            model::put_entry(&content, &t, mt);
        }
    } else {
        for (_, mt) in model::entries(&content) {
            if !model::has(&mt, "schema") {
                model::set_v(&mt, "schema", V::Obj(schema.clone()));
            }
        }
    }
}

fn get_consumes(ctx: &Ctx) -> Option<Vec<String>> {
    match &ctx.current_consumes {
        Some(c) if !c.is_empty() => Some(c.clone()),
        _ => ctx.cfg.default_consumes.clone(),
    }
}

fn consumes_for_request_body(ctx: &Ctx) -> Option<Vec<String>> {
    match &ctx.current_consumes {
        Some(c) if !c.is_empty() => Some(c.clone()),
        _ => ctx.default_consumes.clone(),
    }
}

pub fn set_rb_required(rb: &M, value: bool) {
    model::set_v(rb, "required", V::Bool(value));
    model::set_private_ext(rb, "is-required-set", Some(V::Bool(true)));
}

fn is_required_set(rb: &M) -> bool {
    model::private_ext(rb, "is-required-set") == Some(V::Bool(true))
}

fn process_request_body(ctx: &mut Ctx, m: MethodId, rp: &ResourceParameters) -> Option<M> {
    let idx = ctx.idx;
    let mi = idx.method(m).clone();
    let mut rb: Option<M> = None;
    // @RequestBody on the method or one of its parameters
    let mut annotations: Vec<(Anno, Option<usize>)> = Vec::new();
    for a in &mi.annotations {
        if a.name == REQUEST_BODY {
            annotations.push((a.clone(), None));
        }
    }
    for (i, p) in mi.params.iter().enumerate() {
        for a in &p.annotations {
            if a.name == REQUEST_BODY {
                annotations.push((a.clone(), Some(i)));
            }
        }
    }
    for (a, pos) in annotations {
        let body = model::new(MK::RequestBody);
        model::set(&body, "description", a.str("description").map(V::Str));
        if a.value("content").is_some() {
            ctx.unsupported("@RequestBody(content) is not supported".into());
        }
        match a.bool("required") {
            Some(r) => set_rb_required(&body, r),
            None => model::set_private_ext(&body, "required-default", Some(V::Bool(true))),
        }
        if let Some(form) = &rp.form_body_content {
            let merged = model::merge(Some(form.clone()), model::get_obj(&body, "content"));
            model::set(&body, "content", merged.map(V::Obj));
        }
        let body_type = match pos {
            Some(i) => Some(mi.params[i].ty.clone()),
            None => request_body_param_type(ctx, m, rp),
        };
        if let Some(bt) = body_type {
            let has_schema = model::get_obj(&body, "content").is_some_and(|c| {
                model::entries(&c)
                    .iter()
                    .any(|(_, mt)| model::has(mt, "schema"))
            });
            let resolved = ctx.resource_resolve(&bt);
            if !has_schema {
                let schema_anno = param_annotation(ctx, m, &resolved, SCHEMA);
                if let Some(s) = schema::type_to_schema(ctx, &resolved, schema_anno.as_ref()) {
                    let consumes = get_consumes(ctx).unwrap_or_default();
                    set_request_body_schema(&body, &s, &consumes);
                }
            }
            if !is_required_set(&body) && util::is_optional(&resolved) {
                set_rb_required(&body, false);
            }
            set_request_body_constraints(ctx, &body, m, &resolved);
        }
        rb = Some(body);
    }
    let rb_has_content = rb
        .as_ref()
        .is_some_and(|b| model::has(b, "content") || model::get_ref(b).is_some());
    if !rb_has_content && get_consumes(ctx).is_some() {
        if let Some(form) = &rp.form_body_content {
            let body = rb.clone().unwrap_or_else(|| {
                let b = model::new(MK::RequestBody);
                model::set_private_ext(&b, "required-default", Some(V::Bool(true)));
                b
            });
            model::set_v(&body, "content", V::Obj(form.clone()));
            rb = Some(body);
        } else if let Some(bt) = request_body_param_type(ctx, m, rp) {
            let resolved = ctx.resource_resolve(&bt);
            let schema = if is_multipart_input(&resolved) {
                let s = model::schema();
                model::add_type(&s, "object");
                Some(s)
            } else {
                let schema_anno = param_annotation(ctx, m, &resolved, SCHEMA);
                schema::type_to_schema(ctx, &resolved, schema_anno.as_ref())
            };
            let body = rb.clone().unwrap_or_else(|| {
                let b = model::new(MK::RequestBody);
                model::set_private_ext(&b, "required-default", Some(V::Bool(true)));
                b
            });
            if let Some(s) = schema {
                let consumes = consumes_for_request_body(ctx).unwrap_or_default();
                set_request_body_schema(&body, &s, &consumes);
            }
            if !is_required_set(&body) && util::is_optional(&resolved) {
                set_rb_required(&body, false);
            }
            set_request_body_constraints(ctx, &body, m, &resolved);
            rb = Some(body);
        }
    }
    rb
}

fn is_multipart_input(t: &Type) -> bool {
    t.name()
        .starts_with("org.jboss.resteasy.plugins.providers.multipart.Multipart")
}

/// `getMethodParameterAnnotation(method, type, name)`: annotation on the first parameter whose type equals `t`.
fn param_annotation(ctx: &Ctx, m: MethodId, t: &Type, name: &str) -> Option<Anno> {
    let pos = ctx.idx.method(m).params.iter().position(|p| &p.ty == t)?;
    util::ann(ctx.idx, &AT::Decl(Target::Param(m, pos)), name)
}

fn set_request_body_constraints(ctx: &Ctx, rb: &M, m: MethodId, t: &Type) {
    let Some(pos) = ctx.idx.method(m).params.iter().position(|p| &p.ty == t) else {
        return;
    };
    if ctx.idx.method(m).params[pos].annotations.is_empty() {
        return;
    }
    let Some(content) = model::get_obj(rb, "content") else {
        return;
    };
    for (_, mt) in model::entries(&content) {
        if let Some(s) = model::get_obj(&mt, "schema") {
            beanval::apply(
                ctx.idx,
                &AT::Decl(Target::Param(m, pos)),
                &s,
                None,
                &Req::Flag(rb, "requestBody"),
            );
        }
    }
}

// ---------------------------------------------------------------- responses

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        _ => "Unknown",
    }
}

fn read_api_responses(ctx: &mut Ctx, annos: &[Anno]) -> Vec<(String, M)> {
    let mut list: Vec<Anno> = annos
        .iter()
        .filter(|a| a.name == API_RESPONSE)
        .cloned()
        .collect();
    if let Some(c) = annos.iter().find(|a| a.name == API_RESPONSES) {
        list.extend(c.nested_array("value"));
    }
    let mut out: Vec<(String, M)> = Vec::new();
    for a in list {
        let code = a
            .str("responseCode")
            .unwrap_or_else(|| "default".to_string());
        let r = model::new(MK::APIResponse);
        model::set(&r, "description", a.str("description").map(V::Str));
        if a.value("headers").is_some() || a.value("links").is_some() || a.value("ref").is_some() {
            ctx.unsupported("@APIResponse(headers/links/ref) is not supported".into());
        }
        if a.value("content").is_some() {
            let content = read_content(ctx, &a.nested_array("content"));
            model::set_v(&r, "content", V::Obj(content));
        }
        match out.iter().position(|(c, _)| *c == code) {
            Some(i) => out[i].1 = r,
            None => out.push((code, r)),
        }
    }
    out
}

fn read_content(ctx: &mut Ctx, contents: &[Anno]) -> M {
    let content = model::new(MK::Content);
    for c in contents {
        let mt = model::new(MK::MediaType);
        if c.value("examples").is_some()
            || c.value("example").is_some()
            || c.value("encoding").is_some()
        {
            ctx.unsupported("@Content(examples/encoding) is not supported".into());
        }
        if let Some(sa) = c.nested("schema").cloned() {
            let s = schema::read_schema_simple(ctx, &sa);
            model::set(&mt, "schema", s.map(V::Obj));
        }
        match c.str("mediaType") {
            Some(t) => model::put_entry(&content, &t, mt),
            None => {
                for t in ctx.current_produces.clone().unwrap_or_default() {
                    model::put_entry(&content, &t, mt.clone());
                }
            }
        }
    }
    content
}

fn responses_of(op: &M) -> M {
    match model::get_obj(op, "responses") {
        Some(r) => r,
        None => {
            let r = model::new(MK::APIResponses);
            model::set_v(op, "responses", V::Obj(r.clone()));
            r
        }
    }
}

fn is_void(t: &Type) -> bool {
    if util::is_void(t) {
        return true;
    }
    if is_rest_response(t) {
        return t.args().iter().any(is_void);
    }
    if util::is_wrapped(t) {
        return is_void(&util::unwrap(t));
    }
    false
}

fn process_response(ctx: &mut Ctx, class: ClassId, m: MethodId, op: &M) {
    let idx = ctx.idx;
    for (code, r) in read_api_responses(ctx, &idx.classes[class].annotations.clone()) {
        model::put_entry(&responses_of(op), &code, r);
    }
    let method_responses = read_api_responses(ctx, &idx.method(m).annotations.clone());
    for (code, r) in method_responses {
        model::put_entry(&responses_of(op), &code, r);
    }
    let empty_container = idx
        .method(m)
        .annotations
        .iter()
        .any(|a| a.name == API_RESPONSES && a.nested_array("value").is_empty());
    if !empty_container {
        create_response_from_method(ctx, m, op);
    }
}

fn create_response_from_method(ctx: &mut Ctx, m: MethodId, op: &M) {
    let idx = ctx.idx;
    let mi = idx.method(m).clone();
    let return_type = ctx.resource_resolve(&mi.return_type);
    let internal = schema::is_scanner_internal_response(&return_type);
    let void = is_void(&mi.return_type);
    let is_post = mi
        .annotations
        .iter()
        .any(|a| a.name == "jakarta.ws.rs.POST" || a.name == "javax.ws.rs.POST");
    let status = if void {
        if is_post { 201 } else { 204 }
    } else {
        200
    };
    let code = status.to_string();
    if let Some(r) = model::get_obj(op, "responses")
        && model::entry(&r, &code).is_none()
    {
        return;
    }
    let mut response = model::new(MK::APIResponse);
    model::set_v(
        &response,
        "description",
        V::Str(reason_phrase(status).into()),
    );
    let has_content = model::get_obj(op, "responses")
        .and_then(|r| model::entry(&r, &code))
        .is_some_and(|r| model::has(&r, "content"));
    if !void && !has_content {
        let produces = match &ctx.current_produces {
            Some(p) if !p.is_empty() => p.clone(),
            _ => default_produces(ctx, m),
        };
        let content = model::new(MK::Content);
        if internal {
            for p in produces {
                let mt = model::new(MK::MediaType);
                if p == "application/json" {
                    model::set_v(&mt, "schema", V::Obj(model::schema()));
                }
                model::put_entry(&content, &p, mt);
            }
        } else {
            let schema = schema::type_to_schema(ctx, &return_type, None);
            beanval::apply_to_type(idx, &return_type, schema.as_ref());
            if let Some(s) = &schema
                && model::get_nullable(s).is_none()
                && util::is_optional(&return_type)
            {
                make_optional(s);
            }
            for p in produces {
                let mt = model::new(MK::MediaType);
                model::set(&mt, "schema", schema.clone().map(V::Obj));
                model::put_entry(&content, &p, mt);
            }
        }
        model::set_v(&response, "content", V::Obj(content));
    }
    let responses = responses_of(op);
    if let Some(from_annotations) = model::entry(&responses, &code) {
        response = model::merge(Some(response), Some(from_annotations)).unwrap();
    }
    model::put_entry(&responses, &code, response);
}

fn make_optional(s: &M) {
    if model::has(s, "type") {
        model::set_nullable(s, true);
    }
    if let Some(r) = model::get_ref(s) {
        let ref_schema = model::schema_ref(&r);
        model::set_ref(s, None);
        if !model::has(s, "anyOf") {
            model::add_any_of(s, ref_schema);
            model::add_any_of(s, model::null_schema());
        } else {
            let any = model::schema();
            model::add_any_of(&any, ref_schema);
            model::add_any_of(&any, model::null_schema());
            model::add_all_of(s, any);
        }
    }
}
