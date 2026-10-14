// SPDX-License-Identifier: Apache-2.0
//
// A synthesizable AXI manager for the FlooNoC reference NoC. After reset it
// writes four beats of `Seed`-derived data to `TargetBase + Seed * 64`, reads
// them back and compares them, then raises `done_o`. `error_o` rises with it
// if a response is not OKAY, a read beat differs from what was written, or
// the last flag of a read beat is wrong.
//
// `Seed` must be below 1024 and the burst at most 64 bytes, so that the
// writes of all seeds stay apart inside the 64 KiB from `TargetBase`.

module traffic_core #(
  /// AXI request and response structs (`AXI_TYPEDEF_ALL_CT`).
  parameter type         axi_req_t  = logic,
  parameter type         axi_rsp_t  = logic,
  /// Base address of the target memory's window.
  parameter int unsigned TargetBase = 0,
  /// Selects the address and the data.
  parameter int unsigned Seed       = 0
) (
  input  logic     clk_i,
  input  logic     rst_ni,
  output axi_req_t axi_req_o,
  input  axi_rsp_t axi_rsp_i,
  output logic     done_o,
  output logic     error_o
);

  localparam int unsigned AddrWidth = $bits(axi_req_o.aw.addr);
  localparam int unsigned DataWidth = $bits(axi_req_o.w.data);
  localparam int unsigned StrbWidth = DataWidth / 8;
  localparam int unsigned NumBeats  = 4;
  localparam logic [1:0]  LastBeat  = 2'(NumBeats - 1);

  if (Seed >= 1024) begin : gen_seed_check
    $error("traffic_core: Seed %0d is not below 1024", Seed);
  end
  if (NumBeats * StrbWidth > 64) begin : gen_burst_check
    $error("traffic_core: a %0d-byte burst exceeds 64 bytes", NumBeats * StrbWidth);
  end

  localparam logic [AddrWidth-1:0] Addr = AddrWidth'(TargetBase) + AddrWidth'(Seed * 64);

  /// The data of beat `beat`: per 32-bit lane, the seed, the beat, the lane
  /// and a constant marker.
  function automatic logic [DataWidth-1:0] pattern(logic [1:0] beat);
    logic [DataWidth-1:0] data;
    for (int unsigned i = 0; i < StrbWidth; i++) begin
      logic [31:0] word;
      word = {6'h2a, 10'(Seed), beat, 6'(i / 4), 8'h5c};
      data[8*i+:8] = word[8*(i%4)+:8];
    end
    return data;
  endfunction

  typedef enum logic [2:0] {
    StAw,
    StW,
    StB,
    StAr,
    StR,
    StDone
  } state_e;

  state_e state_q, state_d;
  logic [1:0] beat_q, beat_d;
  logic error_q, error_d;

  always_comb begin
    axi_req_o          = '0;
    axi_req_o.aw.addr  = Addr;
    axi_req_o.aw.len   = axi_pkg::len_t'(NumBeats - 1);
    axi_req_o.aw.size  = axi_pkg::size_t'($clog2(StrbWidth));
    axi_req_o.aw.burst = axi_pkg::BURST_INCR;
    axi_req_o.aw_valid = state_q == StAw;
    axi_req_o.w.data   = pattern(beat_q);
    axi_req_o.w.strb   = '1;
    axi_req_o.w.last   = beat_q == LastBeat;
    axi_req_o.w_valid  = state_q == StW;
    axi_req_o.b_ready  = state_q == StB;
    axi_req_o.ar.addr  = Addr;
    axi_req_o.ar.len   = axi_pkg::len_t'(NumBeats - 1);
    axi_req_o.ar.size  = axi_pkg::size_t'($clog2(StrbWidth));
    axi_req_o.ar.burst = axi_pkg::BURST_INCR;
    axi_req_o.ar_valid = state_q == StAr;
    axi_req_o.r_ready  = state_q == StR;
  end

  always_comb begin
    state_d = state_q;
    beat_d  = beat_q;
    error_d = error_q;
    unique case (state_q)
      StAw: if (axi_rsp_i.aw_ready) state_d = StW;
      StW: begin
        if (axi_rsp_i.w_ready) begin
          beat_d = beat_q + 2'd1;
          if (beat_q == LastBeat) state_d = StB;
        end
      end
      StB: begin
        if (axi_rsp_i.b_valid) begin
          if (axi_rsp_i.b.resp != axi_pkg::RESP_OKAY) error_d = 1'b1;
          state_d = StAr;
        end
      end
      StAr: if (axi_rsp_i.ar_ready) state_d = StR;
      StR: begin
        if (axi_rsp_i.r_valid) begin
          if (axi_rsp_i.r.resp != axi_pkg::RESP_OKAY ||
              axi_rsp_i.r.data != pattern(beat_q) ||
              axi_rsp_i.r.last != (beat_q == LastBeat)) begin
            error_d = 1'b1;
          end
          beat_d = beat_q + 2'd1;
          if (beat_q == LastBeat) state_d = StDone;
        end
      end
      default: ;
    endcase
  end

  always_ff @(posedge clk_i or negedge rst_ni) begin
    if (!rst_ni) begin
      state_q <= StAw;
      beat_q  <= '0;
      error_q <= 1'b0;
    end else begin
      state_q <= state_d;
      beat_q  <= beat_d;
      error_q <= error_d;
    end
  end

  assign done_o  = state_q == StDone;
  assign error_o = error_q;

endmodule
