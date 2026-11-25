//! Port of the parts of smallrye-open-api's annotation scanner that Quarkus uses for a JAX-RS application.

pub mod beanval;
pub mod jaxrs;
pub mod params;
pub mod props;
pub mod schema;
pub mod util;

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use crate::index::{Index, MethodId, Ty, Type};
use crate::model::{self, M, MK, V};

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub operation_id_strategy: Option<String>,
    pub scan_profiles: Vec<String>,
    pub scan_exclude_profiles: Vec<String>,
    pub default_produces: Option<Vec<String>>,
    pub default_consumes: Option<Vec<String>>,
    pub default_primitives_produces: Option<Vec<String>>,
    pub default_primitives_consumes: Option<Vec<String>>,
    pub default_streaming_produces: Option<Vec<String>>,
    pub default_streaming_consumes: Option<Vec<String>>,
    pub servers: Vec<String>,
    pub info_title: Option<String>,
    pub info_version: Option<String>,
}

/// A type resolution map stack (`TypeResolver.resolutionStack`). Front of the deque is index 0.
pub type ResolutionStack = Vec<HashMap<String, Type>>;

pub struct Ctx<'a> {
    pub idx: &'a Index,
    pub cfg: &'a Config,
    pub openapi: M,
    pub registry: Registry,
    /// `context.getScanStack()`
    pub scan_stack: Vec<Type>,
    /// `context.getResolverStack()`: resolution stacks of resource class `TypeResolver`s.
    pub resolver_stack: Vec<ResolutionStack>,
    /// `context.getCurrentScanner()` is the JAX-RS scanner.
    pub jaxrs_active: bool,
    pub current_consumes: Option<Vec<String>>,
    pub current_produces: Option<Vec<String>>,
    pub default_consumes: Option<Vec<String>>,
    pub default_produces: Option<Vec<String>>,
    pub operation_ids: HashMap<String, MethodId>,
    errors: std::cell::RefCell<Vec<String>>,
    pub ancestry_cache: std::cell::RefCell<HashMap<MethodId, Vec<MethodId>>>,
}

impl<'a> Ctx<'a> {
    pub fn new(idx: &'a Index, cfg: &'a Config) -> Ctx<'a> {
        Ctx {
            idx,
            cfg,
            openapi: model::new(MK::OpenAPI),
            registry: Registry::default(),
            scan_stack: Vec::new(),
            resolver_stack: Vec::new(),
            jaxrs_active: false,
            current_consumes: None,
            current_produces: None,
            default_consumes: None,
            default_produces: None,
            operation_ids: HashMap::new(),
            errors: std::cell::RefCell::new(Vec::new()),
            ancestry_cache: std::cell::RefCell::new(HashMap::new()),
        }
    }

    /// `context.getResourceTypeResolver().resolve(type)`: the resolver on top of the resolver stack.
    pub fn resource_resolve(&self, t: &Type) -> Type {
        match self.resolver_stack.last() {
            Some(stack) => props::resolve_type(t, stack, true),
            None => t.clone(),
        }
    }

    /// Records a construct the generator cannot reproduce; generation fails at the end.
    pub fn unsupported(&self, msg: String) {
        self.errors.borrow_mut().push(msg);
    }

    pub fn take_errors(&self) -> Vec<String> {
        std::mem::take(&mut self.errors.borrow_mut())
    }

    pub fn components(&self) -> M {
        match model::get_obj(&self.openapi, "components") {
            Some(c) => c,
            None => {
                let c = model::new(MK::Components);
                model::set_v(&self.openapi, "components", V::Obj(c.clone()));
                c
            }
        }
    }

    pub fn add_component_schema(&self, name: &str, schema: &M) {
        let components = self.components();
        model::put_map(&components, "schemas", name, V::Obj(schema.clone()));
    }
}

// ---------------------------------------------------------------- schema registry

struct Info {
    name: String,
    schema: Option<M>,
    schema_ref: String,
}

#[derive(Default)]
pub struct Registry {
    entries: IndexMap<String, Info>,
    names: HashSet<String>,
}

/// `SchemaRegistry.TypeKey` equality: top-level annotations are ignored, nested type arguments compare fully.
pub fn type_key(t: &Type) -> String {
    match &t.ty {
        Ty::Param { name, args, .. } => format!(
            "P:{name}<{}>",
            args.iter().map(full_key).collect::<Vec<_>>().join(",")
        ),
        Ty::Var { id, bounds } => format!(
            "V:{id}:{}",
            bounds.iter().map(full_key).collect::<Vec<_>>().join("&")
        ),
        Ty::Wildcard { extends, super_ } => format!(
            "W:{}:{}",
            extends.as_ref().map(|t| full_key(t)).unwrap_or_default(),
            super_.as_ref().map(|t| full_key(t)).unwrap_or_default()
        ),
        Ty::Class(n) => format!("C:{n}"),
        other => format!("{other:?}"),
    }
}

fn full_key(t: &Type) -> String {
    format!("{}@{:?}", type_key(t), t.annotations)
}

/// `TypeKey.defaultName()`.
fn default_name(t: &Type) -> String {
    let mut name = t.local_name();
    match &t.ty {
        Ty::Param { args, .. } => append_param_names(&mut name, args),
        Ty::Wildcard { .. } => name.push_str(&wildcard_name(t)),
        _ => {}
    }
    name
}

fn append_param_names(name: &mut String, args: &[Type]) {
    for a in args {
        match &a.ty {
            Ty::Param { args: inner, .. } => {
                name.push_str(&a.local_name());
                append_param_names(name, inner);
            }
            Ty::Wildcard { .. } => name.push_str(&wildcard_name(a)),
            _ => name.push_str(&a.local_name()),
        }
    }
}

fn wildcard_name(t: &Type) -> String {
    match &t.ty {
        Ty::Wildcard {
            super_: Some(s), ..
        } => format!("Super{}", s.local_name()),
        Ty::Wildcard {
            extends: Some(e), ..
        } => {
            if e.name() != crate::index::OBJECT {
                format!("Extends{}", e.local_name())
            } else {
                e.local_name()
            }
        }
        _ => String::new(),
    }
}

impl Registry {
    pub fn has_ref(&self, t: &Type) -> bool {
        self.entries.contains_key(&type_key(t))
    }

    pub fn has_schema(&self, t: &Type) -> bool {
        self.entries
            .get(&type_key(t))
            .is_some_and(|i| i.schema.is_some())
    }

    pub fn lookup_ref(&self, t: &Type) -> M {
        let info = self
            .entries
            .get(&type_key(t))
            .unwrap_or_else(|| panic!("not registered: {t}"));
        model::schema_ref(&info.schema_ref)
    }

    pub fn lookup_schema(&self, t: &Type) -> Option<M> {
        self.entries
            .get(&type_key(t))
            .and_then(|i| i.schema.clone())
    }

    fn derive_name(&self, ctx_idx: &Index, t: &Type) -> String {
        let mut schema_name = None;
        if let Some(ci) = ctx_idx.class(&t.name())
            && let Some(a) = ci.annotation(util::SCHEMA)
        {
            schema_name = a.str("name");
        }
        let base = schema_name.unwrap_or_else(|| default_name(t));
        let mut name = base.clone();
        let mut i = 1;
        while self.names.contains(&name) {
            name = format!("{base}{i}");
            i += 1;
        }
        name
    }

    fn insert(&mut self, ctx: &Ctx, t: &Type, schema: Option<M>) -> M {
        let name = self.derive_name(ctx.idx, t);
        let r = format!("{}{name}", model::SCHEMA_REF_PREFIX);
        if let Some(s) = &schema {
            ctx.add_component_schema(&name, s);
        }
        self.names.insert(name.clone());
        self.entries.insert(
            type_key(t),
            Info {
                name,
                schema,
                schema_ref: r.clone(),
            },
        );
        model::schema_ref(&r)
    }

    fn remove(&mut self, t: &Type) {
        if let Some(info) = self.entries.shift_remove(&type_key(t)) {
            self.names.remove(&info.name);
        }
    }
}

/// `SchemaRegistry.register(Type, views, Schema)`.
pub fn registry_register(ctx: &mut Ctx, t: &Type, schema: &M) -> M {
    if ctx.registry.has_ref(t) {
        ctx.registry.remove(t);
    }
    let mut reg = std::mem::take(&mut ctx.registry);
    let r = reg.insert(ctx, t, Some(schema.clone()));
    ctx.registry = reg;
    r
}

/// `SchemaRegistry.isTypeRegistrationSupported`.
/// Array schemas are not excluded because `mp.openapi.extensions.smallrye.array-references.enable` defaults to true.
pub fn registration_supported(ctx: &Ctx, t: &Type) -> bool {
    util::allow_registration(ctx.idx, t)
}

pub enum RegAction {
    /// `checkRegistration`: register the schema itself.
    Check,
    /// `registerReference`: register a reference only.
    Reference,
    /// lookup only (`(reg, key) -> null`).
    None,
}

/// `SchemaRegistry.register(type, views, resolver, schema, action)`. Returns `None` when the action returns null.
pub fn registry_register_with(
    ctx: &mut Ctx,
    t: &Type,
    resolver: Option<&ResolutionStack>,
    schema: &M,
    action: RegAction,
) -> Option<M> {
    let resolved = match resolver {
        Some(stack) => props::resolve_type(t, stack, true),
        None => t.clone(),
    };
    match resolved.ty {
        Ty::Class(_) | Ty::Param { .. } | Ty::Var { .. } | Ty::Wildcard { .. } => {}
        _ => return Some(schema.clone()),
    }
    if ctx.registry.has_ref(&resolved) {
        return Some(ctx.registry.lookup_ref(&resolved));
    }
    if !registration_supported(ctx, &resolved) || !ctx.idx.contains(&resolved.name()) {
        return Some(schema.clone());
    }
    let mut reg = std::mem::take(&mut ctx.registry);
    let result = match action {
        RegAction::Check => Some(reg.insert(ctx, &resolved, Some(schema.clone()))),
        RegAction::Reference => Some(reg.insert(ctx, &resolved, None)),
        RegAction::None => None,
    };
    ctx.registry = reg;
    result
}
