// SPDX-License-Identifier: Apache-2.0

//! Diagnostic codes. Each has an entry in `doc/errors/<code>.md`.

use super::Code;

/// Unknown parameter.
pub const S0101: Code = Code("S0101");
/// Slang elaboration error.
pub const S0102: Code = Code("S0102");
/// Unsupported port form (unpacked dimensions).
pub const S0103: Code = Code("S0103");
/// Interface port unsupported.
pub const S0104: Code = Code("S0104");
/// Unsupported override expression.
pub const S0105: Code = Code("S0105");
/// Incompatible types.
pub const S0201: Code = Code("S0201");
/// No such field.
pub const S0202: Code = Code("S0202");
/// Index out of range.
pub const S0203: Code = Code("S0203");
/// Field access through union.
pub const S0204: Code = Code("S0204");

/// Module defined both by emitted RTL and by a manifest source.
pub const S0301: Code = Code("S0301");

/// Every defined code.
pub const ALL: &[Code] = &[
    S0101, S0102, S0103, S0104, S0105, S0201, S0202, S0203, S0204, S0301,
];
