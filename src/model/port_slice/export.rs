// SPDX-License-Identifier: Apache-2.0

use crate::PortKind;
use crate::{IO, ModDef, Port, PortSlice, Span};

impl PortSlice {
    /// Create a new port called `name` on the parent module and connects it to
    /// this port slice.
    ///
    /// The exact behavior depends on whether this is a port slice on a module
    /// definition or a module instance. If this is a port slice on a module
    /// definition, a new port is created on the same module definition, with
    /// the same width, but opposite direction. For example, suppose that this
    /// is a port slice `a` on a module definition that is an 8-bit input;
    /// calling `export_as("y")` will create an 8-bit output on the same
    /// module definition called `y`.
    ///
    /// If, on the other hand, this is a port slice on a module instance, a new
    /// port will be created on the module definition containing the
    /// instance, with the same width and direction. For example, if this is
    /// an 8-bit input port `x` on a module instance, calling
    /// `export_as("y")` will create a new 8-bit input port `y` on the
    /// module definition that contains the instance.
    ///
    /// A typed slice (a whole typed port, a field or an element) exports as a
    /// port of the same type.
    #[track_caller]
    pub fn export_as(&self, name: impl AsRef<str>) -> Port {
        let io = match &self.port.kind {
            PortKind::ModDef { .. } => self.port.io().with_width(self.width()).flip(),
            PortKind::ModInst { .. } => self.port.io().with_width(self.width()),
        };

        let core = self.get_mod_def_core();
        let moddef = ModDef { core };

        let new_port = moddef.add_port(name, io.clone());
        if let Some(ty) = self.port_type() {
            let io: fn(usize) -> IO = match io {
                IO::Input(_) => IO::Input,
                IO::Output(_) => IO::Output,
                IO::InOut(_) => IO::InOut,
            };
            moddef.core.write().insert_typed_port(
                new_port.name().to_string(),
                io,
                ty,
                Span::caller(),
            );
        }
        self.connect(&new_port);

        new_port
    }

    /// Same as export_as(), but the new port is created with the same name as
    /// the port being exported. As a result, this method can only be used with
    /// ports on module instances. The method will panic if called on a port
    /// slice on a module definition.
    #[track_caller]
    pub fn export(&self) -> Port {
        let name = match &self.port.kind {
            PortKind::ModDef { .. } => panic!(
                "Use export_as() to export {}, specifying the new name of the exported port.",
                self.debug_string()
            ),
            PortKind::ModInst { port_name, .. } => port_name.clone(),
        };
        self.export_as(&name)
    }
}
