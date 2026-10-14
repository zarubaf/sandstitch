// SPDX-License-Identifier: Apache-2.0

use std::path::Path;
use std::sync::Arc;

use parking_lot::RwLock;

use indexmap::IndexMap;
use indexmap::map::Entry;
use xlsynth_vast::{
    Expr, LiteralFormat, LogicRef, VastDataType, VastFile, VastFileType, VastModule,
};

use super::net_types::{End, NetTypes, declarable, typed_path};
use crate::model::connection::connected_item::ConnectedItem;
use crate::model::parameter::ParameterSpec;
use crate::resolve::coexist::is_emitted;
use crate::resolve::design::DesignResolution;
use crate::resolve::{EndResolution, Resolution, Segment, Source};
use crate::{Dim, IO, ModDef, ModDefCore, Port, PortSlice, PortType, Select, Span, Usage};

#[derive(Debug, PartialEq, Clone)]
enum NetNameSource {
    ManuallySpecified(String),
    ModDefPort(String),
    ModInstPort((String, String)),
}

impl NetNameSource {
    /// The net named after `port`.
    fn of_port(port: &Port) -> Self {
        let port_name = port.get_port_name();
        match port.inst_name() {
            Some(inst_name) => NetNameSource::ModInstPort((inst_name.to_string(), port_name)),
            None => NetNameSource::ModDefPort(port_name),
        }
    }
}

use std::fmt;

/// Options controlling Verilog emission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmitOptions {
    /// Validate the module definition before emitting Verilog.
    pub validate: bool,
    /// Emit each bit of a continuous assignment as a separate assignment.
    pub bitblast_assignments: bool,
    /// Preserve a single-bit slice such as `x[2:2]` instead of simplifying it to `x[2]`.
    pub preserve_single_bit_slices: bool,
    /// Preserve a full-width slice such as `x[3:0]` instead of simplifying it to `x`.
    pub preserve_full_width_slices: bool,
}

impl Default for EmitOptions {
    fn default() -> Self {
        Self {
            validate: true,
            bitblast_assignments: false,
            preserve_single_bit_slices: false,
            preserve_full_width_slices: false,
        }
    }
}

impl fmt::Display for NetNameSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NetNameSource::ManuallySpecified(name) => write!(f, "{name}"),
            NetNameSource::ModDefPort(name) => write!(f, "{name}"),
            NetNameSource::ModInstPort((inst_name, port_name)) => {
                write!(f, "{}_{}", inst_name, port_name)
            }
        }
    }
}

struct NetCollection {
    sources: IndexMap<String, NetNameSource>,
    logic_refs: IndexMap<String, LogicRef>,
    /// Typing decisions for this module's nets.
    types: NetTypes,
    /// The declared type of each typed net; nets without an entry are flat.
    declared: IndexMap<String, PortType>,
}

impl NetCollection {
    pub fn new(types: NetTypes) -> Self {
        Self {
            sources: IndexMap::new(),
            logic_refs: IndexMap::new(),
            types,
            declared: IndexMap::new(),
        }
    }

    /// The data type of net `name`, `width` bits wide and named after a port
    /// of type `port_type`; remembers the type of a typed net.
    fn data_type(
        &mut self,
        name: &str,
        width: usize,
        port_type: Option<PortType>,
        file: &mut VastFile,
    ) -> VastDataType {
        match self.types.type_of(name, port_type) {
            Some(ty) => {
                let data_type = declared_type(&ty, file);
                self.declared.insert(name.to_string(), ty);
                data_type
            }
            None => file.make_bit_vector_type(width as i64, false),
        }
    }

    pub fn declare_mod_def_port(
        &mut self,
        name: &str,
        io: &IO,
        port_type: Option<PortType>,
        file: &mut VastFile,
        module: &mut VastModule,
    ) {
        // Declare the port in the sources map.
        let name_as_string = name.to_string();
        if let Some(existing_source) = self.sources.insert(
            name_as_string.clone(),
            NetNameSource::ModDefPort(name.to_string()),
        ) {
            panic!(
                "Error while declaring ModDef port {name} in a NetCollection: a net name source for this port has already been declared ({existing_source:?})"
            );
        }

        let width = match io {
            IO::Input(width) | IO::Output(width) | IO::InOut(width) => *width,
        };
        let data_type = self.data_type(name, width, port_type, file);
        let logic_ref = match io {
            IO::Input(_) => file.add_input(*module, name, &data_type),
            IO::Output(_) => file.add_output(*module, name, &data_type),
            IO::InOut(_) => file.add_inout(*module, name, &data_type),
        };

        if self.logic_refs.insert(name_as_string, logic_ref).is_some() {
            panic!(
                "NetCollection out of sync: net \"{name}\" is declared in the refs map but not the sources map."
            );
        }
    }

    pub fn get_logic_ref(&self, name: &str) -> Option<&LogicRef> {
        self.logic_refs.get(name)
    }

    /// Returns the net for `source`, declaring it on first use. `port_type`
    /// is the type of the port the net is named after.
    pub fn get_logic_ref_create_if_necessary(
        &mut self,
        source: NetNameSource,
        width: usize,
        port_type: Option<PortType>,
        file: &mut VastFile,
        module: &mut VastModule,
    ) -> LogicRef {
        match self.sources.entry(source.to_string()) {
            Entry::Occupied(source_entry) => {
                let existing_ref = self.logic_refs.get(source_entry.key());
                let existing_source = source_entry.get();
                if existing_source == &source {
                    *existing_ref.unwrap()
                } else {
                    unreachable!("net name collisions are reported by check()");
                }
            }
            Entry::Vacant(source_entry) => {
                let net_name = source.to_string();
                source_entry.insert(source.clone());
                let data_type = match source {
                    NetNameSource::ModInstPort(_) => {
                        self.data_type(&net_name, width, port_type, file)
                    }
                    NetNameSource::ManuallySpecified(_) => {
                        file.make_bit_vector_type(width as i64, false)
                    }
                    NetNameSource::ModDefPort(_) => {
                        panic!(
                            "ModDef ports should be added to NetCollection using declare_mod_def_port()"
                        )
                    }
                };
                let wire = file.add_wire(*module, &net_name, &data_type);
                match self.logic_refs.entry(net_name.clone()) {
                    Entry::Vacant(ref_entry) => *ref_entry.insert(wire),
                    Entry::Occupied(_) => {
                        panic!(
                            "NetCollection out of sync: net \"{}\" is declared in the refs map but not the sources map.",
                            net_name
                        );
                    }
                }
            }
        }
    }

    /// The expression for `slice` of net `name`: a member select or index on
    /// a typed net, a bit slice on a flat one.
    fn expression(
        &self,
        name: &str,
        net: LogicRef,
        slice: &PortSlice,
        end: End,
        file: &mut VastFile,
        options: &EmitOptions,
    ) -> Expr {
        match self.declared.get(name) {
            Some(_) => {
                let path = typed_path(slice, &end).unwrap_or_else(|| {
                    panic!(
                        "typed net {name} has an untyped use: {}",
                        slice.debug_string()
                    )
                });
                typed_net_expression(net, &path, file)
            }
            None => slice_net(
                &net,
                slice.port.io().width(),
                slice.msb,
                slice.lsb,
                file,
                options,
            ),
        }
    }
}

/// The declaration type of a typed net: the named type with the packed
/// dimensions of `ty` as declared.
fn declared_type(ty: &PortType, file: &mut VastFile) -> VastDataType {
    let (package, name) = declarable(ty).expect("typed nets have a declarable type");
    let element = file.make_extern_package_type(package, name);
    let dims = packed_dims(ty);
    if dims.is_empty() {
        element
    } else {
        let ranges: Vec<(i64, i64)> = dims.iter().map(|d| (d.msb, d.lsb)).collect();
        file.make_packed_array_type_ranges(element, &ranges)
    }
}

fn packed_dims(ty: &PortType) -> &[Dim] {
    match ty {
        PortType::Logic { packed, .. }
        | PortType::Struct { packed, .. }
        | PortType::Union { packed, .. }
        | PortType::Enum { packed, .. } => packed,
    }
}

/// The selection `path` of a typed net.
fn typed_net_expression(net: LogicRef, path: &[Select], file: &mut VastFile) -> Expr {
    if path.is_empty() {
        return net.to_expr();
    }
    // Dimensions are declared as in the type, so indices are emitted as
    // selected.
    let mut expr = net.to_indexable_expr();
    for select in path {
        expr = match select {
            Select::Field(name) => file.make_member_select_indexable(&expr.to_expr(), name),
            Select::Index(index) => file.make_index(&expr, *index as i64).to_indexable_expr(),
        };
    }
    expr.to_expr()
}

/// The segments of `end` to connect in an instantiation: `None` when the port
/// is only marked unused.
fn instance_segments(end: &EndResolution) -> Option<&[Segment]> {
    let only_unused = end.segments.len() == 1 && matches!(end.segments[0].source, Source::Unused);
    (!only_unused).then_some(end.segments.as_slice())
}

impl ModDef {
    /// Writes Verilog code for this module definition to the given file path.
    pub fn emit_to_file(&self, path: &Path, options: EmitOptions) {
        let err_msg = format!("emitting ModDef to file at path: {path:?}");
        std::fs::write(path, self.emit(options)).expect(&err_msg);
    }

    /// Returns Verilog code for this module definition as a string.
    ///
    /// Panics with the rendered diagnostics if [`ModDef::check`] reports
    /// errors.
    pub fn emit(&self, options: EmitOptions) -> String {
        let (diagnostics, design) = self.check_design();
        if diagnostics.has_errors() {
            panic!("{}", diagnostics.render());
        }
        if options.validate {
            self.validate();
        }
        let mut file = VastFile::new(VastFileType::SystemVerilog);
        for core in design.emitted() {
            ModDef { core: core.clone() }.emit_definition(&design, &mut file, &options);
        }
        let emit_result = file.emit();
        let mut leaf_text = Vec::new();
        if !emit_result.is_empty() {
            leaf_text.push(emit_result);
        }
        leaf_text.join("\n")
    }

    /// Adds this definition's module to `file`.
    fn emit_definition(
        &self,
        design: &DesignResolution,
        file: &mut VastFile,
        options: &EmitOptions,
    ) {
        let core = self.core.read();

        // Start the module declaration.

        let mut module = file.add_module(&core.name);

        // Look up this definition's resolution and decide from it which nets
        // are typed.

        let (types, resolved) = plan_nets(&self.core, &core, design, options);

        // Create nets for each module port

        let mut nets = NetCollection::new(types);

        for (port_name, io) in core.ports.iter() {
            let port_type = core.port_type(port_name).cloned();
            nets.declare_mod_def_port(port_name, io, port_type, file, &mut module);
        }

        let Some((resolution, assignments)) = resolved else {
            return;
        };

        // Create module instances
        for ((inst_name, inst), ends) in core.instances.iter().zip(&resolution.instances) {
            let module_name = inst.read().name.clone();
            let mut parameter_port_names: Vec<String> = Vec::new();
            let mut parameter_expr_vals: Vec<Expr> = Vec::new();
            let mut connection_port_names = Vec::new();
            let mut connection_expressions = Vec::new();

            for end in ends {
                let port_name = &end.name;
                let enum_t = inst
                    .read()
                    .enum_ports
                    .get(port_name)
                    .map(|enum_t| file.make_extern_type(enum_t));
                connection_port_names.push(port_name.clone());

                let Some(segments) = instance_segments(end) else {
                    connection_expressions.push(None);
                    continue;
                };

                let mut concat_entries = segments
                    .iter()
                    .map(|c| {
                        connected_item_to_expression(
                            &c.this,
                            &ConnectedItem::from(&c.source),
                            file,
                            &mut module,
                            &mut nets,
                            options,
                        )
                    })
                    .collect::<Vec<_>>();

                let connection_expression = match concat_entries.len() {
                    0 => None,
                    1 => Some(concat_entries.remove(0)),
                    _ => {
                        let slice_references: Vec<&Expr> = concat_entries.iter().collect();
                        Some(file.make_concat(&slice_references))
                    }
                };

                connection_expressions.push(connection_expression.map(|expr| {
                    if let Some(enum_t) = enum_t {
                        file.make_type_cast(&enum_t, &expr)
                    } else {
                        expr
                    }
                }));
            }

            // Build parameter override expressions, if any. An emitted
            // definition is already specialized and declares no parameters.
            if !inst.read().parameters.is_empty() && overrides_emitted(&inst.read()) {
                let param_core = inst.read();
                for (param_name, spec) in param_core.parameters.iter() {
                    parameter_port_names.push(param_name.clone());
                    let expr = match spec {
                        crate::model::parameter::ParameterSpec::Integer { value, ty } => {
                            if value.sign() == num_bigint::Sign::Minus {
                                // TODO(sherbst) 2025-10-29: Support negative parameter values
                                panic!("Negative parameter values not yet supported");
                            }
                            let literal_str = format!("bits[{}]:{}", ty.width(), value);
                            file.make_literal(&literal_str, &LiteralFormat::Hex)
                                .unwrap()
                        }
                        crate::model::parameter::ParameterSpec::String(value) => {
                            file.make_string_literal(value)
                        }
                        // Names were validated as `ident` or `pkg::ident` when
                        // the module was specialized.
                        crate::model::parameter::ParameterSpec::Type(name) => {
                            let ty = match name.split_once("::") {
                                Some((package, name)) => {
                                    file.make_extern_package_type(package, name)
                                }
                                None => file.make_extern_type(name),
                            };
                            file.make_type_expr(&ty)
                        }
                        crate::model::parameter::ParameterSpec::Expr(name) => {
                            match name.split_once("::") {
                                Some((package, name)) => file.make_package_ref(package, name),
                                None => file.make_name_ref(name),
                            }
                        }
                    };
                    parameter_expr_vals.push(expr);
                }
            }

            let parameter_expressions: Vec<&Expr> = parameter_expr_vals.iter().collect();
            let parameter_port_name_refs: Vec<&str> =
                parameter_port_names.iter().map(|s| s.as_str()).collect();
            let instantiation = file.make_instantiation(
                &module_name,
                inst_name,
                &parameter_port_name_refs,
                &parameter_expressions,
                &connection_port_names
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<&str>>(),
                &connection_expressions
                    .iter()
                    .map(|o| o.as_ref())
                    .collect::<Vec<_>>(),
            );
            file.add_member_instantiation(module, instantiation);
        }

        // Emit assign statements for ModDef ports if necessary
        for (this, other) in assignments {
            let port_name = this.port.get_port_name();
            let lhs = nets.expression(
                &port_name,
                *nets.get_logic_ref(&port_name).unwrap(),
                &this,
                End::of(&other),
                file,
                options,
            );

            let rhs =
                connected_item_to_expression(&this, &other, file, &mut module, &mut nets, options);

            let assignment = file.make_continuous_assignment(&lhs, &rhs);
            file.add_member_continuous_assignment(module, assignment);
        }

        debug_assert_eq!(
            nets.logic_refs
                .keys()
                .cloned()
                .collect::<std::collections::HashSet<_>>(),
            resolution
                .nets
                .iter()
                .map(|net| net.name.emitted())
                .collect::<std::collections::HashSet<_>>(),
            "the emitted nets of {} differ from its resolved nets",
            core.name
        );
    }
}

/// The continuous assignments driving a definition's ports, in emission
/// order.
type Assignments = Vec<(PortSlice, ConnectedItem)>;

/// What emission of definition `this` (whose locked core is `core`) decides
/// before emitting anything: which nets are typed and, unless it is a stub,
/// its resolution and the assignments driving its ports. A stub emits no
/// uses, so its ports take their own types.
fn plan_nets<'d>(
    this: &Arc<RwLock<ModDefCore>>,
    core: &ModDefCore,
    design: &'d DesignResolution,
    options: &EmitOptions,
) -> (NetTypes, Option<(&'d Resolution, Assignments)>) {
    let mut types = NetTypes::default();
    if core.usage == Usage::EmitStubAndStop {
        return (types, None);
    }
    let resolution = design
        .get(this)
        .expect("every emitted definition is resolved by resolve_design");
    debug_assert_eq!(
        core.instances.len(),
        resolution.instances.len(),
        "resolution instance count differs from the definition's"
    );
    for ((inst_name, inst), ends) in core.instances.iter().zip(&resolution.instances) {
        debug_assert!(
            inst.read()
                .ports
                .keys()
                .eq(ends.iter().map(|end| &end.name)),
            "resolution ports of instance {inst_name} differ from the instance's ports"
        );
    }
    let assignments = plan_assignments(resolution, options);
    record_uses(&mut types, resolution, &assignments);
    (types, Some((resolution, assignments)))
}

/// A type that an emitted definition names: the type of a typed port or net
/// declaration, or of an enum cast.
pub(crate) struct TypeUse {
    pub(crate) ty: PortType,
    /// The port declaring the type, with its declaration span: a port of the
    /// definition, or for an enum cast, the instantiated module's port.
    /// `None` for an instance net.
    pub(crate) port: Option<(String, Span)>,
}

/// Whether instances of `def` are emitted with its parameter overrides: only
/// for an opaque definition. An emitted one (stub or definition) is already
/// specialized and declares no parameters.
fn overrides_emitted(def: &ModDefCore) -> bool {
    !is_emitted(&def.usage)
}

/// What emission with `options` of definition `this` (whose locked core is
/// `core`), planned by `design`, names that a package may declare: the types
/// of its typed ports and nets and of its enum casts, and the type and
/// constant overrides of its instances (`ident` or `pkg::ident`).
pub(crate) fn emitted_uses(
    this: &Arc<RwLock<ModDefCore>>,
    core: &ModDefCore,
    design: &DesignResolution,
    options: &EmitOptions,
) -> (Vec<TypeUse>, Vec<String>) {
    let (types, resolved) = plan_nets(this, core, design, options);
    let span = |port: &str| {
        let span = core.port_spans.get(port).cloned().unwrap_or(Span::None);
        Some((port.to_string(), span))
    };
    let mut uses: Vec<TypeUse> = core
        .ports
        .keys()
        .filter_map(|name| {
            types
                .type_of(name, core.port_type(name).cloned())
                .map(|ty| TypeUse {
                    ty,
                    port: span(name),
                })
        })
        .collect();
    uses.extend(
        types
            .typed()
            .filter(|(name, _)| !core.ports.contains_key(*name))
            .map(|(_, ty)| TypeUse {
                ty: ty.clone(),
                port: None,
            }),
    );
    let mut names = Vec::new();
    let Some((resolution, _)) = resolved else {
        return (uses, names);
    };
    for (inst, ends) in core.instances.values().zip(&resolution.instances) {
        let inst = inst.read();
        for end in ends {
            if inst.enum_ports.contains_key(&end.name)
                && instance_segments(end).is_some()
                && let Some(ty) = inst.port_type(&end.name)
            {
                let span = inst.port_spans.get(&end.name).cloned();
                uses.push(TypeUse {
                    ty: ty.clone(),
                    port: Some((end.name.clone(), span.unwrap_or(Span::None))),
                });
            }
        }
        if !overrides_emitted(&inst) {
            continue;
        }
        for spec in inst.parameters.values() {
            if let ParameterSpec::Type(name) | ParameterSpec::Expr(name) = spec {
                names.push(name.clone());
            }
        }
    }
    (uses, names)
}

/// The continuous assignments driving this module's ports, as
/// (port slice, source) pairs in emission order.
fn plan_assignments(resolution: &Resolution, options: &EmitOptions) -> Assignments {
    let mut assignments = Vec::new();
    for end in &resolution.ports {
        for segment in &end.segments {
            let other = ConnectedItem::from(&segment.source);
            let other_width = match &other {
                ConnectedItem::PortSlice(port_slice) => {
                    if let Port::ModDef {
                        name: port_slice_port_name,
                        ..
                    } = &port_slice.port
                        && port_slice_port_name == &end.name
                    {
                        // If the expression source for this port is the
                        // port itself, skip it.
                        continue;
                    }
                    port_slice.width()
                }
                ConnectedItem::Tieoff(tieoff) => tieoff.width,
                ConnectedItem::Unused(_) => {
                    continue;
                }
                ConnectedItem::Wire(wire) => wire.msb - wire.lsb + 1,
            };

            let width = segment.this.width();

            // Connection construction and slicing should have caught any
            // width mismatch before emission, but double-check here.
            assert_eq!(
                other_width,
                width,
                "Width mismatch while emitting an assignment to {}",
                segment.this.debug_string(),
            );

            // Expand the assignment from MSB to LSB by slicing both sides
            // at the same relative offset. ConnectedItem slicing keeps
            // this mapping uniform for ports, wires, and constants.
            if options.bitblast_assignments {
                for offset in (0..width).rev() {
                    assignments.push((
                        segment.this.slice_with_offset_and_width(offset, 1),
                        other.slice_with_offset_and_width(offset, 1),
                    ));
                }
            } else {
                assignments.push((segment.this.clone(), other));
            }
        }
    }
    assignments
}

/// Records every use of a net that emission will make, with what each use
/// is connected to.
fn record_uses(
    types: &mut NetTypes,
    resolution: &Resolution,
    assignments: &[(PortSlice, ConnectedItem)],
) {
    let record_item = |types: &mut NetTypes, this: &PortSlice, item: &ConnectedItem| match item {
        ConnectedItem::PortSlice(slice) => types.record(
            NetNameSource::of_port(&slice.port).to_string(),
            slice,
            End::Slice(this),
        ),
        ConnectedItem::Unused(_) => types.record(
            NetNameSource::of_port(&this.port).to_string(),
            this,
            End::Slice(this),
        ),
        ConnectedItem::Tieoff(_) | ConnectedItem::Wire(_) => {}
    };
    for segment in resolution
        .instances
        .iter()
        .flatten()
        .filter_map(instance_segments)
        .flatten()
    {
        record_item(types, &segment.this, &ConnectedItem::from(&segment.source));
    }
    for (this, other) in assignments {
        types.record(this.port.get_port_name(), this, End::of(other));
        record_item(types, this, other);
    }
}

fn connected_item_to_expression(
    this: &PortSlice,
    item: &ConnectedItem,
    file: &mut VastFile,
    module: &mut VastModule,
    nets: &mut NetCollection,
    options: &EmitOptions,
) -> Expr {
    match item {
        ConnectedItem::PortSlice(port_slice) => {
            port_net_expression(port_slice, End::Slice(this), file, module, nets, options)
        }
        ConnectedItem::Wire(wire) => {
            let width = wire.width;
            let source = NetNameSource::ManuallySpecified(wire.name.clone());
            let net = nets.get_logic_ref_create_if_necessary(source, width, None, file, module);
            slice_net(&net, width, wire.msb, wire.lsb, file, options)
        }
        ConnectedItem::Tieoff(tieoff) => {
            let literal_str = format!("bits[{}]:{}", tieoff.width, tieoff.value);
            file.make_literal(&literal_str, &LiteralFormat::Hex)
                .unwrap()
        }
        ConnectedItem::Unused(_) => {
            port_net_expression(this, End::Slice(this), file, module, nets, options)
        }
    }
}

/// `slice` of the net named after its port, connected to `end`, declaring
/// the net if necessary.
fn port_net_expression(
    slice: &PortSlice,
    end: End,
    file: &mut VastFile,
    module: &mut VastModule,
    nets: &mut NetCollection,
    options: &EmitOptions,
) -> Expr {
    let port = &slice.port;
    let source = NetNameSource::of_port(port);
    let name = source.to_string();
    let net = nets.get_logic_ref_create_if_necessary(
        source,
        port.io().width(),
        port.port_type(),
        file,
        module,
    );
    nets.expression(&name, net, slice, end, file, options)
}

fn slice_net(
    net: &LogicRef,
    width: usize,
    msb: usize,
    lsb: usize,
    file: &mut VastFile,
    options: &EmitOptions,
) -> Expr {
    if !options.preserve_full_width_slices && width == (msb - lsb + 1) {
        net.to_expr()
    } else if !options.preserve_single_bit_slices && msb == lsb {
        file.make_index(&net.to_indexable_expr(), msb as i64)
            .to_expr()
    } else {
        file.make_slice(&net.to_indexable_expr(), msb as i64, lsb as i64)
            .to_expr()
    }
}
