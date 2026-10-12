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
}
