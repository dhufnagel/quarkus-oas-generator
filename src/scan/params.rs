//! Port of `AbstractParameterProcessor` / `JaxRsParameterProcessor`.

use std::collections::HashSet;

use indexmap::IndexMap;
use std::sync::LazyLock;

use regex::Regex;

static TEMPLATE_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{(\w[\w\.-]*)\}").unwrap());
static TEMPLATE_WITH_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\{[ \t]*(\w[\w\.-]*)[ \t]*:[ \t]*((?:[^{}]|\{[^{}]+\})+)\}").unwrap()
});

/// `parameterInPath`: the path contains `{name}` or `{name: regex}`, blanks allowed around the name.
fn path_has_template(path: &str, name: &str) -> bool {
    path.match_indices('{').any(|(i, _)| {
        let rest = path[i + 1..].trim_start_matches([' ', '\t']);
        rest.strip_prefix(name)
            .map(|after| after.trim_start_matches([' ', '\t']))
            .is_some_and(|after| after.starts_with('}') || after.starts_with(':'))
    })
}

use super::beanval::{self, Req};
use super::schema;
use super::util::{self, AT, Defaults, SCHEMA};
use super::{Ctx, props};
use crate::index::{Anno, ClassId, Kind, MethodId, Target, Type};
use crate::model::{self, M, MK, V};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FrameworkParam {
    pub location: Option<&'static str>,
    pub style: Option<&'static str>,
    pub default_style: Option<&'static str>,
    pub media_type: Option<&'static str>,
    pub bean: bool,
    pub matrix: bool,
}

pub fn framework_param(name: &str) -> Option<FrameworkParam> {
    let simple = name.rsplit('.').next().unwrap_or(name);
    let pkg_ok = name.starts_with("jakarta.ws.rs.")
        || name.starts_with("javax.ws.rs.")
        || name.starts_with("org.jboss.resteasy.annotations.jaxrs.")
        || name.starts_with("org.jboss.resteasy.reactive.")
        || name.starts_with("org.jboss.resteasy.annotations.providers.multipart.");
    if !pkg_ok {
        return None;
    }
    let fp = |location, style, default_style| FrameworkParam {
        location,
        style,
        default_style,
        media_type: None,
        bean: false,
        matrix: false,
    };
    Some(match simple {
        "PathParam" | "RestPath" => fp(Some("path"), None, Some("simple")),
        "MatrixParam" | "RestMatrix" => FrameworkParam {
            matrix: true,
            ..fp(Some("path"), Some("matrix"), Some("matrix"))
        },
        "QueryParam" | "RestQuery" => fp(Some("query"), None, Some("form")),
        "FormParam" | "RestForm" => fp(None, Some("form"), Some("form")),
        "HeaderParam" | "RestHeader" => fp(Some("header"), None, Some("simple")),
        "CookieParam" | "RestCookie" => fp(Some("cookie"), None, Some("form")),
        "BeanParam" => FrameworkParam {
            bean: true,
            ..fp(None, None, None)
        },
        "MultipartForm" => FrameworkParam {
            bean: true,
            media_type: Some("multipart/form-data"),
            ..fp(None, None, None)
        },
        _ => return None,
    })
}

const PARAMETER: &str = "org.eclipse.microprofile.openapi.annotations.parameters.Parameter";
const PARAMETERS: &str = "org.eclipse.microprofile.openapi.annotations.parameters.Parameters";

pub fn is_parameter(name: &str) -> bool {
    framework_param(name).is_some() || name == PARAMETER || name == PARAMETERS
}

pub fn contains_jaxrs_annotations(annos: &[Anno]) -> bool {
    annos.iter().any(|a| {
        framework_param(&a.name).is_some()
            || a.name.starts_with("jakarta.ws.rs")
            || a.name.starts_with("javax.ws.rs")
    })
}

#[derive(Clone)]
struct ParamContext {
    name: Option<String>,
    location: Option<&'static str>,
    style: Option<&'static str>,
    framework: Option<FrameworkParam>,
    target: Option<Target>,
    target_type: Option<Type>,
    default_value: Option<V>,
}

type Key = (Option<String>, Option<&'static str>, Option<&'static str>);

pub struct ResourceParameters {
    pub path_item_paths: Vec<String>,
    pub path_item_params: Option<Vec<M>>,
    pub operation_paths: Vec<String>,
    pub operation_params: Option<Vec<M>>,
    pub form_body_content: Option<M>,
}

impl ResourceParameters {
    pub fn full_operation_paths(&self) -> Vec<String> {
        let mut out = Vec::new();
        for a in &self.path_item_paths {
            for b in &self.operation_paths {
                out.push(format!("{a}{b}"));
            }
        }
        out
    }
    pub fn all(&self) -> Vec<M> {
        let mut all = self.path_item_params.clone().unwrap_or_default();
        all.extend(self.operation_params.clone().unwrap_or_default());
        all
    }
}

pub struct ParamProc {
    pub resource_class: ClassId,
    pub method: MethodId,
    params: IndexMap<usize, ParamContext>,
    keys: Vec<Key>,
    next_id: usize,
    form_params: IndexMap<String, Target>,
    form_media_type: Option<&'static str>,
}

fn strip_slashes(p: &str) -> String {
    let p = p.strip_prefix('/').unwrap_or(p);
    p.strip_suffix('/').unwrap_or(p).to_string()
}

impl ParamProc {
    pub fn new(resource_class: ClassId, method: MethodId) -> ParamProc {
        ParamProc {
            resource_class,
            method,
            params: IndexMap::new(),
            keys: Vec::new(),
            next_id: 0,
            form_params: IndexMap::new(),
            form_media_type: None,
        }
    }

    fn reset(&mut self) {
        self.params.clear();
        self.keys.clear();
        self.form_params.clear();
    }

    pub fn process(&mut self, ctx: &mut Ctx) -> ResourceParameters {
        let mut rp = ResourceParameters {
            path_item_paths: Vec::new(),
            path_item_params: None,
            operation_paths: Vec::new(),
            operation_params: None,
            form_body_content: None,
        };
        // path parameters of the resource class
        let method_class = self.method.class;
        self.read_parameters_inherited(ctx, method_class, false);
        if self.resource_class != method_class {
            self.read_parameters(ctx, self.resource_class, true);
        }
        rp.path_item_params = self.get_parameters(ctx);
        self.reset();
        // operation parameters
        let candidates = ancestry(ctx, self.method);
        for m in &candidates {
            let mi = ctx.idx.method(*m);
            for (pos, p) in mi.params.iter().enumerate() {
                let mut annos: Vec<&Anno> = p.annotations.iter().collect();
                // framework parameters first (stable)
                annos.sort_by_key(|a| !(a.name != PARAMETER && is_parameter(&a.name)));
                for a in annos {
                    self.read_annotated_type(ctx, a, Target::Param(*m, pos), false);
                }
            }
        }
        for m in &candidates {
            for a in ctx.idx.method(*m).annotations.clone() {
                if a.name == PARAMETER || a.name == PARAMETERS {
                    ctx.unsupported(format!(
                        "@Parameter on methods is not supported ({})",
                        ctx.idx.method(*m).name
                    ));
                }
            }
        }
        rp.operation_params = self.get_parameters(ctx);
        self.finalize(ctx, &mut rp);
        rp
    }

    fn read_parameters_inherited(&mut self, ctx: &mut Ctx, class: ClassId, overridden_only: bool) {
        let idx = ctx.idx;
        let mut ancestors: Vec<ClassId> = idx
            .inheritance_chain(class, &Type::class(&idx.classes[class].name))
            .into_iter()
            .map(|(c, _)| c)
            .collect();
        ancestors.reverse();
        for a in ancestors {
            for it in idx.classes[a].interfaces.clone() {
                if let Some(ic) = idx.id(&it.name()) {
                    self.read_parameters(ctx, ic, overridden_only);
                }
            }
            self.read_parameters(ctx, a, overridden_only);
        }
    }

    /// Reads parameter annotations of bean properties (fields, setters, constructor parameters) of `class`.
    fn read_parameters(&mut self, ctx: &mut Ctx, class: ClassId, overridden_only: bool) {
        let idx = ctx.idx;
        let ci = &idx.classes[class];
        let mut found: Vec<(Anno, Target)> = Vec::new();
        for (fi, f) in ci.fields.iter().enumerate() {
            if f.annotations.iter().any(|a| is_parameter(&a.name)) {
                for a in &f.annotations {
                    if is_parameter(&a.name) {
                        found.push((
                            a.clone(),
                            Target::Field(crate::index::FieldId { class, idx: fi }),
                        ));
                    }
                }
            }
        }
        for (mi, m) in ci.methods.iter().enumerate() {
            let mid = MethodId { class, idx: mi };
            if is_resource_method(ctx, mid) || is_sub_resource_locator(ctx, mid) {
                continue;
            }
            for (pi, p) in m.params.iter().enumerate() {
                if p.annotations.iter().any(|a| is_parameter(&a.name)) {
                    for a in &p.annotations {
                        if is_parameter(&a.name) {
                            found.push((a.clone(), Target::Param(mid, pi)));
                        }
                    }
                }
            }
            if m.params.len() == 1 && m.annotations.iter().any(|a| is_parameter(&a.name)) {
                for a in &m.annotations {
                    if is_parameter(&a.name) {
                        found.push((a.clone(), Target::Method(mid)));
                    }
                }
            }
        }
        found.sort_by_key(|(a, _)| !(a.name != PARAMETER && is_parameter(&a.name)));
        for (a, t) in found {
            self.read_annotated_type(ctx, &a, t, overridden_only);
        }
    }

    fn read_annotated_type(
        &mut self,
        ctx: &mut Ctx,
        a: &Anno,
        target: Target,
        overridden_only: bool,
    ) {
        if a.name == PARAMETER {
            ctx.unsupported("@Parameter is not supported".into());
            return;
        }
        let Some(fp) = framework_param(&a.name) else {
            return;
        };
        let target_type = target_type(ctx, target, self.method);
        if fp.style == Some("form") {
            let name = param_name(ctx, a, target);
            self.form_params.insert(name.unwrap_or_default(), target);
            self.read_framework_parameter(ctx, a, fp, target, overridden_only);
        } else if fp.matrix {
            ctx.unsupported("matrix parameters are not supported".into());
        } else if fp.location == Some("path")
            && target_type
                .as_ref()
                .is_some_and(|t| t.name().ends_with(".ws.rs.core.PathSegment"))
        {
            ctx.unsupported("PathSegment parameters are not supported".into());
        } else if fp.location.is_some() {
            self.read_framework_parameter(ctx, a, fp, target, overridden_only);
        } else {
            if let Some(mt) = fp.media_type
                && self.form_media_type.is_none()
            {
                self.form_media_type = Some(mt);
            }
            let tt = target_type.map(|t| util::unwrap(&t));
            if let Some(tt) = tt
                && let Some(bean) = ctx.idx.id(&tt.name())
            {
                let stack = props::for_class(ctx.idx, bean, Some(&tt));
                ctx.resolver_stack.push(stack);
                self.read_parameters_inherited(ctx, bean, overridden_only);
                ctx.resolver_stack.pop();
            }
        }
    }

    fn read_framework_parameter(
        &mut self,
        ctx: &mut Ctx,
        a: &Anno,
        fp: FrameworkParam,
        target: Target,
        overridden_only: bool,
    ) {
        let name = param_name(ctx, a, target);
        let default_value = default_value(ctx, target, self.method);
        let key: Key = (name, fp.location, fp.default_style);
        self.read_parameter(ctx, key, fp, default_value, target, overridden_only);
    }

    fn find_context(&self, key: &Key, target: Target) -> Option<usize> {
        if let Some(pos) = self
            .keys
            .iter()
            .position(|k| k == key && !(k.0.is_none() && k.1.is_none() && k.2.is_none()))
        {
            return Some(*self.params.keys().nth(pos).unwrap());
        }
        let compatible =
            |a: Option<&'static str>, b: Option<&'static str>| a.is_none() || b.is_none() || a == b;
        for (id, c) in &self.params {
            if same_target(c, target, key.0.as_deref())
                && compatible(c.location, key.1)
                && compatible(c.style, key.2)
            {
                return Some(*id);
            }
        }
        for (id, c) in &self.params {
            if (c.location.is_none() || key.1.is_none())
                && key.0.is_some()
                && key.2.is_some()
                && key.0 == c.name
                && key.2 == c.style
            {
                return Some(*id);
            }
        }
        None
    }

    fn read_parameter(
        &mut self,
        ctx: &mut Ctx,
        key: Key,
        fp: FrameworkParam,
        default_value: Option<V>,
        target: Target,
        overridden_only: bool,
    ) {
        let existing = self.find_context(&key, target);
        let (id, add) = match existing {
            Some(id) => (id, false),
            None => {
                if overridden_only {
                    return;
                }
                let id = self.next_id;
                self.next_id += 1;
                self.params.insert(
                    id,
                    ParamContext {
                        name: None,
                        location: None,
                        style: None,
                        framework: None,
                        target: None,
                        target_type: None,
                        default_value: None,
                    },
                );
                (id, true)
            }
        };
        let method = self.method;
        let c = self.params.get_mut(&id).unwrap();
        if c.name.is_none() {
            c.name = key.0.clone();
        }
        if c.location.is_none() {
            c.location = key.1;
        }
        if c.style.is_none() {
            c.style = key.2;
        }
        if c.framework.is_none() {
            c.framework = Some(fp);
            c.default_value = default_value;
        }
        if c.target.is_none() || matches!(c.target, Some(Target::Method(_))) {
            c.target = Some(target);
            let t = target_type(ctx, target, method);
            c.target_type = t.map(|t| resolve_with_stack(ctx, &t));
        }
        let new_key = (c.name.clone(), c.location, c.style);
        if add {
            self.keys.push(new_key);
        }
    }

    fn get_parameters(&mut self, ctx: &mut Ctx) -> Option<Vec<M>> {
        let contexts: Vec<ParamContext> = self.params.values().cloned().collect();
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for c in contexts {
            if let Some(p) = self.map_parameter(ctx, &c) {
                let k = (
                    model::get_str(&p, "name"),
                    model::get_str(&p, "in"),
                    model::get_str(&p, "style"),
                );
                if seen.insert(k) {
                    out.push(p);
                }
            }
        }
        if out.is_empty() { None } else { Some(out) }
    }

    fn map_parameter(&self, ctx: &mut Ctx, c: &ParamContext) -> Option<M> {
        let p = model::new(MK::Parameter);
        model::set(&p, "name", c.name.clone().map(V::Str));
        if !model::has(&p, "in")
            && let Some(l) = c.location
        {
            model::set_v(&p, "in", V::Str(l.into()));
        }
        if c.location == Some("path") {
            model::set_v(&p, "required", V::Bool(true));
        }
        if self.is_ignored(ctx, &p) {
            return None;
        }
        if !model::has(&p, "style")
            && let Some(fp) = c.framework
        {
            model::set(&p, "style", fp.style.map(|s| V::Str(s.into())));
        }
        self.map_parameter_schema(ctx, &p, c);
        util::map_deprecated(ctx.idx, c.target, &p);
        if model::has(&p, "schema") {
            self.augment_param_schema(ctx, &p, c);
        }
        if !model::has(&p, "required") && c.target_type.as_ref().is_some_and(util::is_optional) {
            model::set_v(&p, "required", V::Bool(false));
        }
        if let Some(t) = c.target {
            model::set_private_ext(&p, "param-ref", Some(V::Str(format!("{t:?}"))));
        }
        Some(p)
    }

    fn map_parameter_schema(&self, ctx: &mut Ctx, p: &M, c: &ParamContext) {
        let Some(tt) = &c.target_type else { return };
        let schema_anno = c
            .target
            .and_then(|t| util::ann(ctx.idx, &AT::Decl(t), SCHEMA));
        let schema = match &schema_anno {
            Some(a) if a.bool("hidden") == Some(true) => None,
            Some(a) => {
                let param_type = a.class("implementation").unwrap_or_else(|| tt.clone());
                let defaults = if util::is_array_schema(a) || util::is_type_overridden(tt, Some(a))
                {
                    Defaults::default()
                } else {
                    util::type_defaults(&param_type)
                };
                Some(schema::read_schema(ctx, model::schema(), a, &defaults))
            }
            None => schema::type_to_schema(ctx, tt, None),
        };
        if let Some(s) = schema {
            model::set_v(p, "schema", V::Obj(s));
        }
    }

    fn augment_param_schema(&self, ctx: &mut Ctx, p: &M, c: &ParamContext) {
        let param_schema = model::get_obj(p, "schema").unwrap();
        let local = match model::get_ref(&param_schema) {
            Some(r) => {
                let name = r.rsplit('/').next().unwrap_or(&r).to_string();
                let component = model::get_obj(&ctx.openapi, "components")
                    .and_then(|c| model::get_map(&c, "schemas"))
                    .and_then(|m| m.get(&name).cloned());
                let local = model::schema();
                match component {
                    Some(V::Obj(comp)) => model::set(&local, "type", model::get(&comp, "type")),
                    _ => {
                        let pt = c.target_type.as_ref().map(util::unwrap).unwrap();
                        model::add_type(&local, util::type_format(&pt).ty.unwrap_or("object"));
                    }
                }
                local
            }
            None => param_schema.clone(),
        };
        let original_mod = model::modcount(&local);
        if let Some(t) = c.target {
            let name = model::get_str(p, "name");
            beanval::apply(
                ctx.idx,
                &AT::Decl(t),
                &local,
                name.as_deref(),
                &Req::Flag(p, "parameter"),
            );
        }
        if let Some(d) = &c.default_value
            && !model::has(&local, "default")
        {
            model::set_v(&local, "default", d.clone());
        }
        let modified =
            !std::rc::Rc::ptr_eq(&local, &param_schema) && model::modcount(&local) > original_mod;
        if modified {
            let nullable = model::get_nullable(&local);
            model::set_type_list(&local, None);
            if !model::is_empty_public(&local) {
                let s = model::schema();
                model::add_all_of(&s, param_schema.clone());
                model::add_all_of(&s, local);
                model::set_v(p, "schema", V::Obj(s));
            } else if nullable == Some(true) {
                let s = model::schema();
                model::add_any_of(&s, param_schema.clone());
                model::add_any_of(&s, model::null_schema());
                model::set_v(p, "schema", V::Obj(s));
            }
        }
    }

    fn is_ignored(&self, ctx: &Ctx, p: &M) -> bool {
        let name = model::get_str(p, "name");
        let in_ = model::get_str(p, "in");
        if in_.is_none() {
            return true;
        }
        let Some(name) = name.filter(|n| !n.trim().is_empty()) else {
            return true;
        };
        if in_.as_deref() == Some("path") {
            let paths = full_paths_of_method(ctx, self.method);
            if !paths.iter().any(|p| path_has_template(p, &name)) {
                return true;
            }
        }
        if in_.as_deref() == Some("header")
            && matches!(
                name.to_uppercase().as_str(),
                "ACCEPT" | "AUTHORIZATION" | "CONTENT-TYPE"
            )
        {
            return true;
        }
        false
    }

    fn finalize(&mut self, ctx: &mut Ctx, rp: &mut ResourceParameters) {
        let all = rp.all();
        rp.path_item_paths = generate_paths(&class_paths(ctx, self.resource_class), &all);
        rp.operation_paths = generate_paths(&method_paths_of(ctx, self.method), &all);
        // unannotated path parameters
        let template = &*TEMPLATE_NAME;
        let mut names: Vec<String> = Vec::new();
        for a in &rp.path_item_paths {
            for b in &rp.operation_paths {
                for p in [a, b] {
                    for cap in template.captures_iter(p) {
                        let n = cap[1].to_string();
                        if !names.contains(&n) {
                            names.push(n);
                        }
                    }
                }
            }
        }
        for n in names {
            let exists = all.iter().any(|p| {
                model::get_str(p, "name").as_deref() == Some(&n)
                    && model::get_str(p, "in").as_deref() == Some("path")
            });
            if exists {
                continue;
            }
            if let Some(c) = self.unannotated_path_param(ctx, &n)
                && let Some(p) = self.map_parameter(ctx, &c)
            {
                rp.operation_params.get_or_insert_with(Vec::new).push(p);
            }
        }
        // sortedParametersEnable
        let in_order = |p: &M| match model::get_str(p, "in").as_deref() {
            Some("path") => 0,
            Some("query") => 1,
            Some("header") => 2,
            Some("cookie") => 3,
            _ => 4,
        };
        for list in [&mut rp.path_item_params, &mut rp.operation_params]
            .into_iter()
            .flatten()
        {
            list.sort_by(|a, b| {
                model::get_ref(a)
                    .cmp(&model::get_ref(b))
                    .then(in_order(a).cmp(&in_order(b)))
                    .then_with(
                        || match (model::get_str(a, "name"), model::get_str(b, "name")) {
                            (Some(x), Some(y)) => x.cmp(&y),
                            (Some(_), None) => std::cmp::Ordering::Less,
                            (None, Some(_)) => std::cmp::Ordering::Greater,
                            _ => std::cmp::Ordering::Equal,
                        },
                    )
            });
        }
        rp.form_body_content = self.form_body_content(ctx);
    }

    fn unannotated_path_param(&self, ctx: &Ctx, name: &str) -> Option<ParamContext> {
        let mi = ctx.idx.method(self.method);
        for (i, p) in mi.params.iter().enumerate() {
            if p.name == name && !contains_jaxrs_annotations(&p.annotations) {
                return Some(ParamContext {
                    name: Some(name.to_string()),
                    location: Some("path"),
                    style: Some("simple"),
                    framework: framework_param("org.jboss.resteasy.reactive.RestPath"),
                    target: Some(Target::Param(self.method, i)),
                    target_type: Some(p.ty.clone()),
                    default_value: None,
                });
            }
        }
        None
    }

    pub fn form_body_content(&mut self, ctx: &mut Ctx) -> Option<M> {
        if self.form_params.is_empty() {
            return None;
        }
        let media_types: Vec<String> = match &ctx.current_consumes {
            Some(c) => c.clone(),
            None => match self.form_media_type {
                Some(m) => vec![m.to_string()],
                None => vec!["application/x-www-form-urlencoded".to_string()],
            },
        };
        let content = model::new(MK::Content);
        for mt in media_types {
            let media = model::new(MK::MediaType);
            let schema = model::schema();
            model::add_type(&schema, "object");
            model::set_v(&media, "schema", V::Obj(schema.clone()));
            let mut encodings: IndexMap<String, V> = IndexMap::new();
            if mt == "application/x-www-form-urlencoded" {
                // style/explode of form parameters are never set here
            }
            let form: Vec<(String, Target)> = self
                .form_params
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect();
            for (name, target) in form {
                let Some(pt) = target_type(ctx, target, self.method) else {
                    continue;
                };
                let schema_anno = util::ann(ctx.idx, &AT::Decl(target), SCHEMA);
                let Some(mut ps) = schema::type_to_schema(ctx, &pt, schema_anno.as_ref()) else {
                    continue;
                };
                if let Some(pt_anno) = util::ann(
                    ctx.idx,
                    &AT::Decl(target),
                    "org.jboss.resteasy.reactive.PartType",
                )
                .or_else(|| {
                    util::ann(
                        ctx.idx,
                        &AT::Decl(target),
                        "org.jboss.resteasy.annotations.providers.multipart.PartType",
                    )
                }) {
                    let enc = model::new(MK::Encoding);
                    model::set(&enc, "contentType", pt_anno.str("value").map(V::Str));
                    encodings.insert(name.clone(), V::Obj(enc));
                }
                if let Some(d) = default_value(ctx, target, self.method)
                    && !model::has(&ps, "default")
                {
                    model::set_v(&ps, "default", d);
                }
                util::map_deprecated(ctx.idx, Some(target), &ps);
                beanval::apply(
                    ctx.idx,
                    &AT::Decl(target),
                    &ps,
                    Some(&name),
                    &Req::Schema(&schema),
                );
                if model::get_nullable(&ps).is_none() && util::is_optional(&pt) {
                    model::set_nullable(&ps, true);
                }
                if let Some(existing) = model::property(&schema, &name) {
                    ps = model::merge(Some(existing), Some(ps)).unwrap();
                }
                model::add_property(&schema, &name, ps);
            }
            if !encodings.is_empty() {
                model::set_v(&media, "encoding", V::Map(encodings));
            }
            model::put_entry(&content, &mt, media);
        }
        Some(content)
    }
}

fn same_target(c: &ParamContext, target: Target, name: Option<&str>) -> bool {
    match c.target {
        Some(t) if t == target => {
            if !matches!(target, Target::Method(_)) {
                true
            } else if let Some(n) = name {
                c.name.is_none() || c.name.as_deref() == Some(n)
            } else {
                c.name.is_some()
            }
        }
        Some(t) => {
            let tm = |t: Target| match t {
                Target::Method(m) | Target::Param(m, _) => Some(m),
                _ => None,
            };
            tm(t).is_some() && tm(t) == tm(target) && name.is_some() && c.name.as_deref() == name
        }
        None => false,
    }
}

fn resolve_with_stack(ctx: &Ctx, t: &Type) -> Type {
    // resolvers are applied from the most recently pushed one
    let mut t = t.clone();
    for stack in ctx.resolver_stack.iter().rev() {
        t = props::resolve_type(&t, stack, true);
    }
    t
}

/// `getType(target)`.
fn target_type(ctx: &Ctx, target: Target, resource_method: MethodId) -> Option<Type> {
    match target {
        Target::Field(f) => Some(ctx.idx.field(f).ty.clone()),
        Target::Method(m) if m != resource_method => {
            let mi = ctx.idx.method(m);
            if mi.params.len() == 1 {
                Some(mi.params[0].ty.clone())
            } else {
                None
            }
        }
        Target::Param(m, p) => Some(ctx.idx.method(m).params[p].ty.clone()),
        _ => None,
    }
}

fn param_name(ctx: &Ctx, a: &Anno, target: Target) -> Option<String> {
    if let Some(v) = a.str("value")
        && !v.is_empty()
    {
        return Some(v);
    }
    match target {
        Target::Field(f) => Some(ctx.idx.field(f).name.clone()),
        Target::Param(m, p) => Some(ctx.idx.method(m).params[p].name.clone()),
        Target::Method(m) => {
            let mi = ctx.idx.method(m);
            if mi.params.len() == 1 {
                Some(match mi.name.strip_prefix("set") {
                    Some(rest) => decapitalize(rest),
                    None => mi.name.clone(),
                })
            } else {
                None
            }
        }
        Target::Class(_) => None,
    }
}

fn decapitalize(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() > 1 && chars[0].is_uppercase() && chars[1].is_uppercase() {
        return s.to_string();
    }
    let mut out: String = chars
        .first()
        .map(|c| c.to_lowercase().collect())
        .unwrap_or_default();
    out.extend(chars.iter().skip(1));
    out
}

fn default_value(ctx: &Ctx, target: Target, resource_method: MethodId) -> Option<V> {
    let a = util::ann_any(
        ctx.idx,
        &AT::Decl(target),
        &["javax.ws.rs.DefaultValue", "jakarta.ws.rs.DefaultValue"],
    )?;
    let s = a.str("value")?;
    if let Some(t) = target_type(ctx, target, resource_method)
        && let crate::index::Ty::Prim(p) = t.ty
    {
        return Some(match p {
            "boolean" => V::Bool(s == "true"),
            "short" | "int" | "long" => s.parse::<i64>().map(V::Int).unwrap_or(V::Str(s)),
            "float" | "double" => V::Dec(s),
            _ => V::Str(s),
        });
    }
    Some(V::Str(s))
}

/// `AugmentedIndexView.ancestry(method)`: the method and its overridden versions in supertypes.
pub fn ancestry(ctx: &Ctx, m: MethodId) -> Vec<MethodId> {
    if let Some(cached) = ctx.ancestry_cache.borrow().get(&m) {
        return cached.clone();
    }
    let result = compute_ancestry(ctx, m);
    ctx.ancestry_cache.borrow_mut().insert(m, result.clone());
    result
}

fn compute_ancestry(ctx: &Ctx, m: MethodId) -> Vec<MethodId> {
    let idx = ctx.idx;
    let declaring = m.class;
    let mi = idx.method(m).clone();
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (c, _) in idx.inheritance_chain(declaring, &Type::class(&idx.classes[declaring].name)) {
        if seen.insert(c)
            && let Some(found) = idx.find_method_like(c, &mi)
        {
            out.push(found);
        }
        for it in idx.interfaces(c) {
            if util::known_java_type(&it.name()) {
                continue;
            }
            if let Some(ic) = idx.id(&it.name())
                && seen.insert(ic)
                && let Some(found) = idx.find_method_like(ic, &mi)
            {
                out.push(found);
            }
        }
    }
    out
}

const PATH: &str = "jakarta.ws.rs.Path";

pub fn is_resource_method(ctx: &Ctx, m: MethodId) -> bool {
    ancestry(ctx, m).iter().any(|a| {
        ctx.idx
            .method(*a)
            .annotations
            .iter()
            .any(|x| is_http_method(&x.name))
    })
}

pub fn is_http_method(name: &str) -> bool {
    matches!(
        name,
        "jakarta.ws.rs.GET"
            | "jakarta.ws.rs.PUT"
            | "jakarta.ws.rs.POST"
            | "jakarta.ws.rs.DELETE"
            | "jakarta.ws.rs.HEAD"
            | "jakarta.ws.rs.OPTIONS"
            | "jakarta.ws.rs.PATCH"
            | "javax.ws.rs.GET"
            | "javax.ws.rs.PUT"
            | "javax.ws.rs.POST"
            | "javax.ws.rs.DELETE"
            | "javax.ws.rs.HEAD"
            | "javax.ws.rs.OPTIONS"
            | "javax.ws.rs.PATCH"
    )
}

fn is_sub_resource_locator(ctx: &Ctx, m: MethodId) -> bool {
    let mi = ctx.idx.method(m);
    matches!(
        mi.return_type.kind(),
        Kind::Class | Kind::Parameterized | Kind::TypeVariable
    ) && mi
        .annotations
        .iter()
        .any(|a| a.name == PATH || a.name == "javax.ws.rs.Path")
        && !mi.annotations.iter().any(|a| is_http_method(&a.name))
}

pub fn path_annotation(annos: &[Anno]) -> Option<String> {
    annos
        .iter()
        .find(|a| a.name == PATH || a.name == "javax.ws.rs.Path")
        .and_then(|a| a.str("value"))
}

pub fn class_paths(ctx: &Ctx, class: ClassId) -> Vec<String> {
    match path_annotation(&ctx.idx.classes[class].annotations) {
        Some(p) => vec![strip_slashes(&p)],
        None => vec![String::new()],
    }
}

pub fn method_paths_of(ctx: &Ctx, m: MethodId) -> Vec<String> {
    let p = ancestry(ctx, m)
        .into_iter()
        .find_map(|a| path_annotation(&ctx.idx.method(a).annotations));
    match p {
        Some(p) => vec![strip_slashes(&p)],
        None => vec![String::new()],
    }
}

fn full_paths_of_method(ctx: &Ctx, m: MethodId) -> Vec<String> {
    let method_paths = method_paths_of(ctx, m);
    let class_paths = class_paths(ctx, m.class);
    let mut out = Vec::new();
    for c in &class_paths {
        for mp in &method_paths {
            let p = if mp.is_empty() {
                c.clone()
            } else {
                format!("{c}/{mp}")
            };
            out.push(format!("/{p}"));
        }
    }
    out
}

fn generate_paths(paths: &[String], params: &[M]) -> Vec<String> {
    let template = &*TEMPLATE_WITH_PATTERN;
    paths
        .iter()
        .map(|p| {
            let mut path = if p.is_empty() {
                String::new()
            } else {
                format!("/{p}")
            };
            while let Some(cap) = template.captures(&path) {
                let name = cap[1].trim().to_string();
                let pattern = cap[2].trim().to_string();
                for prm in params {
                    if model::get_str(prm, "name").as_deref() == Some(&name)
                        && model::get_str(prm, "in").as_deref() == Some("path")
                        && let Some(s) = model::get_obj(prm, "schema")
                        && model::types(&s).is_some_and(|t| t.iter().any(|x| x == "string"))
                        && !model::has(&s, "pattern")
                    {
                        model::set_v(&s, "pattern", V::Str(pattern.clone()));
                    }
                }
                let m = cap.get(0).unwrap();
                path = format!("{}{{{name}}}{}", &path[..m.start()], &path[m.end()..]);
            }
            path
        })
        .collect()
}
