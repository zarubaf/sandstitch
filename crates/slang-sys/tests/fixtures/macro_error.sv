// SPDX-License-Identifier: Apache-2.0
`define BAD assign y = undeclared_name;
module macro_error (output logic y);
  `BAD
endmodule
