// SPDX-License-Identifier: Apache-2.0
`include "defs.svh"
module uses_header (input logic [`WIDTH-1:0] a, output logic [`WIDTH-1:0] y);
  assign y = a;
endmodule
