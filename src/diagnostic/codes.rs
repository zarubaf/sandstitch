// SPDX-License-Identifier: Apache-2.0

//! Diagnostic codes. Each has an entry in `doc/errors/<code>.md`.

use super::Code;

/// Defines each code as a constant and lists them all in [`ALL`], so that
/// no code can be left out of it.
macro_rules! codes {
    ($($(#[$doc:meta])* $name:ident;)*) => {
        $(
            $(#[$doc])*
            pub const $name: Code = Code(stringify!($name));
        )*

        /// Every defined code.
        pub const ALL: &[Code] = &[$($name),*];
    };
}

codes! {
    /// Unknown parameter.
    S0101;
    /// Slang elaboration error.
    S0102;
    /// Unsupported port form (unpacked dimensions or a non-integral type).
    S0103;
    /// Interface port unsupported.
    S0104;
    /// Unsupported override: an expression other than `ident` or
    /// `pkg::ident`, a value of the wrong kind or range, or a `-G` value that
    /// cannot be emitted.
    S0105;
    /// Incompatible types.
    S0201;
    /// No such field.
    S0202;
    /// Index out of range.
    S0203;
    /// Field access through union.
    S0204;
    /// Field access on an unpacked struct.
    S0205;
    /// Unpacked struct or union connected as bits.
    S0206;
    /// Interface member without a partner.
    S0207;
    /// Interface member repeated in one direction.
    S0208;
    /// Name-based interface operation on a member present in both directions.
    S0209;
    /// Module defined both by emitted RTL and by a manifest source.
    S0301;
    /// Macro defined differently by two imports in the build.
    S0302;
    /// Component bits with more than one driver: two definition inputs or
    /// instance outputs, one of each, a constant and a port, or two constants.
    S0401;
    /// Bits of an end that are not connected, tied off or marked unused.
    S0402;
    /// Component bits with no driver.
    S0403;
    /// Conflicting intent on the same bits: unused and connected, tied off or
    /// wired; unused twice; the same end tied off twice; wired twice; tied off
    /// and wired; a wire on a net driven by a definition input.
    S0404;
    /// Tieoff or unused on an end whose direction does not allow it.
    S0405;
    /// Inout conflict: several definition inouts on one net, or a definition
    /// inout with a definition input or output.
    S0406;
    /// Two different nets that the emitted module would declare under one
    /// name.
    S0407;
    /// A connection between bits already joined by other connections, other
    /// than an identical repeat.
    S0409;
}
