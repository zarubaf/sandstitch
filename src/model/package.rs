// SPDX-License-Identifier: Apache-2.0

//! SystemVerilog packages built in Rust: typed members lowered to
//! xlsynth-vast, written under a content-hashed name, and handed out as
//! [`ParameterValue`] handles. See `doc/design/packages.md`.

use crate::ParameterValue;
use crate::import::specialize::{IDENTIFIER as IDENT, NAME};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use xlsynth_vast::{DataKind, Expr, LiteralFormat, VastDataType, VastFile, VastFileType};

/// The type of a package localparam or struct field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackageType {
    /// `int unsigned`.
    IntUnsigned,
    /// `logic [w-1:0]`; width 1 is `logic [0:0]`.
    Logic(usize),
    /// A type named `ident` or `pkg::ident`.
    Named(String),
}

/// The value of a package localparam.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackageValue {
    /// An unsigned integer.
    Int(u32),
    /// A constant named `ident` or `pkg::ident`.
    Name(String),
    /// An assignment pattern of named fields, `'{Name: value, ...}`.
    Fields(Vec<(String, PackageValue)>),
}

#[derive(Clone, Debug)]
enum Member {
    Localparam {
        name: String,
        ty: PackageType,
        value: PackageValue,
    },
    TypedefLogic {
        name: String,
        width: usize,
    },
    TypedefStruct {
        name: String,
        fields: Vec<(String, PackageType)>,
    },
    TypedefEnum {
        name: String,
        width: usize,
        variants: Vec<(String, u32)>,
    },
    Macro {
        name: String,
        args: Vec<String>,
    },
}

/// A SystemVerilog package under construction.
///
/// Members are emitted in the order they were added. The builder methods
/// panic, naming the package and the member, on input the spec rejects.
#[derive(Clone, Debug)]
pub struct PackageDef {
    name: String,
    includes: Vec<String>,
    members: Vec<Member>,
    types: HashSet<String>,
    constants: HashSet<String>,
}

impl PackageDef {
    /// Starts an empty package. Panics if `name` is not an identifier.
    pub fn new(name: &str) -> PackageDef {
        assert!(
            IDENT.is_match(name),
            "package `{name}`: the package name is not an identifier"
        );
        PackageDef {
            name: name.to_string(),
            includes: Vec::new(),
            members: Vec::new(),
            types: HashSet::new(),
            constants: HashSet::new(),
        }
    }

    #[track_caller]
    fn fail(&self, member: &str, reason: &str) -> ! {
        panic!("package `{}`, member `{member}`: {reason}", self.name)
    }

    fn ident(&self, member: &str, what: &str, text: &str) {
        if !IDENT.is_match(text) {
            self.fail(member, &format!("{what} `{text}` is not an identifier"));
        }
    }

    fn declare(&mut self, name: &str, is_type: bool) {
        self.ident(name, "name", name);
        if self.types.contains(name) || self.constants.contains(name) {
            self.fail(name, "declared twice in the package");
        }
        if is_type {
            self.types.insert(name.to_string());
        } else {
            self.constants.insert(name.to_string());
        }
    }

    fn check_width(&self, member: &str, width: usize) {
        if width == 0 {
            self.fail(member, "the width is 0");
        }
    }

    fn check_type(&self, member: &str, ty: &PackageType) {
        match ty {
            PackageType::IntUnsigned => {}
            PackageType::Logic(width) => self.check_width(member, *width),
            PackageType::Named(name) => {
                if !NAME.is_match(name) {
                    self.fail(
                        member,
                        &format!("type `{name}` is not `ident` or `pkg::ident`"),
                    );
                }
            }
        }
    }

    fn check_value(&self, member: &str, ty: Option<&PackageType>, value: &PackageValue) {
        match value {
            PackageValue::Int(v) => {
                if let Some(PackageType::Logic(w)) = ty
                    && *w < 32
                    && u64::from(*v) >= 1u64 << *w
                {
                    self.fail(
                        member,
                        &format!("value {v} does not fit in logic [{}:0]", w - 1),
                    );
                }
            }
            PackageValue::Name(name) => {
                if !NAME.is_match(name) {
                    self.fail(
                        member,
                        &format!("value `{name}` is not `ident` or `pkg::ident`"),
                    );
                }
            }
            PackageValue::Fields(fields) => {
                if matches!(ty, Some(PackageType::IntUnsigned | PackageType::Logic(_))) {
                    self.fail(member, "named fields need a named type");
                }
                if fields.is_empty() {
                    self.fail(member, "the assignment pattern has no fields");
                }
                let mut seen = HashSet::new();
                for (field, v) in fields {
                    self.ident(member, "field", field);
                    if !seen.insert(field.as_str()) {
                        self.fail(member, &format!("field `{field}` is declared twice"));
                    }
                    // The field's own type is not known here.
                    self.check_value(member, None, v);
                }
            }
        }
    }

    /// Adds `` `include "path" `` before the package.
    pub fn include(&mut self, path: &str) -> &mut Self {
        if path.is_empty() || path.contains(['"', '\n', '\r']) {
            self.fail(
                &format!("include {path:?}"),
                "the include path is empty or contains a quote or line break",
            );
        }
        self.includes.push(path.to_string());
        self
    }

    /// Adds `localparam <ty> name = value;`.
    pub fn localparam(&mut self, name: &str, ty: PackageType, value: PackageValue) -> &mut Self {
        self.declare(name, false);
        self.check_type(name, &ty);
        self.check_value(name, Some(&ty), &value);
        self.members.push(Member::Localparam {
            name: name.to_string(),
            ty,
            value,
        });
        self
    }

    /// Adds `typedef logic [width-1:0] name;`.
    pub fn typedef_logic(&mut self, name: &str, width: usize) -> &mut Self {
        self.declare(name, true);
        self.check_width(name, width);
        self.members.push(Member::TypedefLogic {
            name: name.to_string(),
            width,
        });
        self
    }

    /// Adds a packed struct typedef; `fields` list the MSB first.
    pub fn typedef_struct(&mut self, name: &str, fields: &[(&str, PackageType)]) -> &mut Self {
        self.declare(name, true);
        if fields.is_empty() {
            self.fail(name, "the struct has no fields");
        }
        let mut seen = HashSet::new();
        for (field, ty) in fields {
            self.ident(name, "field", field);
            if !seen.insert(*field) {
                self.fail(name, &format!("field `{field}` is declared twice"));
            }
            self.check_type(name, ty);
        }
        self.members.push(Member::TypedefStruct {
            name: name.to_string(),
            fields: fields
                .iter()
                .map(|(f, t)| (f.to_string(), t.clone()))
                .collect(),
        });
        self
    }

    /// Adds `typedef enum logic [width-1:0] {...} name;`. Variants are
    /// constants of the package.
    pub fn typedef_enum(
        &mut self,
        name: &str,
        width: usize,
        variants: &[(&str, u32)],
    ) -> &mut Self {
        self.declare(name, true);
        self.check_width(name, width);
        if variants.is_empty() {
            self.fail(name, "the enum has no variants");
        }
        let mut values = HashSet::new();
        for (variant, value) in variants {
            self.ident(name, "variant", variant);
            if width < 32 && u64::from(*value) >= 1u64 << width {
                self.fail(
                    name,
                    &format!("variant `{variant}` = {value} does not fit in {width} bits"),
                );
            }
            if !values.insert(*value) {
                self.fail(
                    name,
                    &format!("variant `{variant}` repeats the value {value}"),
                );
            }
            self.declare(variant, false);
        }
        self.members.push(Member::TypedefEnum {
            name: name.to_string(),
            width,
            variants: variants.iter().map(|(v, x)| (v.to_string(), *x)).collect(),
        });
        self
    }

    /// Adds an invocation `` `name(args...) `` of a macro at package scope.
    /// `declares` lists the type names the macro defines.
    pub fn macro_call(&mut self, name: &str, args: &[&str], declares: &[&str]) -> &mut Self {
        self.ident(name, "macro name", name);
        for arg in args {
            let integer = !arg.is_empty() && arg.bytes().all(|b| b.is_ascii_digit());
            let valid = if integer {
                (*arg == "0" || !arg.starts_with('0')) && arg.parse::<u32>().is_ok()
            } else {
                NAME.is_match(arg)
            };
            if !valid {
                self.fail(
                    name,
                    &format!("macro argument `{arg}` is neither a name nor an unsigned integer"),
                );
            }
        }
        for declared in declares {
            self.declare(declared, true);
        }
        self.members.push(Member::Macro {
            name: name.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
        });
        self
    }

    /// The handle for the type `<package>::<name>`. Typedefs and names a
    /// macro declares are types. Panics if the package declares no such type.
    #[track_caller]
    pub fn ty(&self, name: &str) -> ParameterValue {
        if !self.types.contains(name) {
            self.fail(name, "the package declares no such type");
        }
        ParameterValue::Type(format!("{}::{name}", self.name))
    }

    /// The handle for the constant `<package>::<name>`. Localparams and enum
    /// variants are constants. Panics if the package declares no such
    /// constant.
    #[track_caller]
    pub fn constant(&self, name: &str) -> ParameterValue {
        if !self.constants.contains(name) {
            self.fail(name, "the package declares no such constant");
        }
        ParameterValue::Expr(format!("{}::{name}", self.name))
    }

    /// The package as SystemVerilog text. Deterministic.
    pub fn emit(&self) -> String {
        let mut file = VastFile::new(VastFileType::SystemVerilog);
        for include in &self.includes {
            file.add_include(include);
        }
        let blank = file.make_blank_line();
        file.add_blank_line(blank);
        let package = file.add_package(&self.name);
        for member in &self.members {
            match member {
                Member::Localparam { name, ty, value } => {
                    let (data_type, kind) = lower_type(&mut file, ty);
                    let def = file.make_def(name, kind, &data_type);
                    let rhs = lower_value(&mut file, value);
                    file.add_package_localparam(package, &def, &rhs);
                }
                Member::TypedefLogic { name, width } => {
                    let ty = logic_vector(&mut file, *width);
                    file.add_package_typedef(package, name, &ty);
                }
                Member::TypedefStruct { name, fields } => {
                    let fields: Vec<(&str, VastDataType)> = fields
                        .iter()
                        .map(|(f, t)| (f.as_str(), lower_type(&mut file, t).0))
                        .collect();
                    file.add_package_struct_typedef(package, name, &fields);
                }
                Member::TypedefEnum {
                    name,
                    width,
                    variants,
                } => {
                    let base = logic_vector(&mut file, *width);
                    let variants: Vec<(&str, Expr)> = variants
                        .iter()
                        .map(|(v, x)| (v.as_str(), int_literal(&mut file, *x)))
                        .collect();
                    file.add_package_enum_typedef(package, name, &base, &variants);
                }
                Member::Macro { name, args } => {
                    let args: Vec<Expr> = args
                        .iter()
                        .map(|a| match a.parse::<u32>() {
                            Ok(n) => int_literal(&mut file, n),
                            Err(_) => name_expr(&mut file, a),
                        })
                        .collect();
                    let macro_ref = if args.is_empty() {
                        file.make_macro_ref(name)
                    } else {
                        file.make_macro_ref_with_args(name, &args.iter().collect::<Vec<_>>())
                    };
                    let statement = file.make_macro_statement(&macro_ref, false);
                    file.add_package_macro(package, statement);
                }
            }
        }
        format!("// Generated by sandstitch. Do not edit.\n{}", file.emit())
    }

    /// Writes the package to `dir/<name>-<hash>.sv`, where the hash is the
    /// first 12 hex digits of the SHA-256 of [`Self::emit`], and returns the
    /// path. The write is atomic (temp file, then rename); equal packages
    /// share one file. `dir` must outlive every use of the path.
    pub fn write(&self, dir: &Path) -> std::io::Result<PathBuf> {
        let text = self.emit();
        let digest = Sha256::digest(text.as_bytes());
        let hash: String = digest[..6].iter().map(|b| format!("{b:02x}")).collect();
        let path = dir.join(format!("{}-{hash}.sv", self.name));
        if std::fs::read_to_string(&path).is_ok_and(|existing| existing == text) {
            return Ok(path);
        }
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(
            ".{}-{hash}.{}.{:?}.tmp",
            self.name,
            std::process::id(),
            std::thread::current().id()
        ));
        let result = std::fs::write(&tmp, &text).and_then(|()| std::fs::rename(&tmp, &path));
        if let Err(e) = result {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        Ok(path)
    }
}

fn logic_vector(file: &mut VastFile, width: usize) -> VastDataType {
    if width == 1 {
        let scalar = file.make_scalar_type();
        file.make_packed_array_type_ranges(scalar, &[(0, 0)])
    } else {
        file.make_bit_vector_type(width as i64, false)
    }
}

fn lower_type(file: &mut VastFile, ty: &PackageType) -> (VastDataType, DataKind) {
    match ty {
        PackageType::IntUnsigned => (file.make_int_type(false), DataKind::Int),
        PackageType::Logic(width) => (logic_vector(file, *width), DataKind::Logic),
        PackageType::Named(name) => match name.split_once("::") {
            Some((pkg, ident)) => (file.make_extern_package_type(pkg, ident), DataKind::User),
            None => (file.make_extern_type(name), DataKind::User),
        },
    }
}

fn int_literal(file: &mut VastFile, value: u32) -> Expr {
    match i32::try_from(value) {
        Ok(v) => file.make_unsized_decimal_literal(v),
        Err(_) => file
            .make_literal(
                &format!("bits[32]:{value}"),
                &LiteralFormat::UnsignedDecimal,
            )
            .expect("a u32 fits a 32-bit literal"),
    }
}

fn name_expr(file: &mut VastFile, name: &str) -> Expr {
    match name.split_once("::") {
        Some((pkg, ident)) => file.make_package_ref(pkg, ident),
        None => file.make_name_ref(name),
    }
}

fn lower_value(file: &mut VastFile, value: &PackageValue) -> Expr {
    match value {
        PackageValue::Int(v) => int_literal(file, *v),
        PackageValue::Name(name) => name_expr(file, name),
        PackageValue::Fields(fields) => {
            let lowered: Vec<(&str, Expr)> = fields
                .iter()
                .map(|(f, v)| (f.as_str(), lower_value(file, v)))
                .collect();
            file.make_named_assignment_pattern(&lowered)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;

    fn spec_example() -> PackageDef {
        let mut p = PackageDef::new("cfg_pkg");
        p.include("ext/typedef.svh")
            .localparam(
                "Cfg",
                PackageType::Named("ext_pkg::cfg_t".into()),
                PackageValue::Fields(vec![
                    ("Width".into(), PackageValue::Int(48)),
                    ("Depth".into(), PackageValue::Int(4)),
                ]),
            )
            .localparam("Count", PackageType::IntUnsigned, PackageValue::Int(1))
            .typedef_logic("x_bits_t", 2)
            .typedef_struct(
                "xy_t",
                &[("x", PackageType::Logic(2)), ("y", PackageType::Logic(2))],
            )
            .typedef_enum("dir_e", 2, &[("A", 0), ("B", 1)])
            .localparam(
                "Origin",
                PackageType::Named("xy_t".into()),
                PackageValue::Fields(vec![
                    ("x".into(), PackageValue::Int(1)),
                    ("y".into(), PackageValue::Int(0)),
                ]),
            )
            .localparam(
                "Start",
                PackageType::Named("dir_e".into()),
                PackageValue::Name("B".into()),
            )
            .macro_call(
                "TYPEDEF_HDR_T",
                &["hdr_t", "xy_t", "logic", "2"],
                &["hdr_t"],
            );
        p
    }

    #[test]
    fn emits_every_member_kind() {
        let expected = concat!(
            "// Generated by sandstitch. Do not edit.\n",
            "`include \"ext/typedef.svh\"\n",
            "\n",
            "package cfg_pkg;\n",
            "  localparam ext_pkg::cfg_t Cfg = '{Width: 48, Depth: 4};\n",
            "  localparam int unsigned Count = 1;\n",
            "  typedef logic [1:0] x_bits_t;\n",
            "  typedef struct packed {\n",
            "    logic [1:0] x;\n",
            "    logic [1:0] y;\n",
            "  } xy_t;\n",
            "  typedef enum logic [1:0] {\n",
            "    A = 0,\n",
            "    B = 1\n",
            "  } dir_e;\n",
            "  localparam xy_t Origin = '{x: 1, y: 0};\n",
            "  localparam dir_e Start = B;\n",
            "  `TYPEDEF_HDR_T(hdr_t, xy_t, logic, 2)\n",
            "endpackage\n",
        );
        assert_eq!(spec_example().emit(), expected);
    }

    #[test]
    fn large_integers_and_one_bit_types() {
        let mut p = PackageDef::new("p");
        p.localparam("Big", PackageType::IntUnsigned, PackageValue::Int(u32::MAX))
            .localparam(
                "Max",
                PackageType::IntUnsigned,
                PackageValue::Int(i32::MAX as u32),
            )
            .typedef_logic("b_t", 1);
        let text = p.emit();
        assert!(text.contains("Big = 32'd4294967295;"), "{text}");
        assert!(text.contains("Max = 2147483647;"), "{text}");
        assert!(text.contains("typedef logic [0:0] b_t;"), "{text}");
    }

    #[test]
    fn macro_declared_names_are_types() {
        let p = spec_example();
        assert_eq!(p.ty("hdr_t"), ParameterValue::Type("cfg_pkg::hdr_t".into()));
        assert_eq!(p.ty("xy_t"), ParameterValue::Type("cfg_pkg::xy_t".into()));
    }

    #[test]
    fn constants_include_localparams_and_variants() {
        let p = spec_example();
        assert_eq!(
            p.constant("Count"),
            ParameterValue::Expr("cfg_pkg::Count".into())
        );
        assert_eq!(p.constant("B"), ParameterValue::Expr("cfg_pkg::B".into()));
    }

    #[test]
    #[should_panic(
        expected = "package `cfg_pkg`, member `xy_t`: the package declares no such constant"
    )]
    fn constant_rejects_a_type() {
        spec_example().constant("xy_t");
    }

    #[test]
    #[should_panic(
        expected = "package `cfg_pkg`, member `Count`: the package declares no such type"
    )]
    fn ty_rejects_a_constant() {
        spec_example().ty("Count");
    }

    #[test]
    fn write_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let p = spec_example();
        let a = p.write(dir.path()).unwrap();
        let b = p.write(dir.path()).unwrap();
        assert_eq!(a, b);
        assert_eq!(std::fs::read_to_string(&a).unwrap(), p.emit());
        let name = a.file_name().unwrap().to_str().unwrap();
        assert!(
            Regex::new(r"^cfg_pkg-[0-9a-f]{12}\.sv$")
                .unwrap()
                .is_match(name),
            "{name}"
        );
        // A different package gets a different file.
        let mut q = spec_example();
        q.typedef_logic("extra_t", 3);
        assert_ne!(q.write(dir.path()).unwrap(), a);
    }

    #[test]
    #[should_panic(expected = "package `1bad`: the package name is not an identifier")]
    fn rejects_bad_package_name() {
        PackageDef::new("1bad");
    }

    #[test]
    #[should_panic(expected = "package `p`, member `a b`: name `a b` is not an identifier")]
    fn rejects_bad_member_name() {
        PackageDef::new("p").typedef_logic("a b", 1);
    }

    #[test]
    #[should_panic(expected = "package `p`, member `f`: field `a-b` is not an identifier")]
    fn rejects_bad_field_name() {
        PackageDef::new("p").typedef_struct("f", &[("a-b", PackageType::Logic(1))]);
    }

    #[test]
    #[should_panic(expected = "package `p`, member `e`: variant `1x` is not an identifier")]
    fn rejects_bad_variant_name() {
        PackageDef::new("p").typedef_enum("e", 2, &[("1x", 0)]);
    }

    #[test]
    #[should_panic(expected = "package `p`, member `m x`: macro name `m x` is not an identifier")]
    fn rejects_bad_macro_name() {
        PackageDef::new("p").macro_call("m x", &[], &[]);
    }

    #[test]
    #[should_panic(expected = "package `p`, member `t`: declared twice in the package")]
    fn rejects_duplicate_typedef() {
        PackageDef::new("p")
            .typedef_logic("t", 1)
            .typedef_logic("t", 2);
    }

    #[test]
    #[should_panic(expected = "package `p`, member `A`: declared twice in the package")]
    fn rejects_variant_clashing_with_localparam() {
        PackageDef::new("p")
            .localparam("A", PackageType::IntUnsigned, PackageValue::Int(0))
            .typedef_enum("e", 2, &[("A", 0)]);
    }

    #[test]
    #[should_panic(expected = "package `p`, member `h`: declared twice in the package")]
    fn rejects_macro_declared_clash() {
        PackageDef::new("p")
            .typedef_logic("h", 1)
            .macro_call("M", &["h"], &["h"]);
    }

    #[test]
    #[should_panic(expected = "package `p`, member `s`: field `x` is declared twice")]
    fn rejects_duplicate_field() {
        PackageDef::new("p").typedef_struct(
            "s",
            &[("x", PackageType::Logic(1)), ("x", PackageType::Logic(2))],
        );
    }

    #[test]
    #[should_panic(expected = "package `p`, member `s`: the struct has no fields")]
    fn rejects_empty_struct() {
        PackageDef::new("p").typedef_struct("s", &[]);
    }

    #[test]
    #[should_panic(expected = "package `p`, member `e`: the enum has no variants")]
    fn rejects_empty_enum() {
        PackageDef::new("p").typedef_enum("e", 2, &[]);
    }

    #[test]
    #[should_panic(expected = "package `p`, member `t`: the width is 0")]
    fn rejects_zero_typedef_width() {
        PackageDef::new("p").typedef_logic("t", 0);
    }

    #[test]
    #[should_panic(expected = "package `p`, member `e`: the width is 0")]
    fn rejects_zero_enum_width() {
        PackageDef::new("p").typedef_enum("e", 0, &[("A", 0)]);
    }

    #[test]
    #[should_panic(expected = "package `p`, member `s`: the width is 0")]
    fn rejects_zero_field_width() {
        PackageDef::new("p").typedef_struct("s", &[("x", PackageType::Logic(0))]);
    }

    #[test]
    #[should_panic(expected = "package `p`, member `e`: variant `B` = 4 does not fit in 2 bits")]
    fn rejects_oversized_variant() {
        PackageDef::new("p").typedef_enum("e", 2, &[("A", 0), ("B", 4)]);
    }

    #[test]
    #[should_panic(expected = "package `p`, member `e`: variant `B` repeats the value 1")]
    fn rejects_repeated_variant_value() {
        PackageDef::new("p").typedef_enum("e", 2, &[("A", 1), ("B", 1)]);
    }

    #[test]
    #[should_panic(expected = "package `p`, member `L`: value 4 does not fit in logic [1:0]")]
    fn rejects_oversized_int() {
        PackageDef::new("p").localparam("L", PackageType::Logic(2), PackageValue::Int(4));
    }

    #[test]
    #[should_panic(expected = "package `p`, member `include \"\"`: the include path is empty")]
    fn rejects_empty_include() {
        PackageDef::new("p").include("");
    }

    #[test]
    #[should_panic(
        expected = "package `p`, member `include \"a\\\"b\"`: the include path is empty or contains"
    )]
    fn rejects_quote_in_include() {
        PackageDef::new("p").include("a\"b");
    }

    #[test]
    #[should_panic(
        expected = "package `p`, member `include \"a\\nb\"`: the include path is empty or contains"
    )]
    fn rejects_newline_in_include() {
        PackageDef::new("p").include("a\nb");
    }

    #[test]
    #[should_panic(
        expected = "package `p`, member `M`: macro argument `-1` is neither a name nor an unsigned integer"
    )]
    fn rejects_bad_macro_argument() {
        PackageDef::new("p").macro_call("M", &["-1"], &[]);
    }

    #[test]
    #[should_panic(expected = "package `p`, member `M`: macro argument `4294967296` is neither")]
    fn rejects_oversized_macro_integer() {
        PackageDef::new("p").macro_call("M", &["4294967296"], &[]);
    }

    #[test]
    #[should_panic(expected = "package `p`, member `M`: macro argument `007` is neither")]
    fn rejects_leading_zero_macro_integer() {
        PackageDef::new("p").macro_call("M", &["007"], &[]);
    }

    #[test]
    fn macro_without_arguments_has_no_parentheses() {
        let mut p = PackageDef::new("p");
        p.macro_call("M", &[], &[]);
        assert!(p.emit().contains("  `M\nendpackage"), "{}", p.emit());
    }

    #[test]
    fn macro_accepts_scoped_name_and_zero() {
        let mut p = PackageDef::new("p");
        p.macro_call("M", &["ext_pkg::ch_e", "0"], &[]);
        assert!(
            p.emit().contains("  `M(ext_pkg::ch_e, 0)\n"),
            "{}",
            p.emit()
        );
    }

    #[test]
    #[should_panic(
        expected = "package `p`, member `v`: type `a::b::c` is not `ident` or `pkg::ident`"
    )]
    fn rejects_bad_named_type() {
        PackageDef::new("p").localparam(
            "v",
            PackageType::Named("a::b::c".into()),
            PackageValue::Int(0),
        );
    }

    #[test]
    #[should_panic(expected = "package `p`, member `v`: value `1x` is not `ident` or `pkg::ident`")]
    fn rejects_bad_value_name() {
        PackageDef::new("p").localparam(
            "v",
            PackageType::Named("t".into()),
            PackageValue::Name("1x".into()),
        );
    }

    #[test]
    #[should_panic(expected = "package `p`, member `v`: field `a b` is not an identifier")]
    fn rejects_bad_pattern_field_name() {
        PackageDef::new("p").localparam(
            "v",
            PackageType::Named("t".into()),
            PackageValue::Fields(vec![("a b".into(), PackageValue::Int(0))]),
        );
    }

    #[test]
    #[should_panic(expected = "package `p`, member `v`: the assignment pattern has no fields")]
    fn rejects_empty_pattern() {
        PackageDef::new("p").localparam(
            "v",
            PackageType::Named("t".into()),
            PackageValue::Fields(vec![]),
        );
    }

    #[test]
    #[should_panic(expected = "package `p`, member `v`: field `a` is declared twice")]
    fn rejects_repeated_pattern_field() {
        PackageDef::new("p").localparam(
            "v",
            PackageType::Named("t".into()),
            PackageValue::Fields(vec![
                ("a".into(), PackageValue::Int(0)),
                ("a".into(), PackageValue::Int(1)),
            ]),
        );
    }

    #[test]
    #[should_panic(expected = "package `p`, member `v`: named fields need a named type")]
    fn rejects_fields_on_int_type() {
        PackageDef::new("p").localparam(
            "v",
            PackageType::IntUnsigned,
            PackageValue::Fields(vec![("a".into(), PackageValue::Int(0))]),
        );
    }

    #[test]
    #[should_panic(expected = "package `p`, member `v`: named fields need a named type")]
    fn rejects_fields_on_logic_type() {
        PackageDef::new("p").localparam(
            "v",
            PackageType::Logic(4),
            PackageValue::Fields(vec![("a".into(), PackageValue::Int(0))]),
        );
    }
}
