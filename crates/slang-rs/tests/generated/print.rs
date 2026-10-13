// SPDX-License-Identifier: Apache-2.0

//! Writes a model as SystemVerilog source.

use std::fmt::Write;

use crate::model::{
    Base, DataType, Design, Dim, Dir, EnumBase, Expr, IntKind, Module, PackageValue, PortDecl,
    TypeRef, VectorKw, enum_base_width,
};

/// The source of `design`: its packages, then its modules.
pub fn sv(design: &Design) -> String {
    let mut out = String::new();
    for (p, package) in design.packages.iter().enumerate() {
        let at = Scope {
            design,
            package: Some(p),
            module: None,
        };
        writeln!(out, "package {};", package.name).unwrap();
        for param in &package.params {
            let value = match param.value {
                PackageValue::Const(value) => value.to_string(),
                PackageValue::Sum {
                    package,
                    param,
                    offset,
                } => {
                    let package = &design.packages[package];
                    format!(
                        "{}::{} + ({offset})",
                        package.name, package.params[param].name
                    )
                }
            };
            writeln!(out, "  localparam int {} = {value};", param.name).unwrap();
        }
        for typedef in &package.typedefs {
            writeln!(
                out,
                "  typedef {} {}{};",
                at.ty(&typedef.ty),
                typedef.name,
                at.dims(&typedef.unpacked)
            )
            .unwrap();
        }
        writeln!(out, "endpackage").unwrap();
    }
    for module in &design.modules {
        module_sv(design, module, &mut out);
    }
    out
}

fn module_sv(design: &Design, module: &Module, out: &mut String) {
    let at = Scope {
        design,
        package: None,
        module: Some(module),
    };
    write!(out, "module {}", module.name).unwrap();
    let mut header = Vec::new();
    for param in &module.params {
        header.push(format!("parameter int {} = {}", param.name, param.default));
    }
    for param in &module.type_params {
        header.push(format!(
            "parameter type {} = {}",
            param.name,
            at.ty(&param.default)
        ));
    }
    for param in &module.typed_params {
        let ty = &module.type_params[param.type_param].name;
        header.push(format!("parameter {ty} {} = {ty}'('0)", param.name));
    }
    if !header.is_empty() {
        write!(out, " #({})", header.join(", ")).unwrap();
    }
    let ports: Vec<String> = if module.ansi {
        module.ports.iter().map(|port| at.port(port)).collect()
    } else {
        module.ports.iter().map(|port| port.name.clone()).collect()
    };
    if !ports.is_empty() {
        write!(out, " ({})", ports.join(", ")).unwrap();
    }
    writeln!(out, ";").unwrap();
    for local in &module.local_params {
        writeln!(
            out,
            "  localparam logic [{}-1:0] {} = '0;",
            module.params[local.param].name, local.name
        )
        .unwrap();
    }
    for local in &module.local_type_params {
        writeln!(
            out,
            "  localparam type {} = {};",
            local.name, module.type_params[local.type_param].name
        )
        .unwrap();
    }
    for typedef in &module.typedefs {
        writeln!(
            out,
            "  typedef {} {}{};",
            at.ty(&typedef.ty),
            typedef.name,
            at.dims(&typedef.unpacked)
        )
        .unwrap();
    }
    if !module.ansi {
        for port in &module.ports {
            writeln!(out, "  {};", at.port(port)).unwrap();
        }
    }
    for instance in &module.instances {
        let child = &design.modules[instance.module];
        let end = match &instance.generate {
            Some(generate) => {
                let (relation, step, bound) = if generate.descending {
                    (">", "--", generate.start - generate.count as i32)
                } else {
                    ("<", "++", generate.start + generate.count as i32)
                };
                writeln!(
                    out,
                    "  for (genvar i = {}; i {relation} {bound}; i{step}) begin : {}",
                    generate.start, generate.name
                )
                .unwrap();
                "\n  end"
            }
            None => "",
        };
        write!(out, "  {}", child.name).unwrap();
        let mut overrides = Vec::new();
        for (i, value) in &instance.params {
            overrides.push(format!(".{}({})", child.params[*i].name, value));
        }
        for (i, ty) in &instance.type_params {
            overrides.push(format!(".{}({})", child.type_params[*i].name, at.ty(ty)));
        }
        if !overrides.is_empty() {
            write!(out, " #({})", overrides.join(", ")).unwrap();
        }
        let connections: Vec<String> = child
            .ports
            .iter()
            .map(|port| format!(".{}()", port.name))
            .collect();
        writeln!(
            out,
            " {}{} ({});{end}",
            instance.name,
            at.dims(&instance.array),
            connections.join(", ")
        )
        .unwrap();
        if let Some(element) = &instance.element_param {
            let indices: String = element.element.iter().map(|i| format!("[{i}]")).collect();
            writeln!(
                out,
                "  defparam {}{indices}.{} = {};",
                instance.name, child.params[element.param].name, element.value
            )
            .unwrap();
        }
    }
    writeln!(out, "endmodule").unwrap();
}

/// Where a type is written: in a package or in a module.
struct Scope<'a> {
    design: &'a Design,
    package: Option<usize>,
    module: Option<&'a Module>,
}

impl Scope<'_> {
    fn port(&self, port: &PortDecl) -> String {
        let dir = match port.dir {
            Dir::Input => "input",
            Dir::Output => "output",
            // Without `wire`, a non-ANSI port with a data type is a
            // variable, which cannot be `inout`.
            Dir::InOut => "inout wire",
        };
        format!(
            "{dir} {} {}{}",
            self.ty(&port.ty),
            port.name,
            self.dims(&port.unpacked)
        )
    }

    fn ty(&self, ty: &DataType) -> String {
        let packed = self.dims(&ty.packed);
        let base = match &ty.base {
            Base::Vector { kw, signed } => {
                format!("{}{}", vector_kw(*kw), if *signed { " signed" } else { "" })
            }
            Base::Integer { kind, signed } => integer(*kind, *signed),
            Base::Aggregate(aggregate) => {
                let mut text = String::from(if aggregate.union { "union" } else { "struct" });
                if aggregate.packed {
                    text.push_str(" packed");
                }
                if aggregate.signed {
                    text.push_str(" signed");
                }
                text.push_str(" {");
                for member in &aggregate.members {
                    write!(
                        text,
                        " {} {}{};",
                        self.ty(&member.ty),
                        member.name,
                        self.dims(&member.unpacked)
                    )
                    .unwrap();
                }
                text.push_str(" }");
                text
            }
            Base::Enum(e) => {
                let mut text = String::from("enum");
                match &e.base {
                    None => {}
                    Some(EnumBase::Vector { kw, signed, range }) => {
                        write!(text, " {}", vector_kw(*kw)).unwrap();
                        if *signed {
                            text.push_str(" signed");
                        }
                        if let Some((msb, lsb)) = range {
                            write!(text, " [{msb}:{lsb}]").unwrap();
                        }
                    }
                    Some(EnumBase::Integer { kind, signed }) => {
                        write!(text, " {}", integer(*kind, *signed)).unwrap();
                    }
                }
                let (width, signed) = enum_base_width(e.base.as_ref());
                let variants: Vec<String> = e
                    .variants
                    .iter()
                    .map(|v| match v.value {
                        None => v.name.clone(),
                        // A sized literal must have the base type's width.
                        Some(value) if e.base.is_none() => format!("{} = {value}", v.name),
                        Some(value) => {
                            // Slang counts a sign bit even for `1'sd0`.
                            let s = if signed && value != 0 { "s" } else { "" };
                            let minus = if value < 0 { "-" } else { "" };
                            format!("{} = {minus}{width}'{s}d{}", v.name, value.unsigned_abs())
                        }
                    })
                    .collect();
                write!(text, " {{ {} }}", variants.join(", ")).unwrap();
                text
            }
            Base::Named(target) => self.name(*target),
        };
        format!("{base}{packed}")
    }

    fn name(&self, target: TypeRef) -> String {
        match target {
            TypeRef::Package { package, typedef } => {
                let package_def = &self.design.packages[package];
                let name = &package_def.typedefs[typedef].name;
                if self.package == Some(package) {
                    name.clone()
                } else {
                    format!("{}::{name}", package_def.name)
                }
            }
            TypeRef::Local(i) => self.module.expect("a module").typedefs[i].name.clone(),
            TypeRef::TypeParam(i) => self.module.expect("a module").type_params[i].name.clone(),
        }
    }

    fn dims(&self, dims: &[Dim]) -> String {
        let mut text = String::new();
        for dim in dims {
            match dim {
                Dim::Range(msb, lsb) => {
                    write!(text, "[{}:{}]", self.expr(*msb), self.expr(*lsb)).unwrap()
                }
                Dim::Size(size) => write!(text, "[{size}]").unwrap(),
            }
        }
        if text.is_empty() {
            text
        } else {
            format!(" {text}")
        }
    }

    fn expr(&self, expr: Expr) -> String {
        match expr {
            Expr::Const(value) => value.to_string(),
            Expr::Param { param, offset } => {
                let name = &self.module.expect("a module").params[param].name;
                match offset {
                    0 => name.clone(),
                    o if o > 0 => format!("{name}+{o}"),
                    o => format!("{name}{o}"),
                }
            }
        }
    }
}

fn vector_kw(kw: VectorKw) -> &'static str {
    match kw {
        VectorKw::Logic => "logic",
        VectorKw::Reg => "reg",
        VectorKw::Bit => "bit",
    }
}

fn integer(kind: IntKind, signed: Option<bool>) -> String {
    match signed {
        None => kind.keyword().to_string(),
        Some(true) => format!("{} signed", kind.keyword()),
        Some(false) => format!("{} unsigned", kind.keyword()),
    }
}
