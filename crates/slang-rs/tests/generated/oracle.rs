// SPDX-License-Identifier: Apache-2.0

//! What the typed design must answer for a model, derived from
//! SystemVerilog's meaning and the naming rule of
//! `doc/design/slang-in-process.md` ("Types"), not from either
//! implementation.
//!
//! An anonymous struct, union or enum is named `<scope>s$#<id>` (`u$`, `e$`),
//! where `<id>` is the model's id of the declaration; Slang's numeric id
//! takes its place in the design, and the test compares the two up to a
//! consistent renaming.

use num_bigint::BigInt;
use slang_rs::{Field, Port, PortDir, Range, Type, UnsupportedPort, Variant};

use crate::model::{Base, DataType, Design, Dim, Dir, EnumBase, Expr, IntKind, Module, TypeRef};

/// The ports of the instance at `path`, without locations; empty if the
/// model has no such instance.
pub fn ports(design: &Design, path: &str) -> Vec<Result<Port, UnsupportedPort>> {
    instances(design)
        .into_iter()
        .find(|(p, _)| p == path)
        .map(|(_, env)| {
            let module = &design.modules[env.module];
            module
                .ports
                .iter()
                .map(|port| {
                    let ty = Resolver { design, env: &env }.declared(
                        &port.ty,
                        &port.unpacked,
                        Scope::Module(env.module),
                    );
                    Ok(Port {
                        dir: match port.dir {
                            Dir::Input => PortDir::Input,
                            Dir::Output => PortDir::Output,
                            Dir::InOut => PortDir::InOut,
                        },
                        name: port.name.clone(),
                        ty,
                        loc: None,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The path of every instance: the top, then the instances below it, an
/// array instance once per element (`u[-1]`, `u[1][0]`).
pub fn paths(design: &Design) -> Vec<String> {
    instances(design)
        .into_iter()
        .map(|(path, _)| path)
        .collect()
}

/// The parameters of one instance: its module, the values of its value
/// parameters and the types of its type parameters.
struct Env {
    module: usize,
    params: Vec<i64>,
    type_params: Vec<Type>,
}

fn instances(design: &Design) -> Vec<(String, Env)> {
    let mut out = Vec::new();
    let top = env(design, 0, None);
    walk(design, design.modules[0].name.clone(), top, &mut out);
    out
}

/// The environment of an instance of `module`, with the overrides of
/// `instance` written in the instantiating module's environment `parent`.
fn env(design: &Design, module: usize, instance: Option<(&crate::model::Instance, &Env)>) -> Env {
    let def: &Module = &design.modules[module];
    let params: Vec<i64> = def
        .params
        .iter()
        .enumerate()
        .map(|(i, param)| {
            instance
                .and_then(|(inst, _)| inst.params.iter().find(|(j, _)| *j == i))
                .map_or(i64::from(param.default), |(_, v)| i64::from(*v))
        })
        .collect();
    let defaults = Env {
        module,
        params: params.clone(),
        type_params: vec![],
    };
    let type_params = def
        .type_params
        .iter()
        .enumerate()
        .map(|(i, param)| {
            match instance.and_then(|(inst, parent)| {
                inst.type_params
                    .iter()
                    .find(|(j, _)| *j == i)
                    .map(|(_, ty)| (ty, parent))
            }) {
                // An override is written, and named, where it is written.
                Some((ty, parent)) => Resolver {
                    design,
                    env: parent,
                }
                .declared(ty, &[], Scope::Module(parent.module)),
                None => Resolver {
                    design,
                    env: &defaults,
                }
                .declared(&param.default, &[], Scope::Module(module)),
            }
        })
        .collect();
    Env {
        module,
        params,
        type_params,
    }
}

fn walk(design: &Design, path: String, env: Env, out: &mut Vec<(String, Env)>) {
    let module = &design.modules[env.module];
    let mut children = Vec::new();
    for instance in &module.instances {
        let mut names = vec![instance.name.clone()];
        for dim in &instance.array {
            let (a, b) = match dim {
                Dim::Range(Expr::Const(a), Expr::Const(b)) => (*a, *b),
                Dim::Size(n) => (0, *n as i32 - 1),
                other => panic!("instance array dimension {other:?} is not constant"),
            };
            names = names
                .iter()
                .flat_map(|name| (a.min(b)..=a.max(b)).map(move |i| format!("{name}[{i}]")))
                .collect();
        }
        for name in names {
            let child = self::env(design, instance.module, Some((instance, &env)));
            children.push((format!("{path}.{name}"), child));
        }
    }
    out.push((path, env));
    for (path, child) in children {
        walk(design, path, child, out);
    }
}

/// Where a type is declared.
#[derive(Clone, Copy)]
enum Scope {
    Package(usize),
    Module(usize),
}

struct Resolver<'a> {
    design: &'a Design,
    env: &'a Env,
}

impl Resolver<'_> {
    /// The type of a declaration (port, typedef, member) of type `ty` with
    /// `unpacked` dimensions after its name, declared directly in `at`.
    fn declared(&self, ty: &DataType, unpacked: &[Dim], at: Scope) -> Type {
        let mut resolved = self.ty(ty, at, false);
        let mut dims: Vec<Range> = unpacked.iter().map(|d| self.dim(d)).collect();
        dims.append(unpacked_dims(&mut resolved));
        *unpacked_dims(&mut resolved) = dims;
        resolved
    }

    /// `ty` written in `at`; `nested` inside a struct or union.
    fn ty(&self, ty: &DataType, at: Scope, nested: bool) -> Type {
        let mut packed: Vec<Range> = ty.packed.iter().map(|d| self.dim(d)).collect();
        let mut resolved = match &ty.base {
            Base::Vector { signed, .. } => Type::Logic {
                signed: *signed,
                packed_dimensions: vec![],
                unpacked_dimensions: vec![],
            },
            Base::Integer { kind, signed } => {
                let (width, default_signed) = integer(*kind);
                Type::Logic {
                    signed: signed.unwrap_or(default_signed),
                    packed_dimensions: vec![Range {
                        msb: width - 1,
                        lsb: 0,
                    }],
                    unpacked_dimensions: vec![],
                }
            }
            Base::Aggregate(aggregate) => {
                let tag = if aggregate.union { "u$" } else { "s$" };
                let name = anonymous(self.design, at, nested, tag, aggregate.id);
                // Members are declared in the aggregate, nested in `at`.
                let fields = aggregate
                    .members
                    .iter()
                    .map(|member| {
                        let mut ty = self.ty(&member.ty, at, true);
                        let mut dims: Vec<Range> =
                            member.unpacked.iter().map(|d| self.dim(d)).collect();
                        dims.append(unpacked_dims(&mut ty));
                        *unpacked_dims(&mut ty) = dims;
                        Field {
                            name: member.name.clone(),
                            ty,
                        }
                    })
                    .collect();
                let signed = aggregate.packed && aggregate.signed;
                if aggregate.union {
                    Type::Union {
                        name,
                        signed,
                        packed: aggregate.packed,
                        fields,
                        packed_dimensions: vec![],
                        unpacked_dimensions: vec![],
                    }
                } else {
                    Type::Struct {
                        name,
                        signed,
                        packed: aggregate.packed,
                        fields,
                        packed_dimensions: vec![],
                        unpacked_dimensions: vec![],
                    }
                }
            }
            Base::Enum(e) => {
                let (width, signed) = match &e.base {
                    // The implicit base is `int`.
                    None => (32, true),
                    Some(EnumBase::Vector { signed, range, .. }) => {
                        (range.map_or(1, |(m, l)| (m - l).abs() + 1), *signed)
                    }
                    Some(EnumBase::Integer { kind, signed }) => {
                        let (width, default_signed) = integer(*kind);
                        (width, signed.unwrap_or(default_signed))
                    }
                };
                let mut previous: Option<i128> = None;
                let variants = e
                    .variants
                    .iter()
                    .map(|variant| {
                        // An omitted value is the previous one plus one,
                        // the first 0.
                        let value = variant.value.unwrap_or(previous.map_or(0, |p| p + 1));
                        previous = Some(value);
                        Variant {
                            name: variant.name.clone(),
                            width: width as usize,
                            value: BigInt::from(value),
                            signed,
                        }
                    })
                    .collect();
                Type::Enum {
                    name: anonymous(self.design, at, nested, "e$", e.id),
                    variants,
                    packed_dimensions: vec![],
                    unpacked_dimensions: vec![],
                }
            }
            Base::Named(target) => self.named(*target, at),
        };
        packed.append(packed_dims(&mut resolved));
        *packed_dims(&mut resolved) = packed;
        resolved
    }

    fn named(&self, target: TypeRef, at: Scope) -> Type {
        let (typedef, scope, prefix) = match (target, at) {
            (TypeRef::Package { package, typedef }, _) => {
                let package_def = &self.design.packages[package];
                (
                    &package_def.typedefs[typedef],
                    Scope::Package(package),
                    format!("{}::", package_def.name),
                )
            }
            (TypeRef::Local(i), Scope::Module(m)) => {
                let module = &self.design.modules[m];
                (
                    &module.typedefs[i],
                    Scope::Module(m),
                    format!("{}.", module.name),
                )
            }
            // A type parameter's alias names nothing.
            (TypeRef::TypeParam(i), Scope::Module(_)) => return self.env.type_params[i].clone(),
            (target, Scope::Package(_)) => panic!("{target:?} written in a package"),
        };
        // A package typedef is resolved where it is declared; it sees no
        // parameter.
        let package_env = Env {
            module: 0,
            params: vec![],
            type_params: vec![],
        };
        let env = match scope {
            Scope::Package(_) => &package_env,
            Scope::Module(_) => self.env,
        };
        let mut resolved = Resolver {
            design: self.design,
            env,
        }
        .declared(&typedef.ty, &typedef.unpacked, scope);
        // A typedef that reaches a struct, union or enum through aliases
        // only names it; one that adds a dimension names the array, which
        // has no name, and the element keeps its own.
        let undimensioned =
            packed_dims(&mut resolved).is_empty() && unpacked_dims(&mut resolved).is_empty();
        if undimensioned {
            if let Type::Struct { name, .. } | Type::Union { name, .. } | Type::Enum { name, .. } =
                &mut resolved
            {
                *name = format!("{prefix}{}", typedef.name);
            }
        }
        resolved
    }

    fn dim(&self, dim: &Dim) -> Range {
        match dim {
            Dim::Range(msb, lsb) => Range {
                msb: self.expr(*msb),
                lsb: self.expr(*lsb),
            },
            // `[n]` is `[0:n-1]`.
            Dim::Size(n) => Range {
                msb: 0,
                lsb: *n as i32 - 1,
            },
        }
    }

    fn expr(&self, expr: Expr) -> i32 {
        match expr {
            Expr::Const(value) => value,
            Expr::Param { param, offset } => (self.env.params[param] + i64::from(offset)) as i32,
        }
    }
}

/// The width and default signedness of an integer type.
fn integer(kind: IntKind) -> (i32, bool) {
    match kind {
        IntKind::Byte => (8, true),
        IntKind::ShortInt => (16, true),
        IntKind::Int => (32, true),
        IntKind::LongInt => (64, true),
        IntKind::Integer => (32, true),
    }
}

/// The name of an anonymous type declared in `at`: Slang's type printer
/// writes the lexical path of the type's parent scope, `::` after a package
/// and `.` otherwise. A type written in a struct or union has that
/// aggregate as its parent scope, whose lexical path, having no name of its
/// own, is that of the package or module, followed by `.`.
fn anonymous(design: &Design, at: Scope, nested: bool, tag: &str, id: usize) -> String {
    let (path, separator) = match at {
        Scope::Package(p) => (&design.packages[p].name, "::"),
        Scope::Module(m) => (&design.modules[m].name, "."),
    };
    let separator = if nested { "." } else { separator };
    format!("{path}{separator}{tag}#{id}")
}

fn packed_dims(ty: &mut Type) -> &mut Vec<Range> {
    match ty {
        Type::Logic {
            packed_dimensions, ..
        }
        | Type::Struct {
            packed_dimensions, ..
        }
        | Type::Union {
            packed_dimensions, ..
        }
        | Type::Enum {
            packed_dimensions, ..
        } => packed_dimensions,
    }
}

fn unpacked_dims(ty: &mut Type) -> &mut Vec<Range> {
    match ty {
        Type::Logic {
            unpacked_dimensions,
            ..
        }
        | Type::Struct {
            unpacked_dimensions,
            ..
        }
        | Type::Union {
            unpacked_dimensions,
            ..
        }
        | Type::Enum {
            unpacked_dimensions,
            ..
        } => unpacked_dimensions,
    }
}
