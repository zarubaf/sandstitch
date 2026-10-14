// SPDX-License-Identifier: Apache-2.0
//
// Simulation only: drives the clock (10 ns period) and the active-low
// reset of the FlooNoC reference NoC harness, then waits up to
// `TimeoutCycles` cycles for every core's `done_i`. It prints `PASS` if
// every core is done without `error_i` and every memory check reports
// `ok_i`, and `FAIL` with the reason otherwise, then ends the simulation.

module tb_driver #(
  parameter int unsigned NumCores      = 1,
  parameter int unsigned NumMems       = 1,
  parameter int unsigned TimeoutCycles = 10000
) (
  output logic                clk_o,
  output logic                rst_no,
  input  logic [NumCores-1:0] done_i,
  input  logic [NumCores-1:0] error_i,
  input  logic [NumMems-1:0]  ok_i
);

  localparam time ClkPeriod   = 10ns;
  localparam int unsigned ResetCycles = 5;
  /// Cycles after the last `done` for the memory monitors, which report a
  /// write one cycle late, to settle.
  localparam int unsigned SettleCycles = 4;

  initial begin
    clk_o = 1'b0;
    forever #(ClkPeriod / 2) clk_o = ~clk_o;
  end

  initial begin
    automatic int unsigned cycles = 0;
    rst_no = 1'b0;
    repeat (ResetCycles) @(posedge clk_o);
    rst_no = 1'b1;
    while (!(&done_i) && cycles < TimeoutCycles) begin
      @(posedge clk_o);
      cycles++;
    end
    repeat (SettleCycles) @(posedge clk_o);
    if (!(&done_i)) begin
      $display("FAIL: timeout after %0d cycles, done %b", cycles, done_i);
    end else if (|error_i) begin
      $display("FAIL: core error %b", error_i);
    end else if (!(&ok_i)) begin
      $display("FAIL: memory check ok %b", ok_i);
    end else begin
      $display("PASS: %0d cores done after %0d cycles", NumCores, cycles);
    end
    $finish;
  end

endmodule
