//! What quarkus-smallrye-openapi adds on top of SmallRye: operation summaries, security requirements with
//! 401/403 responses, 400 responses for bodies, the configured security scheme and the default info.

use std::collections::HashSet;

use crate::index::{ClassId, Index, MethodId, Target};
use crate::model::{self, M, MK, V};
use crate::scan::jaxrs::{METHOD_REF_EXT, MethodRefs};
use crate::scan::params::is_http_method;

pub struct SecurityScheme {
    pub name: String,
    pub description: String,
    pub scheme_type: String,
    pub scheme: String,
    pub bearer_format: Option<String>,
}

const AUTHENTICATED: &str = "io.quarkus.security.Authenticated";
const PERMISSIONS_ALLOWED: &str = "io.quarkus.security.PermissionsAllowed";
const ROLES_ALLOWED: &str = "jakarta.annotation.security.RolesAllowed";
const SECURITY_REQUIREMENT: &str =
    "org.eclipse.microprofile.openapi.annotations.security.SecurityRequirement";

fn find_http_method(idx: &Index, m: MethodId) -> Option<MethodId> {
    if idx
        .method(m)
        .annotations
        .iter()
        .any(|a| is_http_method(&a.name))
    {
        return Some(m);
    }
    let mi = idx.method(m);
    let mut current = idx.classes[m.class]
        .super_type
        .as_ref()
        .and_then(|s| idx.id(&s.name()));
    while let Some(c) = current {
        for (i, other) in idx.classes[c].methods.iter().enumerate() {
            if other.name == mi.name
                && other.params.len() == mi.params.len()
                && other.annotations.iter().any(|a| is_http_method(&a.name))
            {
                return Some(MethodId { class: c, idx: i });
            }
        }
        current = idx.classes[c]
            .super_type
            .as_ref()
            .and_then(|s| idx.id(&s.name()));
    }
    for it in idx.interfaces(m.class) {
        if let Some(ic) = idx.id(&it.name()) {
            for (i, other) in idx.classes[ic].methods.iter().enumerate() {
                if other.name == mi.name
                    && other.params.len() == mi.params.len()
                    && other.annotations.iter().any(|a| is_http_method(&a.name))
                {
                    return Some(MethodId { class: ic, idx: i });
                }
            }
        }
    }
    None
}

fn has_security_requirement(idx: &Index, m: MethodId) -> bool {
    idx.method(m)
        .annotations
        .iter()
        .any(|a| a.name == SECURITY_REQUIREMENT)
        || idx.classes[m.class]
            .annotations
            .iter()
            .any(|a| a.name == SECURITY_REQUIREMENT)
}

/// Method references (resource class, method) with a security annotation, as Quarkus' `getMethods`.
fn secured_methods(idx: &Index, annotation: &str) -> HashSet<(ClassId, MethodId)> {
    let mut out = HashSet::new();
    for (target, _) in idx.annotations(annotation) {
        match target {
            Target::Method(m) => {
                if let Some(http) = find_http_method(idx, m)
                    && !has_security_requirement(idx, m)
                {
                    out.insert((m.class, http));
                }
            }
            Target::Class(c) => {
                for m in class_methods(idx, c) {
                    if idx
                        .method(m)
                        .annotations
                        .iter()
                        .any(|a| a.name == annotation)
                    {
                        continue;
                    }
                    if find_http_method(idx, m).is_some() && !has_security_requirement(idx, m) {
                        let resource = if idx.classes[m.class].flags.interface {
                            m.class
                        } else {
                            c
                        };
                        out.insert((resource, m));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Quarkus' `getMethods(ClassInfo)`: own methods, methods of direct interfaces, and of superclasses.
fn class_methods(idx: &Index, c: ClassId) -> Vec<MethodId> {
    let ci = &idx.classes[c];
    let mut out: Vec<MethodId> = (0..ci.methods.len())
        .map(|i| MethodId { class: c, idx: i })
        .collect();
    for it in &ci.interfaces {
        if let Some(ic) = idx.id(&it.name()) {
            out.extend((0..idx.classes[ic].methods.len()).map(|i| MethodId { class: ic, idx: i }));
        }
    }
    if let Some(sc) = ci.super_type.as_ref().and_then(|s| idx.id(&s.name())) {
        out.extend(class_methods(idx, sc));
    }
    out
}

/// Whether Quarkus' class-name map contains the reference created for (resource class, method).
fn in_class_name_map(idx: &Index, class: ClassId, m: MethodId) -> bool {
    if !idx
        .method(m)
        .annotations
        .iter()
        .any(|a| is_http_method(&a.name))
    {
        return false;
    }
    if m.class == class {
        return !idx.classes[class].flags.interface && !idx.classes[class].flags.abstract_;
    }
    let declaring = &idx.classes[m.class];
    (declaring.flags.interface || declaring.flags.abstract_)
        && idx.is_a(&idx.classes[class].name, &declaring.name)
}

fn split_camel_case(s: &str) -> String {
    // Quarkus splits with `(?<=[A-Z])(?=[A-Z][a-z])|(?<=[^A-Z])(?=[A-Z])|(?<=[A-Za-z])(?=[^A-Za-z])`;
    // the `regex` crate has no look-around, so the three cases are checked per character
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    for i in 0..chars.len() {
        if i > 0 {
            let prev = chars[i - 1];
            let cur = chars[i];
            let next = chars.get(i + 1).copied();
            let a = prev.is_ascii_uppercase()
                && cur.is_ascii_uppercase()
                && next.is_some_and(|n| n.is_ascii_lowercase());
            let b = !prev.is_ascii_uppercase() && cur.is_ascii_uppercase();
            let c = prev.is_ascii_alphabetic() && !cur.is_ascii_alphabetic();
            if a || b || c {
                out.push(' ');
            }
        }
        out.push(chars[i]);
    }
    out
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn operations(openapi: &M) -> Vec<(String, M)> {
    let Some(paths) = model::get_obj(openapi, "paths") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (_, item) in model::entries(&paths) {
        for method in [
            "get", "put", "post", "delete", "options", "head", "patch", "trace",
        ] {
            if let Some(op) = model::get_obj(&item, method) {
                out.push((method.to_string(), op));
            }
        }
    }
    out
}

fn resolve_schema(openapi: &M, mut s: Option<M>) -> Option<M> {
    while let Some(cur) = s.clone() {
        if let Some(r) = model::get_ref(&cur) {
            let name = r.replace(model::SCHEMA_REF_PREFIX, "");
            s = model::get_obj(openapi, "components")
                .and_then(|c| model::get_map(&c, "schemas"))
                .and_then(|m| match m.get(&name) {
                    Some(V::Obj(o)) => Some(o.clone()),
                    _ => None,
                });
            if s.is_none() {
                break;
            }
        } else if let Some(cs) = model::get_obj(&cur, "contentSchema") {
            s = Some(cs);
            continue;
        } else {
            break;
        }
    }
    s
}

fn has_type(s: &M, t: &str) -> bool {
    model::types(s).is_some_and(|l| l.iter().any(|x| x == t))
}

fn is_file_schema(openapi: &M, s: &M) -> bool {
    if has_type(s, "string") && model::get_str(s, "format").as_deref() == Some("binary") {
        return true;
    }
    if has_type(s, "object")
        && let Some(props) = model::get_map(s, "properties")
    {
        return props.values().any(|p| match p {
            V::Obj(p) => {
                has_type(p, "string") && model::get_str(p, "format").as_deref() == Some("binary")
            }
            _ => false,
        });
    }
    if let Some(r) = model::get_ref(s) {
        let name = r.replace(model::SCHEMA_REF_PREFIX, "");
        let referenced = model::get_obj(openapi, "components")
            .and_then(|c| model::get_map(&c, "schemas"))
            .and_then(|m| match m.get(&name) {
                Some(V::Obj(o)) => Some(o.clone()),
                _ => None,
            });
        if let Some(r) = referenced {
            return is_file_schema(openapi, &r);
        }
    }
    false
}

/// `OperationFilter.filterOpenAPI`.
pub fn operation_filter(idx: &Index, openapi: &M, refs: &MethodRefs, default_scheme: &str) {
    let mut authorized = secured_methods(idx, PERMISSIONS_ALLOWED);
    authorized.extend(secured_methods(idx, ROLES_ALLOWED));
    let authenticated = secured_methods(idx, AUTHENTICATED);
    let scheme_name = model::get_obj(openapi, "components")
        .and_then(|c| model::get_map(&c, "securitySchemes"))
        .and_then(|m| m.keys().next().cloned())
        .unwrap_or_else(|| default_scheme.to_string());
    for (http, op) in operations(openapi) {
        let Some(V::Str(r)) = model::get(&op, METHOD_REF_EXT) else {
            continue;
        };
        let (class, m) = refs[r.parse::<usize>().unwrap()];
        let known = in_class_name_map(idx, class, m);
        if known && !model::has(&op, "summary") {
            model::set_v(
                &op,
                "summary",
                V::Str(capitalize(&split_camel_case(&idx.method(m).name))),
            );
        }
        if authorized.contains(&(class, m)) || authenticated.contains(&(class, m)) {
            let req = model::new(MK::SecurityRequirement);
            model::set_v(&req, &scheme_name, V::List(Vec::new()));
            model::add_list(&op, "security", V::Obj(req));
            let responses = model::get_obj(&op, "responses").unwrap();
            if model::entry(&responses, "401").is_none() {
                let r = model::new(MK::APIResponse);
                model::set_v(&r, "description", V::Str("Not Authorized".into()));
                model::put_entry(&responses, "401", r);
            }
            if model::entry(&responses, "403").is_none() {
                let r = model::new(MK::APIResponse);
                model::set_v(&r, "description", V::Str("Not Allowed".into()));
                model::put_entry(&responses, "403", r);
            }
        }
        if known
            && (http == "post" || http == "put")
            && let Some(body) = model::get_obj(&op, "requestBody")
        {
            let mut primitive = false;
            let mut file = false;
            if let Some(content) = model::get_obj(&body, "content") {
                for (_, mt) in model::entries(&content) {
                    if let Some(s) = model::get_obj(&mt, "schema") {
                        let resolved = if model::get_ref(&s).is_some()
                            || model::get_obj(&s, "contentSchema")
                                .is_some_and(|c| model::get_ref(&c).is_some())
                        {
                            resolve_schema(openapi, Some(s.clone()))
                        } else {
                            Some(s.clone())
                        };
                        if let Some(r) = resolved
                            && (has_type(&r, "string")
                                || has_type(&r, "integer")
                                || has_type(&r, "number")
                                || has_type(&r, "boolean"))
                        {
                            primitive = true;
                        }
                    }
                }
                for (name, mt) in model::entries(&content) {
                    if (name == "multipart/form-data" || name == "application/octet-stream")
                        && let Some(s) = model::get_obj(&mt, "schema")
                        && is_file_schema(openapi, &s)
                    {
                        file = true;
                    }
                }
            }
            let responses = model::get_obj(&op, "responses").unwrap();
            if !primitive && !file && model::entry(&responses, "400").is_none() {
                let r = model::new(MK::APIResponse);
                model::set_v(&r, "description", V::Str("Bad Request".into()));
                model::put_entry(&responses, "400", r);
            }
        }
        model::set(&op, METHOD_REF_EXT, None);
    }
}

/// `SecurityConfigFilter`.
pub fn security_config_filter(openapi: &M, s: &SecurityScheme) {
    let components = match model::get_obj(openapi, "components") {
        Some(c) => c,
        None => {
            let c = model::new(MK::Components);
            model::set_v(openapi, "components", V::Obj(c.clone()));
            c
        }
    };
    let scheme = model::new(MK::SecurityScheme);
    model::set_v(&scheme, "description", V::Str(s.description.clone()));
    model::set_v(&scheme, "type", V::Str(s.scheme_type.clone()));
    model::set_v(&scheme, "scheme", V::Str(s.scheme.clone()));
    if let Some(b) = &s.bearer_format {
        model::set_v(&scheme, "bearerFormat", V::Str(b.clone()));
    }
    let mut schemes = model::get_map(&components, "securitySchemes").unwrap_or_default();
    schemes.insert(s.name.clone(), V::Obj(scheme));
    model::set_v(&components, "securitySchemes", V::Map(schemes));
}

/// `DefaultInfoFilter`.
pub fn default_info_filter(openapi: &M, title: &str, version: &str) {
    let info = match model::get_obj(openapi, "info") {
        Some(i) => i,
        None => {
            let i = model::new(MK::Info);
            model::set_v(openapi, "info", V::Obj(i.clone()));
            i
        }
    };
    if !model::has(&info, "title") {
        model::set_v(&info, "title", V::Str(title.into()));
    }
    if !model::has(&info, "version") {
        model::set_v(&info, "version", V::Str(version.into()));
    }
}
