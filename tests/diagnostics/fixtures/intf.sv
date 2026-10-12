// SPDX-License-Identifier: Apache-2.0

package ipkg;
  typedef struct packed { logic valid; logic [7:0] data; } req_t;
  typedef struct packed { logic ready; logic [3:0] code; } rsp_t;
endpackage

module node import ipkg::*; (
  input  req_t [1:0] req_i,
  output req_t [1:0] req_o,
  input  rsp_t [1:0] rsp_i,
  output rsp_t [1:0] rsp_o
);
endmodule
