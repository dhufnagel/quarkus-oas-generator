//! OpenAPI object model mirroring SmallRye's `BaseModel`: every model object is an insertion-ordered property
//! map. Objects are shared by reference like in Java, so schemas registered as components can still be filled
//! in after a `$ref` to them was handed out.

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;

pub const PRIVATE_EXT: &str = "x-smallrye-private-";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MK {
    OpenAPI,
    Info,
    Components,
    Paths,
    PathItem,
    Operation,
    Parameter,
    RequestBody,
    Content,
    MediaType,
    Schema,
    APIResponses,
    APIResponse,
    Tag,
    Server,
    SecurityRequirement,
    SecurityScheme,
    Discriminator,
    Encoding,
    ExternalDocs,
}

#[derive(Debug)]
pub struct Model {
    pub kind: MK,
    pub props: IndexMap<String, V>,
    pub modcount: u32,
}

pub type M = Rc<RefCell<Model>>;

#[derive(Clone, Debug)]
pub enum V {
    Str(String),
    Bool(bool),
    Int(i64),
    /// `BigDecimal`, kept in its canonical text form.
    Dec(String),
    List(Vec<V>),
    Map(IndexMap<String, V>),
    Obj(M),
}

impl PartialEq for V {
    fn eq(&self, other: &V) -> bool {
        match (self, other) {
            (V::Str(a), V::Str(b)) => a == b,
            (V::Bool(a), V::Bool(b)) => a == b,
            (V::Int(a), V::Int(b)) => a == b,
            (V::Dec(a), V::Dec(b)) => a == b,
            (V::List(a), V::List(b)) => a == b,
            (V::Map(a), V::Map(b)) => {
                a.len() == b.len() && a.iter().all(|(k, v)| b.get(k) == Some(v))
            }
            (V::Obj(a), V::Obj(b)) => model_eq(a, b),
            _ => false,
        }
    }
}

/// `BaseModel.equals`: equal property maps (order-insensitive like `LinkedHashMap.equals`).
pub fn model_eq(a: &M, b: &M) -> bool {
    if Rc::ptr_eq(a, b) {
        return true;
    }
    let a = a.borrow();
    let b = b.borrow();
    a.kind == b.kind
        && a.props.len() == b.props.len()
        && a.props.iter().all(|(k, v)| b.props.get(k) == Some(v))
}

pub fn new(kind: MK) -> M {
    Rc::new(RefCell::new(Model {
        kind,
        props: IndexMap::new(),
        modcount: 0,
    }))
}

pub fn kind(m: &M) -> MK {
    m.borrow().kind
}

pub fn get(m: &M, key: &str) -> Option<V> {
    m.borrow().props.get(key).cloned()
}

pub fn has(m: &M, key: &str) -> bool {
    m.borrow().props.contains_key(key)
}

/// `BaseModel.setProperty`: `None` removes the key; an existing key keeps its position.
pub fn set(m: &M, key: &str, value: Option<V>) {
    let mut mm = m.borrow_mut();
    match value {
        Some(v) => {
            mm.props.insert(key.to_string(), v);
        }
        None => {
            mm.props.shift_remove(key);
        }
    }
    mm.modcount += 1;
}

pub fn set_v(m: &M, key: &str, value: V) {
    set(m, key, Some(value));
}

pub fn get_str(m: &M, key: &str) -> Option<String> {
    match get(m, key) {
        Some(V::Str(s)) => Some(s),
        _ => None,
    }
}

pub fn get_bool(m: &M, key: &str) -> Option<bool> {
    match get(m, key) {
        Some(V::Bool(b)) => Some(b),
        _ => None,
    }
}

pub fn get_obj(m: &M, key: &str) -> Option<M> {
    match get(m, key) {
        Some(V::Obj(o)) => Some(o),
        _ => None,
    }
}

pub fn get_list(m: &M, key: &str) -> Option<Vec<V>> {
    match get(m, key) {
        Some(V::List(l)) => Some(l),
        _ => None,
    }
}

pub fn get_obj_list(m: &M, key: &str) -> Option<Vec<M>> {
    get_list(m, key).map(|l| {
        l.into_iter()
            .filter_map(|v| match v {
                V::Obj(o) => Some(o),
                _ => None,
            })
            .collect()
    })
}

/// `addListPropertyEntry`.
pub fn add_list(m: &M, key: &str, value: V) {
    let mut list = get_list(m, key).unwrap_or_default();
    list.push(value);
    set(m, key, Some(V::List(list)));
}

/// `removeListPropertyEntry`.
pub fn remove_list(m: &M, key: &str, value: &V) {
    if let Some(mut list) = get_list(m, key) {
        if let Some(pos) = list.iter().position(|v| v == value) {
            list.remove(pos);
        }
        m.borrow_mut().props.insert(key.to_string(), V::List(list));
    }
    m.borrow_mut().modcount += 1;
}

/// `putMapPropertyEntry`.
pub fn put_map(m: &M, key: &str, entry: &str, value: V) {
    let mut map = match get(m, key) {
        Some(V::Map(map)) => map,
        _ => IndexMap::new(),
    };
    map.insert(entry.to_string(), value);
    set(m, key, Some(V::Map(map)));
}

pub fn get_map(m: &M, key: &str) -> Option<IndexMap<String, V>> {
    match get(m, key) {
        Some(V::Map(map)) => Some(map),
        _ => None,
    }
}

/// Public (non-private) properties, like `BaseExtensibleModel.getAllProperties`.
pub fn public_props(m: &M) -> Vec<(String, V)> {
    m.borrow()
        .props
        .iter()
        .filter(|(k, _)| !k.starts_with(PRIVATE_EXT))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

pub fn is_empty_public(m: &M) -> bool {
    m.borrow().props.keys().all(|k| k.starts_with(PRIVATE_EXT))
}

pub fn modcount(m: &M) -> u32 {
    m.borrow().modcount
}

// ---------------------------------------------------------------- extensions

pub fn private_ext(m: &M, name: &str) -> Option<V> {
    get(m, &format!("{PRIVATE_EXT}{name}"))
}

pub fn set_private_ext(m: &M, name: &str, value: Option<V>) {
    set(m, &format!("{PRIVATE_EXT}{name}"), value);
}

pub fn extensions(m: &M) -> Vec<(String, V)> {
    m.borrow()
        .props
        .iter()
        .filter(|(k, _)| k.starts_with("x-"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

// ---------------------------------------------------------------- copy & merge

pub fn deep_copy(m: &M) -> M {
    let src = m.borrow();
    let props = src
        .props
        .iter()
        .map(|(k, v)| (k.clone(), deep_copy_v(v)))
        .collect();
    Rc::new(RefCell::new(Model {
        kind: src.kind,
        props,
        modcount: 0,
    }))
}

fn deep_copy_v(v: &V) -> V {
    match v {
        V::List(l) => V::List(l.iter().map(deep_copy_v).collect()),
        V::Map(m) => V::Map(m.iter().map(|(k, v)| (k.clone(), deep_copy_v(v))).collect()),
        V::Obj(o) => V::Obj(deep_copy(o)),
        other => other.clone(),
    }
}

fn same_class(a: &V, b: &V) -> bool {
    match (a, b) {
        (V::Obj(x), V::Obj(y)) => x.borrow().kind == y.borrow().kind,
        _ => std::mem::discriminant(a) == std::mem::discriminant(b),
    }
}

/// `MergeUtil.mergeObjects` for two models: merges `b` into `a` and returns `a`.
pub fn merge(a: Option<M>, b: Option<M>) -> Option<M> {
    match (a, b) {
        (None, b) => b,
        (a, None) => a,
        (Some(a), Some(b)) => {
            if Rc::ptr_eq(&a, &b) {
                return Some(a);
            }
            if a.borrow().kind != b.borrow().kind {
                return Some(b);
            }
            merge_into(&a, &b);
            Some(a)
        }
    }
}

fn merge_into(a: &M, b: &M) {
    let entries: Vec<(String, V)> = b
        .borrow()
        .props
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let kind = a.borrow().kind;
    for (name, new_value) in entries {
        let old = get(a, &name);
        if let Some(V::Obj(o)) = &old
            && Rc::ptr_eq(o, b)
        {
            return;
        }
        if let V::Obj(n) = &new_value
            && Rc::ptr_eq(n, a)
        {
            return;
        }
        let preserve = kind == MK::OpenAPI && name == "openapi";
        let override_ = kind == MK::Schema && name == "examples";
        match old {
            Some(_) if preserve => {}
            None => set(a, &name, Some(new_value)),
            Some(o) if !same_class(&o, &new_value) || override_ => set(a, &name, Some(new_value)),
            Some(V::Obj(o)) => {
                if let V::Obj(n) = new_value {
                    merge_into(&o, &n);
                }
            }
            Some(V::Map(o)) => {
                if let V::Map(n) = new_value {
                    set(a, &name, Some(V::Map(merge_maps(o, n))));
                }
            }
            Some(V::List(o)) => {
                if let V::List(n) = new_value {
                    set(a, &name, Some(V::List(merge_lists(o, n))));
                }
            }
            Some(_) => set(a, &name, Some(new_value)),
        }
    }
}

fn merge_maps(a: IndexMap<String, V>, b: IndexMap<String, V>) -> IndexMap<String, V> {
    let mut result = a;
    for (k, v2) in b {
        let value = match result.get(&k) {
            Some(V::Map(m1)) => match v2 {
                V::Map(m2) => V::Map(merge_maps(m1.clone(), m2)),
                other => other,
            },
            Some(V::List(l1)) => match v2 {
                V::List(l2) => V::List(merge_lists(l1.clone(), l2)),
                other => other,
            },
            Some(V::Obj(o1)) => match &v2 {
                V::Obj(o2) => V::Obj(merge(Some(o1.clone()), Some(o2.clone())).unwrap()),
                _ => v2,
            },
            _ => v2,
        };
        result.insert(k, value);
    }
    result
}

fn merge_lists(a: Vec<V>, b: Vec<V>) -> Vec<V> {
    if a == b {
        return a;
    }
    if a.is_empty() {
        return b;
    }
    match &a[0] {
        V::Str(_) => {
            let mut out = a.clone();
            for v in b {
                if !out.contains(&v) {
                    out.push(v);
                }
            }
            out
        }
        V::Obj(first) => {
            let kind = first.borrow().kind;
            match kind {
                MK::Tag => {
                    let mut out = a.clone();
                    for v2 in b {
                        let name2 = if let V::Obj(o) = &v2 {
                            get_str(o, "name")
                        } else {
                            None
                        };
                        let matched = out.iter().find(|v1| {
                            matches!(v1, V::Obj(o) if get_str(o, "name").is_some() && get_str(o, "name") == name2)
                        });
                        match matched {
                            Some(V::Obj(m1)) => {
                                if let V::Obj(m2) = &v2 {
                                    merge(Some(m1.clone()), Some(m2.clone()));
                                }
                            }
                            _ => out.push(v2),
                        }
                    }
                    out
                }
                MK::SecurityRequirement | MK::Server => {
                    let mut out = a.clone();
                    for v in b {
                        if !out.contains(&v) {
                            out.push(v);
                        }
                    }
                    out
                }
                MK::Parameter => {
                    let mut out = a.clone();
                    for v2 in b {
                        let key = |v: &V| match v {
                            V::Obj(o) => (get_str(o, "name"), get_str(o, "in")),
                            _ => (None, None),
                        };
                        let k2 = key(&v2);
                        match out.iter().position(|v1| key(v1) == k2) {
                            Some(pos) => {
                                if let (V::Obj(m1), V::Obj(m2)) = (out[pos].clone(), &v2) {
                                    merge(Some(m1), Some(m2.clone()));
                                }
                            }
                            None => out.push(v2),
                        }
                    }
                    out
                }
                _ => {
                    let mut out = a;
                    out.extend(b);
                    out
                }
            }
        }
        _ => {
            let mut out = a;
            out.extend(b);
            out
        }
    }
}

// ---------------------------------------------------------------- schema helpers

pub const SCHEMA_REF_PREFIX: &str = "#/components/schemas/";

pub fn schema() -> M {
    new(MK::Schema)
}

pub fn schema_ref(name_or_ref: &str) -> M {
    let s = schema();
    set_ref(&s, Some(name_or_ref));
    s
}

/// `setRef`: a bare name is expanded to a component reference.
pub fn set_ref(m: &M, r: Option<&str>) {
    let value = r.map(|r| {
        if r.contains('/') {
            r.to_string()
        } else {
            let prefix = match kind(m) {
                MK::Schema => "#/components/schemas/",
                MK::Parameter => "#/components/parameters/",
                MK::RequestBody => "#/components/requestBodies/",
                MK::APIResponse => "#/components/responses/",
                MK::SecurityScheme => "#/components/securitySchemes/",
                _ => "",
            };
            format!("{prefix}{r}")
        }
    });
    set(m, "$ref", value.map(V::Str));
}

pub fn get_ref(m: &M) -> Option<String> {
    get_str(m, "$ref")
}

pub fn types(m: &M) -> Option<Vec<String>> {
    get_list(m, "type").map(|l| {
        l.into_iter()
            .filter_map(|v| match v {
                V::Str(s) => Some(s),
                _ => None,
            })
            .collect()
    })
}

fn types_v(t: &[&str]) -> V {
    V::List(t.iter().map(|s| V::Str(s.to_string())).collect())
}

fn observers(m: &M) -> Vec<M> {
    match private_ext(m, "schema-type-observers") {
        Some(V::List(l)) => l
            .into_iter()
            .filter_map(|v| match v {
                V::Obj(o) => Some(o),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// `Schema.setType(List)` (raw, no observers).
pub fn set_type_list(m: &M, t: Option<Vec<String>>) {
    set(
        m,
        "type",
        t.map(|t| V::List(t.into_iter().map(V::Str).collect())),
    );
}

/// `Schema.addType` incl. type observers.
pub fn add_type(m: &M, t: &str) {
    add_list(m, "type", V::Str(t.to_string()));
    for o in observers(m) {
        add_type(&o, t);
    }
}

/// `Schema.removeType` incl. type observers.
pub fn remove_type(m: &M, t: &str) {
    remove_list(m, "type", &V::Str(t.to_string()));
    for o in observers(m) {
        remove_type(&o, t);
    }
}

pub fn null_schema() -> M {
    let s = schema();
    set(&s, "type", Some(types_v(&["null"])));
    s
}

/// `SchemaSupport.getNullable`.
pub fn get_nullable(m: &M) -> Option<bool> {
    let nullable = match private_ext(m, "nullable") {
        Some(V::Bool(b)) => Some(b),
        _ => None,
    };
    match types(m) {
        Some(t) => {
            let null_permitted = t.iter().any(|x| x == "null");
            if !null_permitted && nullable.is_none() {
                None
            } else {
                Some(null_permitted)
            }
        }
        None => nullable,
    }
}

/// `SchemaSupport.setNullable`.
pub fn set_nullable(m: &M, nullable: bool) {
    set_private_ext(m, "nullable", Some(V::Bool(nullable)));
    if nullable {
        if let Some(t) = types(m)
            && !t.iter().any(|x| x == "null")
        {
            add_type(m, "null");
        }
    } else {
        remove_type(m, "null");
    }
}

/// `SchemaSupport.setType(schema, singletonType)`.
pub fn set_type(m: &M, t: Option<&str>) {
    match t {
        None => set_type_list(m, None),
        Some(t) if get_nullable(m) == Some(true) => {
            set_type_list(m, Some(vec![t.to_string(), "null".to_string()]))
        }
        Some(t) => set_type_list(m, Some(vec![t.to_string()])),
    }
    for o in observers(m) {
        set_type(&o, t);
    }
}

pub fn non_null_type(m: &M) -> Option<String> {
    types(m)?.into_iter().find(|t| t != "null")
}

/// `SchemaSupport.addTypeObserver`.
pub fn add_type_observer(observed: &M, observer: &M) {
    let mut list = match private_ext(observed, "schema-type-observers") {
        Some(V::List(l)) => l,
        _ => Vec::new(),
    };
    list.push(V::Obj(observer.clone()));
    set_private_ext(observed, "schema-type-observers", Some(V::List(list)));
    // setTypesRetainingNull
    let new_types = types(observed);
    let old_types = types(observer);
    let new_types = match (old_types, new_types) {
        (Some(old), Some(mut new))
            if old.iter().any(|t| t == "null") && !new.iter().any(|t| t == "null") =>
        {
            new.push("null".to_string());
            Some(new)
        }
        (_, new) => new,
    };
    set_type_list(observer, new_types);
}

pub fn add_any_of(m: &M, s: M) {
    add_list(m, "anyOf", V::Obj(s));
}

pub fn add_all_of(m: &M, s: M) {
    add_list(m, "allOf", V::Obj(s));
}

pub fn add_required(m: &M, name: &str) {
    add_list(m, "required", V::Str(name.to_string()));
}

pub fn required(m: &M) -> Option<Vec<String>> {
    get_list(m, "required").map(|l| {
        l.into_iter()
            .filter_map(|v| match v {
                V::Str(s) => Some(s),
                _ => None,
            })
            .collect()
    })
}

pub fn add_property(m: &M, name: &str, s: M) {
    put_map(m, "properties", name, V::Obj(s));
}

pub fn property(m: &M, name: &str) -> Option<M> {
    match get_map(m, "properties")?.get(name) {
        Some(V::Obj(o)) => Some(o.clone()),
        _ => None,
    }
}

/// Adds an entry to an "unwrapped" container (paths, responses, content, ...).
pub fn put_entry(m: &M, key: &str, value: M) {
    set(m, key, Some(V::Obj(value)));
}

pub fn entry(m: &M, key: &str) -> Option<M> {
    get_obj(m, key)
}

pub fn entries(m: &M) -> Vec<(String, M)> {
    m.borrow()
        .props
        .iter()
        .filter(|(k, _)| !k.starts_with("x-"))
        .filter_map(|(k, v)| match v {
            V::Obj(o) => Some((k.clone(), o.clone())),
            _ => None,
        })
        .collect()
}
