//! Port of `BeanValidationScanner`.

use super::util::{self, AT, SCHEMA};
use crate::index::{Anno, Index, Target, Type};
use crate::model::{self, M, V};

const JSON_PROPERTY: &str = "com.fasterxml.jackson.annotation.JsonProperty";

pub fn is_constraint(name: &str) -> bool {
    let simple = name.rsplit('.').next().unwrap_or(name);
    let bv = (name.starts_with("jakarta.validation.constraints.")
        || name.starts_with("javax.validation.constraints."))
        && matches!(
            simple,
            "DecimalMax"
                | "DecimalMin"
                | "Digits"
                | "Max"
                | "Min"
                | "Negative"
                | "NegativeOrZero"
                | "NotBlank"
                | "NotEmpty"
                | "NotNull"
                | "Pattern"
                | "Positive"
                | "PositiveOrZero"
                | "Size"
        );
    bv || name == JSON_PROPERTY
        || name == "org.jetbrains.annotations.Nullable"
        || name == "kotlinx.serialization.Required"
}

/// Constraint reached through an indexed (application) annotation type.
pub fn has_composed_constraint(idx: &Index, target: Target) -> bool {
    idx.declared(target).iter().any(|a| {
        idx.class(&a.name)
            .is_some_and(|ci| ci.annotations.iter().any(|m| is_constraint(&m.name)))
    })
}

/// What happens when a constraint marks the property as required.
pub enum Req<'a> {
    Noop,
    /// `AnnotationTargetProcessor.setRequired` on the parent object schema.
    Parent {
        parent: &'a M,
    },
    /// `AbstractParameterProcessor.setRequired` on a form schema.
    Schema(&'a M),
    /// Request body / parameter: sets `required` to true if not set.
    Flag(&'a M, &'a str),
}

fn set_required(idx: &Index, req: &Req, target: &AT, key: Option<&str>) {
    match req {
        Req::Noop => {}
        Req::Parent { parent } => {
            let Some(key) = key else { return };
            if model::required(parent).is_none_or(|r| !r.iter().any(|x| x == key)) {
                let schema_anno = util::ann(idx, target, SCHEMA);
                if schema_anno.is_none_or(|a| a.value("required").is_none()) {
                    model::add_required(parent, key);
                }
            }
        }
        Req::Schema(schema) => {
            let Some(key) = key else { return };
            if model::required(schema).is_none_or(|r| !r.iter().any(|x| x == key)) {
                model::add_required(schema, key);
            }
        }
        Req::Flag(m, kind) => {
            if *kind == "requestBody" {
                if model::private_ext(m, "is-required-set") != Some(V::Bool(true)) {
                    model::set_v(m, "required", V::Bool(true));
                }
            } else if !model::has(m, "required") {
                model::set_v(m, "required", V::Bool(true));
            }
        }
    }
}

fn constraint(idx: &Index, target: &AT, simple: &str) -> Option<Anno> {
    let a = util::ann_any(
        idx,
        target,
        &[
            &format!("javax.validation.constraints.{simple}"),
            &format!("jakarta.validation.constraints.{simple}"),
        ],
    )?;
    let groups = a.class_array("groups");
    if a.value("groups").is_none() || groups.is_empty() {
        return Some(a);
    }
    if groups.iter().any(|g| {
        g.name() == "jakarta.validation.groups.Default"
            || g.name() == "javax.validation.groups.Default"
    }) {
        return Some(a);
    }
    None
}

pub fn apply_to_type(idx: &Index, t: &Type, schema: Option<&M>) {
    if let Some(s) = schema {
        apply(idx, &AT::Type(t), s, None, &Req::Noop);
    }
}

pub fn apply(idx: &Index, target: &AT, schema: &M, key: Option<&str>, req: &Req) {
    let Some(types) = model::types(schema) else {
        return;
    };
    if types.is_empty() || model::get_ref(schema).is_some() {
        return;
    }
    for t in types {
        match t.as_str() {
            "array" => {
                not_null(idx, target, key, req);
                required_jackson(idx, target, key, req);
                size(idx, target, schema, "minItems", "maxItems");
                not_empty(idx, target, schema, key, req, "minItems");
            }
            "boolean" => {
                not_null(idx, target, key, req);
                required_jackson(idx, target, key, req);
            }
            "integer" | "number" => {
                decimal_max(idx, target, schema);
                decimal_min(idx, target, schema);
                digits_number(idx, target, schema);
                max(idx, target, schema);
                min(idx, target, schema);
                if constraint(idx, target, "Negative").is_some()
                    && !model::has(schema, "maximum")
                    && !model::has(schema, "exclusiveMaximum")
                {
                    model::set_v(schema, "exclusiveMaximum", V::Dec("0".into()));
                }
                if constraint(idx, target, "NegativeOrZero").is_some()
                    && !model::has(schema, "maximum")
                    && !model::has(schema, "exclusiveMaximum")
                {
                    model::set_v(schema, "maximum", V::Dec("0".into()));
                }
                not_null(idx, target, key, req);
                required_jackson(idx, target, key, req);
                if constraint(idx, target, "Positive").is_some()
                    && !model::has(schema, "minimum")
                    && !model::has(schema, "exclusiveMinimum")
                {
                    model::set_v(schema, "exclusiveMinimum", V::Dec("0".into()));
                }
                if constraint(idx, target, "PositiveOrZero").is_some()
                    && !model::has(schema, "minimum")
                    && !model::has(schema, "exclusiveMinimum")
                {
                    model::set_v(schema, "minimum", V::Dec("0".into()));
                }
            }
            "object" => {
                not_null(idx, target, key, req);
                required_jackson(idx, target, key, req);
                if model::has(schema, "additionalProperties") {
                    size(idx, target, schema, "minProperties", "maxProperties");
                    not_empty(idx, target, schema, key, req, "minProperties");
                }
            }
            "string" => {
                decimal_max(idx, target, schema);
                decimal_min(idx, target, schema);
                if let Some(c) = constraint(idx, target, "Pattern")
                    && !model::has(schema, "pattern")
                {
                    model::set(schema, "pattern", c.str("regexp").map(V::Str));
                }
                digits_string(idx, target, schema);
                if constraint(idx, target, "NotBlank").is_some() {
                    if !model::has(schema, "pattern") {
                        model::set_v(schema, "pattern", V::Str("\\S".into()));
                    }
                    set_required(idx, req, target, key);
                }
                not_null(idx, target, key, req);
                required_jackson(idx, target, key, req);
                size(idx, target, schema, "minLength", "maxLength");
                not_empty(idx, target, schema, key, req, "minLength");
            }
            _ => {}
        }
    }
}

fn not_null(idx: &Index, target: &AT, key: Option<&str>, req: &Req) {
    if constraint(idx, target, "NotNull").is_some() {
        set_required(idx, req, target, key);
    }
}

fn required_jackson(idx: &Index, target: &AT, key: Option<&str>, req: &Req) {
    if util::ann(idx, target, JSON_PROPERTY).and_then(|a| a.bool("required")) == Some(true) {
        set_required(idx, req, target, key);
    }
}

fn size(idx: &Index, target: &AT, schema: &M, min_key: &str, max_key: &str) {
    if let Some(c) = constraint(idx, target, "Size") {
        if let Some(min) = c.int("min")
            && !model::has(schema, min_key)
        {
            model::set_v(schema, min_key, V::Int(min));
        }
        if let Some(max) = c.int("max")
            && !model::has(schema, max_key)
        {
            model::set_v(schema, max_key, V::Int(max));
        }
    }
}

fn not_empty(idx: &Index, target: &AT, schema: &M, key: Option<&str>, req: &Req, min_key: &str) {
    if constraint(idx, target, "NotEmpty").is_some() {
        if !model::has(schema, min_key) {
            model::set_v(schema, min_key, V::Int(1));
        }
        set_required(idx, req, target, key);
    }
}

fn decimal(value: &str) -> Option<String> {
    let v = value.trim();
    if v.parse::<f64>().is_ok() {
        Some(v.to_string())
    } else {
        None
    }
}

fn decimal_max(idx: &Index, target: &AT, schema: &M) {
    if let Some(c) = constraint(idx, target, "DecimalMax")
        && !model::has(schema, "maximum")
        && !model::has(schema, "exclusiveMaximum")
        && let Some(d) = c.str("value").and_then(|v| decimal(&v))
    {
        let key = if c.bool("inclusive") == Some(false) {
            "exclusiveMaximum"
        } else {
            "maximum"
        };
        model::set_v(schema, key, V::Dec(d));
    }
}

fn decimal_min(idx: &Index, target: &AT, schema: &M) {
    if let Some(c) = constraint(idx, target, "DecimalMin")
        && !model::has(schema, "minimum")
        && !model::has(schema, "exclusiveMinimum")
        && let Some(d) = c.str("value").and_then(|v| decimal(&v))
    {
        let key = if c.bool("inclusive") == Some(false) {
            "exclusiveMinimum"
        } else {
            "minimum"
        };
        model::set_v(schema, key, V::Dec(d));
    }
}

fn max(idx: &Index, target: &AT, schema: &M) {
    if let Some(c) = constraint(idx, target, "Max")
        && !model::has(schema, "maximum")
        && let Some(v) = c.int("value")
    {
        model::set_v(schema, "maximum", V::Dec(v.to_string()));
    }
}

fn min(idx: &Index, target: &AT, schema: &M) {
    if let Some(c) = constraint(idx, target, "Min")
        && !model::has(schema, "minimum")
        && let Some(v) = c.int("value")
    {
        model::set_v(schema, "minimum", V::Dec(v.to_string()));
    }
}

fn digits_string(idx: &Index, target: &AT, schema: &M) {
    if let Some(c) = constraint(idx, target, "Digits")
        && !model::has(schema, "pattern")
    {
        let integer = c.int("integer").unwrap_or(0);
        let fraction = c.int("fraction").unwrap_or(0);
        let mut p = String::from("^");
        if integer > 0 {
            p.push_str("\\d");
            if integer > 1 {
                p.push_str(&format!("{{1,{integer}}}"));
            }
        }
        if fraction > 0 {
            p.push_str("([.]\\d");
            if fraction > 1 {
                p.push_str(&format!("{{1,{fraction}}}"));
            }
            p.push_str(")?");
        }
        p.push('$');
        model::set_v(schema, "pattern", V::Str(p));
    }
}

fn digits_number(idx: &Index, target: &AT, schema: &M) {
    if let Some(c) = constraint(idx, target, "Digits")
        && !model::has(schema, "multipleOf")
    {
        let fraction = c.int("fraction").unwrap_or(0);
        let v = if fraction > 0 {
            format!("1E-{fraction}")
        } else {
            "1".to_string()
        };
        model::set_v(schema, "multipleOf", V::Dec(v));
    }
}
