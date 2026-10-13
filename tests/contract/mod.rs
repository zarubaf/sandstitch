// SPDX-License-Identifier: Apache-2.0

use std::path::Path;

use crate::common::slang::{Driver, Expect, Sources};

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

/// Bits `[msb:lsb]` of `end` come from `driver`.
fn expect<'a>(end: &'a str, msb: usize, lsb: usize, driver: Driver<'a>) -> Expect<'a> {
    Expect {
        end,
        msb,
        lsb,
        driver,
    }
}

/// The paths of `sources`' files.
fn files(sources: &Sources) -> Vec<&Path> {
    sources.files.iter().map(|f| f.path()).collect()
}
