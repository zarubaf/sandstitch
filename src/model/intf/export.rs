// SPDX-License-Identifier: Apache-2.0

use crate::model::intf::IntfMember;
use crate::{ConvertibleToPortSlice, IO, Intf, ModDef, Port};

impl Intf {
    /// Creates a new interface on the parent module and connects it to this
    /// interface. The new interface will have the same functions as this
    /// interface; signal names are formed by concatenating the given prefix and
    /// the function name. For example, if this interface is `{"data": "a_data",
    /// "valid": "a_valid"}` and the prefix is "b_", the new interface will be
    /// `{"data": "b_data", "valid": "b_valid"}`. The `name` argument specifies
    /// the name of the new interface, which is used to retrieve the interface
    /// with `get_intf`.
    ///
    /// Exported ports keep the member's type. When the interface has a name
    /// in both directions, the ports are named `<prefix><name>_i` and
    /// `<prefix><name>_o` after the exported port's direction (`_io` for an
    /// inout), and the new interface has the name in both directions too.
    #[track_caller]
    pub fn export_with_prefix(&self, name: impl AsRef<str>, prefix: impl AsRef<str>) -> Intf {
        let members = self.members();
        let repeated = |member: &str| members.iter().filter(|(m, _)| m == member).count() > 1;
        let exported = members
            .iter()
            .map(|(member, slice)| {
                let port_name = if repeated(member) {
                    // Exporting a port of the module itself flips it.
                    let io = match slice.port {
                        Port::ModDef { .. } => slice.port.io().flip(),
                        Port::ModInst { .. } => slice.port.io(),
                    };
                    let suffix = match io {
                        IO::Input(_) => "i",
                        IO::Output(_) => "o",
                        IO::InOut(_) => "io",
                    };
                    format!("{}{member}_{suffix}", prefix.as_ref())
                } else {
                    format!("{}{member}", prefix.as_ref())
                };
                IntfMember::new(member, &slice.export_as(&port_name).to_port_slice())
            })
            .collect();
        ModDef {
            core: self.get_mod_def_core(),
        }
        .define_intf(name.as_ref(), exported)
    }

    /// Export an interface using the given name, with a signal prefix of the
    /// name followed by an underscore. For example, if a block has an interface
    /// called "a" with signals "a_data" and "a_valid", calling
    /// export_with_name_underscore("b") will create a new interface called "b"
    /// with signals "b_data" and "b_valid".
    #[track_caller]
    pub fn export_with_name_underscore(&self, name: impl AsRef<str>) -> Intf {
        let prefix = format!("{}_", name.as_ref());
        self.export_with_prefix(name, prefix)
    }

    /// Exports an interface from a module instance to the parent module
    /// definition, returning a new interface. The new interface has the same
    /// name as the original interface, as well as the same signal names and
    /// signal functions. For example, calling this method on an interface on an
    /// intance called "a" with signals "a_data" and "a_valid" will create a new
    /// interface called "a" on the parent module definition with signals
    /// "a_data" and "a_valid".
    #[track_caller]
    pub fn export(&self) -> Intf {
        if matches!(self, Intf::ModDef { .. }) {
            panic!(
                "Cannot export() {}; must use export_with_prefix() or export_with_name_underscore() instead.",
                self.debug_string()
            );
        }

        let exported = self
            .members()
            .iter()
            .map(|(member, slice)| IntfMember::new(member, &slice.export().to_port_slice()))
            .collect();
        ModDef {
            core: self.get_mod_def_core(),
        }
        .define_intf(&self.get_intf_name(), exported)
    }
}
