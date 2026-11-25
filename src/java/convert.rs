//! Builds the declaration model of [`super::ast`] from the syntax tree of the jtk parser.
//!
//! jtk keeps modifier keywords and literal values only as source positions, so they are read from the source here.

use super::ast::*;
use super::literal;
use jtk_parser::ast::{self as j, K, LitKind, Tree};
use jtk_parser::lexer::{RawKind, lex};

const MODIFIERS: &[&str] = &[
    "public",
    "protected",
    "private",
    "static",
    "abstract",
    "final",
    "native",
    "synchronized",
    "transient",
    "volatile",
    "strictfp",
    "default",
    "sealed",
];

/// Binary operators of constant expressions.
const BINARY_OPS: &[&str] = &[
    "||", "&&", "|", "^", "&", "==", "!=", "<", ">", "<=", ">=", "<<", ">>", ">>>", "+", "-", "*",
    "/", "%",
];

pub fn parse(src: &str) -> Result<CompilationUnit, String> {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    let tree = jtk_parser::parser::parse(src).map_err(|e| {
        let line = src[..e.offset.min(src.len())].matches('\n').count() + 1;
        format!("line {line}: syntax error")
    })?;
    Converter { src }.unit(&tree)
}

struct Converter<'s> {
    src: &'s str,
}

type R<T> = Result<T, String>;

impl Converter<'_> {
    fn unit(&self, tree: &Tree) -> R<CompilationUnit> {
        let K::CompilationUnit {
            package,
            imports,
            types,
            ..
        } = &tree.kind
        else {
            return Err("not a compilation unit".into());
        };
        let package = match package {
            Some((_, name)) => self.qualified_name(name)?,
            None => String::new(),
        };
        let mut unit = CompilationUnit {
            package,
            imports: Vec::new(),
            types: Vec::new(),
        };
        for import in imports {
            if let K::Import { is_static, name } = &import.kind {
                let name = self.qualified_name(name)?;
                let (name, wildcard) = match name.strip_suffix(".*") {
                    Some(prefix) => (prefix.to_string(), true),
                    None => (name, false),
                };
                unit.imports.push(Import {
                    is_static: *is_static,
                    name,
                    wildcard,
                });
            }
        }
        for t in types {
            if let K::Class(c) = &t.kind {
                unit.types.push(self.type_decl(c)?);
            }
        }
        Ok(unit)
    }

    fn type_decl(&self, c: &j::ClassDecl) -> R<TypeDecl> {
        let kind = match c.kind {
            j::ClassKind::Class => TypeKind::Class,
            j::ClassKind::Interface => TypeKind::Interface,
            j::ClassKind::Enum => TypeKind::Enum,
            j::ClassKind::Record => TypeKind::Record,
            j::ClassKind::Annotation => TypeKind::Annotation,
        };
        // jtk stores the `extends` list of interfaces as `implements`
        let (extends, implements) = match kind {
            TypeKind::Interface => (self.type_refs(&c.implements)?, Vec::new()),
            _ => (
                c.extends
                    .iter()
                    .map(|t| self.type_ref(t))
                    .collect::<R<_>>()?,
                self.type_refs(&c.implements)?,
            ),
        };
        let mut decl = TypeDecl {
            kind,
            name: c.name.clone(),
            modifiers: self.modifiers(&c.mods, c.name_pos)?,
            type_params: self.type_params(&c.type_params)?,
            extends,
            implements,
            record_components: c
                .record_components
                .iter()
                .map(|v| self.param(v))
                .collect::<R<_>>()?,
            enum_constants: c
                .enum_constants
                .iter()
                .map(|e| {
                    Ok(EnumConstant {
                        name: e.name.clone(),
                        annotations: self.annotations(&e.mods.annotations)?,
                    })
                })
                .collect::<R<_>>()?,
            fields: Vec::new(),
            methods: Vec::new(),
            types: Vec::new(),
            member_order: Vec::new(),
        };
        for member in &c.members {
            match &member.kind {
                K::Vars(vars) => self.fields(vars, &mut decl)?,
                K::Method(m) => {
                    let method = self.method(m)?;
                    decl.member_order.push((false, decl.methods.len()));
                    decl.methods.push(method);
                }
                K::Class(inner) => decl.types.push(self.type_decl(inner)?),
                _ => {}
            }
        }
        Ok(decl)
    }

    fn fields(&self, vars: &[j::VarDecl], decl: &mut TypeDecl) -> R<()> {
        let Some(first) = vars.first() else {
            return Ok(());
        };
        // declarators share the modifiers; later names would include the initializers of earlier ones
        let modifiers = self.modifiers(&first.mods, first.name_pos)?;
        for v in vars {
            let Some(ty) = &v.ty else {
                return Err(format!("field `{}` without type", v.name));
            };
            decl.member_order.push((true, decl.fields.len()));
            decl.fields.push(FieldDecl {
                modifiers: modifiers.clone(),
                ty: self.type_ref(ty)?,
                name: v.name.clone(),
                init: v.init.as_ref().map(|e| self.expr(e).unwrap_or(Expr::Other)),
            });
        }
        Ok(())
    }

    fn method(&self, m: &j::MethodDecl) -> R<MethodDecl> {
        let return_type = match &m.ret {
            None => None,
            Some(ret) => {
                let ty = self.type_ref(ret)?;
                Some(with_annotations(
                    ty,
                    self.annotations(&m.return_type_annotations)?,
                ))
            }
        };
        Ok(MethodDecl {
            modifiers: self.modifiers(&m.mods, m.name_pos)?,
            type_params: self.type_params(&m.type_params)?,
            name: if return_type.is_none() {
                "<init>".into()
            } else {
                m.name.clone()
            },
            return_type,
            params: m.params.iter().map(|p| self.param(p)).collect::<R<_>>()?,
            has_body: m.body.is_some(),
        })
    }

    fn param(&self, v: &j::VarDecl) -> R<Param> {
        let Some(ty) = &v.ty else {
            return Err(format!("parameter `{}` without type", v.name));
        };
        Ok(Param {
            modifiers: self.modifiers(&v.mods, v.name_pos)?,
            ty: self.type_ref(ty)?,
            name: v.name.clone(),
        })
    }

    /// The modifiers of a declaration whose name starts at `name_pos`: its annotations, and the modifier keywords
    /// read from the source between the start of the modifiers and the name.
    fn modifiers(&self, mods: &j::Mods, name_pos: usize) -> R<Modifiers> {
        let region = &self.src[mods.start..name_pos];
        let (tokens, _) = lex(region, false).map_err(|e| e.to_string())?;
        let flags = tokens
            .into_iter()
            .filter(|(kind, _, _)| *kind == RawKind::Token)
            .map(|(_, start, end)| &region[start..end])
            .filter(|t| MODIFIERS.contains(t))
            .map(String::from)
            .collect();
        Ok(Modifiers {
            flags,
            annotations: self.annotations(&mods.annotations)?,
        })
    }

    fn type_params(&self, params: &[Tree]) -> R<Vec<TypeParam>> {
        params
            .iter()
            .filter_map(|p| match &p.kind {
                K::TypeParameter { name, bounds, .. } => Some((name, bounds)),
                _ => None,
            })
            .map(|(name, bounds)| {
                Ok(TypeParam {
                    name: name.clone(),
                    bounds: self.type_refs(bounds)?,
                })
            })
            .collect()
    }

    fn type_refs(&self, types: &[Tree]) -> R<Vec<TypeRef>> {
        types.iter().map(|t| self.type_ref(t)).collect()
    }

    fn type_ref(&self, t: &Tree) -> R<TypeRef> {
        match &t.kind {
            K::Primitive(name) if name == "void" => Ok(TypeRef::Void),
            K::Primitive(name) => Ok(TypeRef::Primitive {
                name: name.clone(),
                annotations: Vec::new(),
            }),
            K::Annotated { annotations, base } if is_leading(t, annotations) => Ok(
                with_annotations(self.type_ref(base)?, self.annotations(annotations)?),
            ),
            K::ArrayType { elem, dims } => {
                let mut ty = self.type_ref(elem)?;
                for annotations in dims {
                    ty = TypeRef::Array {
                        component: Box::new(ty),
                        annotations: self.annotations(annotations)?,
                    };
                }
                Ok(ty)
            }
            _ => {
                let mut segments = Vec::new();
                self.segments(t, &mut segments)?;
                Ok(TypeRef::Class {
                    segments,
                    annotations: Vec::new(),
                })
            }
        }
    }

    /// The segments of a possibly qualified and parameterized class type. Annotations inside qualified names
    /// (`java.lang.@NotNull String`) are dropped.
    fn segments(&self, t: &Tree, out: &mut Vec<(String, Vec<TypeArg>)>) -> R<()> {
        match &t.kind {
            K::Ident(name) => out.push((name.clone(), Vec::new())),
            K::MemberSelect { expr, name } => {
                self.segments(expr, out)?;
                out.push((name.clone(), Vec::new()));
            }
            K::Parameterized { base, args } => {
                self.segments(base, out)?;
                let args = args.iter().map(|a| self.type_arg(a)).collect::<R<_>>()?;
                if let Some(last) = out.last_mut() {
                    last.1 = args;
                }
            }
            K::Annotated { base, .. } => self.segments(base, out)?,
            other => {
                return Err(format!(
                    "unsupported type {}",
                    Tree::new(0, 0, other.clone()).javac_kind()
                ));
            }
        }
        Ok(())
    }

    fn type_arg(&self, t: &Tree) -> R<TypeArg> {
        let (annotations, base) = match &t.kind {
            K::Annotated { annotations, base } if matches!(base.kind, K::Wildcard { .. }) => {
                (self.annotations(annotations)?, base.as_ref())
            }
            _ => (Vec::new(), t),
        };
        let K::Wildcard { bound } = &base.kind else {
            return Ok(TypeArg::Type(self.type_ref(t)?));
        };
        let (mut extends, mut super_) = (None, None);
        match bound {
            Some((true, ty)) => extends = Some(self.type_ref(ty)?),
            Some((false, ty)) => super_ = Some(self.type_ref(ty)?),
            None => {}
        }
        Ok(TypeArg::Wildcard {
            extends,
            super_,
            annotations,
        })
    }

    fn annotations(&self, trees: &[Tree]) -> R<Vec<Annotation>> {
        trees.iter().map(|t| self.annotation(t)).collect()
    }

    fn annotation(&self, t: &Tree) -> R<Annotation> {
        let K::Annotation { ty, args } = &t.kind else {
            return Err("expected annotation".into());
        };
        let args = args
            .iter()
            .map(|arg| match &arg.kind {
                K::Assign { op, lhs, rhs } if op == "=" && matches!(lhs.kind, K::Ident(_)) => {
                    let K::Ident(key) = &lhs.kind else {
                        unreachable!()
                    };
                    (key.clone(), self.element_value(rhs))
                }
                _ => ("value".to_string(), self.element_value(arg)),
            })
            .collect();
        Ok(Annotation {
            name: self.qualified_name(ty)?,
            args,
        })
    }

    fn element_value(&self, t: &Tree) -> Expr {
        self.expr(t).unwrap_or(Expr::Other)
    }

    /// A constant expression; `None` for anything the index does not evaluate (calls, `new`, lambdas, ...).
    fn expr(&self, t: &Tree) -> Option<Expr> {
        let text = &self.src[t.start..t.end];
        Some(match &t.kind {
            K::Literal(kind) => match kind {
                LitKind::String => Expr::Str(literal::string(text)),
                LitKind::Char => Expr::Char(literal::char(text)),
                LitKind::Boolean => Expr::Bool(text == "true"),
                LitKind::Null => Expr::Null,
                // jtk folds a minus sign into the literal, like javac
                LitKind::Int | LitKind::Long | LitKind::Float | LitKind::Double => {
                    match text.strip_prefix('-') {
                        Some(abs) => Expr::Unary("-", Box::new(Expr::Num(abs.replace('_', "")))),
                        None => Expr::Num(text.replace('_', "")),
                    }
                }
            },
            K::Ident(name) if name == "this" || name == "super" => return None,
            K::Ident(_) => Expr::Name(self.qualified_name(t).ok()?),
            K::MemberSelect { expr, name } if name == "class" => {
                Expr::ClassLit(self.type_ref(expr).ok()?)
            }
            K::MemberSelect { .. } => Expr::Name(self.qualified_name(t).ok()?),
            K::Parens(inner) => self.expr(inner)?,
            K::Binary { op, lhs, rhs } => {
                let op = BINARY_OPS.iter().find(|o| **o == op)?;
                Expr::Binary(Box::new(self.expr(lhs)?), op, Box::new(self.expr(rhs)?))
            }
            K::Unary {
                op,
                postfix: false,
                expr,
            } => {
                let op = ["-", "+", "!", "~"].into_iter().find(|o| o == op)?;
                Expr::Unary(op, Box::new(self.expr(expr)?))
            }
            K::Cast { ty, expr } => match &ty.kind {
                K::Primitive(name) if name != "void" => Expr::Cast(Box::new(self.expr(expr)?)),
                K::Ident(name) if name == "String" => Expr::Cast(Box::new(self.expr(expr)?)),
                _ => return None,
            },
            K::Conditional { cond, then, else_ } => Expr::Cond(
                Box::new(self.expr(cond)?),
                Box::new(self.expr(then)?),
                Box::new(self.expr(else_)?),
            ),
            K::Annotation { .. } => Expr::Annotation(Box::new(self.annotation(t).ok()?)),
            K::NewArray {
                elem: None,
                init: Some(items),
                ..
            } => Expr::Array(items.iter().map(|i| self.element_value(i)).collect()),
            _ => return None,
        })
    }

    /// A dotted name such as a package, an import or an annotation type.
    fn qualified_name(&self, t: &Tree) -> R<String> {
        match &t.kind {
            K::Ident(name) => Ok(name.clone()),
            K::MemberSelect { expr, name } => Ok(format!("{}.{name}", self.qualified_name(expr)?)),
            _ => Err(format!(
                "expected a name, found `{}`",
                &self.src[t.start..t.end]
            )),
        }
    }
}

/// Whether the annotations of an annotated type precede it, rather than a segment of a qualified name.
fn is_leading(t: &Tree, annotations: &[Tree]) -> bool {
    annotations.first().is_some_and(|a| a.start == t.start)
}

/// Adds declaration-position type annotations; for arrays they belong to the element type.
fn with_annotations(ty: TypeRef, added: Vec<Annotation>) -> TypeRef {
    if added.is_empty() {
        return ty;
    }
    match ty {
        TypeRef::Class {
            segments,
            mut annotations,
        } => {
            annotations.splice(0..0, added);
            TypeRef::Class {
                segments,
                annotations,
            }
        }
        TypeRef::Primitive {
            name,
            mut annotations,
        } => {
            annotations.splice(0..0, added);
            TypeRef::Primitive { name, annotations }
        }
        TypeRef::Array {
            component,
            annotations,
        } => TypeRef::Array {
            component: Box::new(with_annotations(*component, added)),
            annotations,
        },
        TypeRef::Void => TypeRef::Void,
    }
}
