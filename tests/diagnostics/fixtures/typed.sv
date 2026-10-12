// SPDX-License-Identifier: Apache-2.0
package bus_pkg;
  typedef struct packed { logic [31:0] addr; logic [7:0] id; } req_t;
  typedef struct packed { logic [7:0] id; logic [31:0] addr; } req2_t;
endpackage

module producer import bus_pkg::*; (
  output req_t req_o
);
endmodule

module consumer import bus_pkg::*; (
  input req2_t req_i
);
endmodule

package sel_pkg;
  typedef struct packed { logic [3:0] dst_id; logic [7:0] len; } hdr_t;
  typedef union packed { logic [11:0] raw; hdr_t hdr; } hdr_u;
endpackage

module sel import sel_pkg::*; (
  input hdr_t [7:0] hdrs_i,
  input hdr_u u_i
);
endmodule
