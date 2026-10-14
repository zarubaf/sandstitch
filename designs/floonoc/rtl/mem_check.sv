// SPDX-License-Identifier: Apache-2.0
//
// Simulation only: the memory of one memory endpoint of the FlooNoC
// reference NoC and its check. Wraps `axi_sim_mem` and watches its write
// monitor: every written beat must lie in the window of router (`X`, `Y`),
// that is, carry `X` in the address bits `[AddrOffsetX +: XBits]` and `Y` in
// `[AddrOffsetY +: YBits]`. `count_o` counts the completed write bursts;
// `ok_o` is high while no beat left the window and the count equals
// `ExpectedWrites`.
//
// The memory responds `ApplDelay` after and samples `AcqDelay` after the
// rising clock edge, inside `tb_driver`'s 10 ns clock period.

module mem_check #(
  /// AXI request and response structs of the memory port.
  parameter type         axi_req_t      = logic,
  parameter type         axi_rsp_t      = logic,
  /// AXI widths of the memory port.
  parameter int unsigned AddrWidth      = 0,
  parameter int unsigned DataWidth      = 0,
  parameter int unsigned IdWidth        = 0,
  parameter int unsigned UserWidth      = 0,
  /// The router coordinates of this memory.
  parameter int unsigned X              = 0,
  parameter int unsigned Y              = 0,
  /// Where the coordinates sit in an address.
  parameter int unsigned XBits          = 1,
  parameter int unsigned YBits          = 1,
  parameter int unsigned AddrOffsetX    = 0,
  parameter int unsigned AddrOffsetY    = 0,
  /// The number of write bursts the cores targeting this memory issue.
  parameter int unsigned ExpectedWrites = 0
) (
  input  logic        clk_i,
  input  logic        rst_ni,
  input  axi_req_t    axi_req_i,
  output axi_rsp_t    axi_rsp_o,
  output logic        ok_o,
  output logic [31:0] count_o
);

  localparam time ApplDelay = 2ns;
  localparam time AcqDelay  = 8ns;

  logic                 mon_w_valid, mon_w_last;
  logic [AddrWidth-1:0] mon_w_addr;

  axi_sim_mem #(
    .AddrWidth (AddrWidth),
    .DataWidth (DataWidth),
    .IdWidth   (IdWidth),
    .UserWidth (UserWidth),
    .NumPorts  (1),
    .axi_req_t (axi_req_t),
    .axi_rsp_t (axi_rsp_t),
    .ApplDelay (ApplDelay),
    .AcqDelay  (AcqDelay)
  ) i_mem (
    .clk_i,
    .rst_ni,
    .axi_req_i,
    .axi_rsp_o,
    .mon_w_valid_o      (mon_w_valid),
    .mon_w_addr_o       (mon_w_addr),
    .mon_w_data_o       (),
    .mon_w_id_o         (),
    .mon_w_user_o       (),
    .mon_w_beat_count_o (),
    .mon_w_last_o       (mon_w_last),
    .mon_r_valid_o      (),
    .mon_r_addr_o       (),
    .mon_r_data_o       (),
    .mon_r_id_o         (),
    .mon_r_user_o       (),
    .mon_r_beat_count_o (),
    .mon_r_last_o       ()
  );

  logic [31:0] count_q;
  logic        stray_q;

  always_ff @(posedge clk_i or negedge rst_ni) begin
    if (!rst_ni) begin
      count_q <= '0;
      stray_q <= 1'b0;
    end else if (mon_w_valid) begin
      if (mon_w_addr[AddrOffsetX+:XBits] != XBits'(X) ||
          mon_w_addr[AddrOffsetY+:YBits] != YBits'(Y)) begin
        stray_q <= 1'b1;
        $display("mem_check (%0d, %0d): write to 0x%0h outside the window", X, Y, mon_w_addr);
      end
      if (mon_w_last) count_q <= count_q + 32'd1;
    end
  end

  assign count_o = count_q;
  assign ok_o    = !stray_q && count_q == ExpectedWrites;

endmodule
