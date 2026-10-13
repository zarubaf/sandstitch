// SPDX-License-Identifier: Apache-2.0

mod bit_ranges;
mod checker;
mod constants;
mod fan_out;
mod hierarchy;

/// Leaf modules the contract tests instantiate.
const LEAVES: &str = "\
module src8(output [7:0] o); endmodule
module snk8(input [7:0] dst); endmodule
module snk4(input [3:0] d); endmodule
";
