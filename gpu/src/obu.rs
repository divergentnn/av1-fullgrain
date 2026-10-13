//! AV1 film grain parameters straight from the bitstream: a small AV1 header parser (no pixel decoding) that follows
//! the reference state the frame header syntax depends on, far enough to read film_grain_params() of every frame,
//! and reports the grain of each shown frame, as a decoder applies it (update_grain = 0 loads a reference slot's
//! parameters with the new seed; show_existing_frame shows a slot's stored parameters).
//!
//! A port of tools/av1-grain-seed/av1hdr.c, which matches libdav1d's frame header parser on the decoder's streams.
//! Section numbers refer to the AV1 Bitstream & Decoding Process Specification.
//!
//! Copyright © 2026, divergentnn (av1-fullgrain). SPDX-License-Identifier: BSD-2-Clause

use crate::params::{FilmGrainParams, ParamError};

const NUM_REF_FRAMES: usize = 8;
const REFS_PER_FRAME: usize = 7;
const PRIMARY_REF_NONE: u32 = 7;
const SELECT: u32 = 2; // SELECT_SCREEN_CONTENT_TOOLS, SELECT_INTEGER_MV
const WARPEDMODEL_PREC_BITS: u32 = 16;
const KEY_FRAME: u32 = 0;
const INTER_FRAME: u32 = 1;
const INTRA_ONLY_FRAME: u32 = 2;
const SWITCH_FRAME: u32 = 3;

fn err<T>(m: impl Into<String>) -> Result<T, ParamError> {
    Err(ParamError(m.into()))
}

struct Bits<'a> {
    buf: &'a [u8],
    pos: usize,
    overrun: bool,
}

impl<'a> Bits<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Bits { buf, pos: 0, overrun: false }
    }
    fn f(&mut self, n: u32) -> u32 {
        let mut v = 0u32;
        for _ in 0..n {
            let bit = if self.pos >= self.buf.len() * 8 {
                self.overrun = true;
                0
            } else {
                (self.buf[self.pos >> 3] >> (7 - (self.pos & 7))) & 1
            };
            v = (v << 1) | bit as u32;
            self.pos += 1;
        }
        v
    }
    fn flag(&mut self) -> bool {
        self.f(1) != 0
    }
    fn su(&mut self, n: u32) -> i32 {
        let v = self.f(n) as i32;
        let sign = 1 << (n - 1);
        if v & sign != 0 { v - 2 * sign } else { v }
    }
    fn ns(&mut self, n: u32) -> u32 {
        let mut w = 0;
        let mut x = n;
        while x != 0 {
            w += 1;
            x >>= 1;
        }
        let m = (1u32 << w) - n;
        let v = self.f(w - 1);
        if v < m {
            return v;
        }
        let extra = self.f(1);
        (v << 1) - m + extra
    }
    fn uvlc(&mut self) -> u32 {
        let mut lz = 0;
        while !self.overrun {
            if self.flag() {
                break;
            }
            lz += 1;
            if lz > 40 {
                break;
            }
        }
        if lz >= 32 {
            return u32::MAX;
        }
        self.f(lz) + ((1u32 << lz) - 1)
    }
    fn byte_alignment(&mut self) {
        while self.pos & 7 != 0 {
            self.f(1);
        }
    }
}

/// film_grain_params() as coded (offsets not removed)
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
struct Grain {
    apply_grain: bool,
    grain_seed: u16,
    num_y_points: u32,
    y_points: [[u8; 2]; 16],
    chroma_scaling_from_luma: bool,
    num_cb_points: u32,
    cb_points: [[u8; 2]; 16],
    num_cr_points: u32,
    cr_points: [[u8; 2]; 16],
    grain_scaling_minus_8: u32,
    ar_coeff_lag: u32,
    ar_coeffs_y_plus_128: [u8; 24],
    ar_coeffs_cb_plus_128: [u8; 25],
    ar_coeffs_cr_plus_128: [u8; 25],
    ar_coeff_shift_minus_6: u32,
    grain_scale_shift: u32,
    cb_mult: u32,
    cb_luma_mult: u32,
    cb_offset: u32,
    cr_mult: u32,
    cr_luma_mult: u32,
    cr_offset: u32,
    overlap_flag: bool,
    clip_to_restricted_range: bool,
}

impl Grain {
    /// dav1d's values (the parameters as dav1d stores them for the picture)
    fn to_params(&self) -> Option<FilmGrainParams> {
        if !self.apply_grain {
            return None;
        }
        let mut g = FilmGrainParams { seed: self.grain_seed as u32, ..Default::default() };
        g.num_y_points = self.num_y_points as i32;
        g.y_points[..self.num_y_points as usize].copy_from_slice(&self.y_points[..self.num_y_points as usize]);
        g.chroma_scaling_from_luma = self.chroma_scaling_from_luma as i32;
        g.num_uv_points = [self.num_cb_points as i32, self.num_cr_points as i32];
        g.uv_points[0][..self.num_cb_points as usize].copy_from_slice(&self.cb_points[..self.num_cb_points as usize]);
        g.uv_points[1][..self.num_cr_points as usize].copy_from_slice(&self.cr_points[..self.num_cr_points as usize]);
        g.scaling_shift = self.grain_scaling_minus_8 as i32 + 8;
        g.ar_coeff_lag = self.ar_coeff_lag as i32;
        let npos = 2 * self.ar_coeff_lag as usize * (self.ar_coeff_lag as usize + 1);
        if self.num_y_points > 0 {
            for i in 0..npos {
                g.ar_coeffs_y[i] = (self.ar_coeffs_y_plus_128[i] as i32 - 128) as i8;
            }
        }
        let nuv = npos + (self.num_y_points > 0) as usize;
        for (pl, (n, c)) in [(self.num_cb_points, &self.ar_coeffs_cb_plus_128), (self.num_cr_points, &self.ar_coeffs_cr_plus_128)]
            .into_iter()
            .enumerate()
        {
            if n > 0 || self.chroma_scaling_from_luma {
                for i in 0..nuv {
                    g.ar_coeffs_uv[pl][i] = (c[i] as i32 - 128) as i8;
                }
            }
        }
        g.ar_coeff_shift = self.ar_coeff_shift_minus_6 as u64 + 6;
        g.grain_scale_shift = self.grain_scale_shift as i32;
        if self.num_cb_points > 0 {
            g.uv_mult[0] = self.cb_mult as i32 - 128;
            g.uv_luma_mult[0] = self.cb_luma_mult as i32 - 128;
            g.uv_offset[0] = self.cb_offset as i32 - 256;
        }
        if self.num_cr_points > 0 {
            g.uv_mult[1] = self.cr_mult as i32 - 128;
            g.uv_luma_mult[1] = self.cr_luma_mult as i32 - 128;
            g.uv_offset[1] = self.cr_offset as i32 - 256;
        }
        g.overlap_flag = self.overlap_flag as i32;
        g.clip_to_restricted_range = self.clip_to_restricted_range as i32;
        Some(g)
    }
}

#[derive(Clone, Default)]
struct Seq {
    still_picture_reduced: bool,
    timing_info_present: bool,
    decoder_model_info_present: bool,
    equal_picture_interval: bool,
    buffer_removal_time_length_minus_1: u32,
    frame_presentation_time_length_minus_1: u32,
    operating_points_cnt_minus_1: usize,
    operating_point_idc: [u32; 32],
    decoder_model_present_for_this_op: [bool; 32],
    frame_width_bits_minus_1: u32,
    frame_height_bits_minus_1: u32,
    max_frame_width_minus_1: u32,
    max_frame_height_minus_1: u32,
    frame_id_numbers_present: bool,
    delta_frame_id_length_minus_2: u32,
    additional_frame_id_length_minus_1: u32,
    use_128x128_superblock: bool,
    enable_warped_motion: bool,
    enable_order_hint: bool,
    enable_ref_frame_mvs: bool,
    seq_force_screen_content_tools: u32,
    seq_force_integer_mv: u32,
    order_hint_bits: u32,
    enable_superres: bool,
    enable_cdef: bool,
    enable_restoration: bool,
    bit_depth: u32,
    mono_chrome: bool,
    num_planes: u32,
    matrix_coefficients: u32,
    subsampling_x: u32,
    subsampling_y: u32,
    separate_uv_delta_q: bool,
    film_grain_params_present: bool,
}

fn parse_sequence_header(buf: &[u8]) -> Result<Seq, ParamError> {
    let mut b = Bits::new(buf);
    let mut s = Seq::default();
    let seq_profile = b.f(3);
    let _still_picture = b.f(1);
    s.still_picture_reduced = b.flag();
    if seq_profile > 2 {
        return err(format!("sequence header: seq_profile {seq_profile}"));
    }
    if s.still_picture_reduced {
        b.f(5); // seq_level_idx[0]
    } else {
        s.timing_info_present = b.flag();
        let mut buffer_delay_length_minus_1 = 0;
        if s.timing_info_present {
            b.f(32);
            b.f(32);
            s.equal_picture_interval = b.flag();
            if s.equal_picture_interval {
                b.uvlc();
            }
            s.decoder_model_info_present = b.flag();
            if s.decoder_model_info_present {
                buffer_delay_length_minus_1 = b.f(5);
                b.f(32);
                s.buffer_removal_time_length_minus_1 = b.f(5);
                s.frame_presentation_time_length_minus_1 = b.f(5);
            }
        }
        let initial_display_delay_present = b.flag();
        s.operating_points_cnt_minus_1 = b.f(5) as usize;
        for i in 0..=s.operating_points_cnt_minus_1 {
            s.operating_point_idc[i] = b.f(12);
            let level = b.f(5);
            if level > 7 {
                b.f(1); // seq_tier
            }
            if s.decoder_model_info_present {
                s.decoder_model_present_for_this_op[i] = b.flag();
                if s.decoder_model_present_for_this_op[i] {
                    let n = buffer_delay_length_minus_1 + 1;
                    b.f(n);
                    b.f(n);
                    b.f(1);
                }
            }
            if initial_display_delay_present && b.flag() {
                b.f(4);
            }
        }
    }
    s.frame_width_bits_minus_1 = b.f(4);
    s.frame_height_bits_minus_1 = b.f(4);
    s.max_frame_width_minus_1 = b.f(s.frame_width_bits_minus_1 + 1);
    s.max_frame_height_minus_1 = b.f(s.frame_height_bits_minus_1 + 1);
    if !s.still_picture_reduced {
        s.frame_id_numbers_present = b.flag();
    }
    if s.frame_id_numbers_present {
        s.delta_frame_id_length_minus_2 = b.f(4);
        s.additional_frame_id_length_minus_1 = b.f(3);
    }
    s.use_128x128_superblock = b.flag();
    b.f(1); // enable_filter_intra
    b.f(1); // enable_intra_edge_filter
    if s.still_picture_reduced {
        s.seq_force_screen_content_tools = SELECT;
        s.seq_force_integer_mv = SELECT;
    } else {
        b.f(1); // enable_interintra_compound
        b.f(1); // enable_masked_compound
        s.enable_warped_motion = b.flag();
        b.f(1); // enable_dual_filter
        s.enable_order_hint = b.flag();
        if s.enable_order_hint {
            b.f(1); // enable_jnt_comp
            s.enable_ref_frame_mvs = b.flag();
        }
        s.seq_force_screen_content_tools = if b.flag() { SELECT } else { b.f(1) };
        s.seq_force_integer_mv = if s.seq_force_screen_content_tools > 0 {
            if b.flag() { SELECT } else { b.f(1) }
        } else {
            SELECT
        };
        if s.enable_order_hint {
            s.order_hint_bits = b.f(3) + 1;
        }
    }
    s.enable_superres = b.flag();
    s.enable_cdef = b.flag();
    s.enable_restoration = b.flag();
    // color_config()
    let high_bitdepth = b.flag();
    s.bit_depth = if seq_profile == 2 && high_bitdepth {
        if b.flag() { 12 } else { 10 }
    } else if high_bitdepth {
        10
    } else {
        8
    };
    s.mono_chrome = if seq_profile == 1 { false } else { b.flag() };
    s.num_planes = if s.mono_chrome { 1 } else { 3 };
    let (mut cp, mut tc, mut mc) = (2, 2, 2);
    if b.flag() {
        cp = b.f(8);
        tc = b.f(8);
        mc = b.f(8);
    }
    s.matrix_coefficients = mc;
    if s.mono_chrome {
        b.f(1); // color_range
        s.subsampling_x = 1;
        s.subsampling_y = 1;
    } else {
        if cp == 1 && tc == 13 && mc == 0 {
            s.subsampling_x = 0;
            s.subsampling_y = 0;
        } else {
            b.f(1); // color_range
            if seq_profile == 0 {
                s.subsampling_x = 1;
                s.subsampling_y = 1;
            } else if seq_profile == 1 {
                s.subsampling_x = 0;
                s.subsampling_y = 0;
            } else if s.bit_depth == 12 {
                s.subsampling_x = b.f(1);
                s.subsampling_y = if s.subsampling_x != 0 { b.f(1) } else { 0 };
            } else {
                s.subsampling_x = 1;
                s.subsampling_y = 0;
            }
            if s.subsampling_x != 0 && s.subsampling_y != 0 {
                b.f(2); // chroma_sample_position
            }
        }
        s.separate_uv_delta_q = b.flag();
    }
    s.film_grain_params_present = b.flag();
    if b.overrun {
        return err("sequence header truncated");
    }
    Ok(s)
}

#[derive(Clone, Copy)]
struct RefSlot {
    valid: bool,
    frame_id: u32,
    frame_type: u32,
    order_hint: u32,
    showable: bool,
    upscaled_width: u32,
    frame_width: u32,
    frame_height: u32,
    render_width: u32,
    render_height: u32,
    gm_params: [[i32; 6]; 8],
    lf_ref_deltas: [i8; 8],
    lf_mode_deltas: [i8; 2],
    feature_enabled: [[bool; 8]; 8],
    feature_data: [[i16; 8]; 8],
    fg: Grain,
}

fn default_gm() -> [[i32; 6]; 8] {
    let mut g = [[0i32; 6]; 8];
    for r in g.iter_mut() {
        for (i, v) in r.iter_mut().enumerate() {
            *v = if i % 3 == 2 { 1 << WARPEDMODEL_PREC_BITS } else { 0 };
        }
    }
    g
}

impl Default for RefSlot {
    fn default() -> Self {
        RefSlot {
            valid: false,
            frame_id: 0,
            frame_type: 0,
            order_hint: 0,
            showable: false,
            upscaled_width: 0,
            frame_width: 0,
            frame_height: 0,
            render_width: 0,
            render_height: 0,
            gm_params: default_gm(),
            lf_ref_deltas: [0; 8],
            lf_mode_deltas: [0; 2],
            feature_enabled: [[false; 8]; 8],
            feature_data: [[0; 8]; 8],
            fg: Grain::default(),
        }
    }
}

#[derive(Clone, Default)]
struct Hdr {
    show_existing_frame: bool,
    frame_to_show_map_idx: usize,
    frame_type: u32,
    show_frame: bool,
    showable_frame: bool,
    error_resilient_mode: bool,
    allow_screen_content_tools: u32,
    force_integer_mv: u32,
    current_frame_id: u32,
    frame_size_override_flag: bool,
    order_hint: u32,
    primary_ref_frame: u32,
    refresh_frame_flags: u32,
    ref_frame_idx: [usize; 7],
    frame_width: u32,
    frame_height: u32,
    upscaled_width: u32,
    render_width: u32,
    render_height: u32,
    allow_intrabc: bool,
    allow_high_precision_mv: bool,
    tile_cols: u32,
    tile_rows: u32,
    tile_cols_log2: u32,
    tile_rows_log2: u32,
    gm_params: [[i32; 6]; 8],
    fg: Grain,
}

/// Parses an AV1 stream one temporal unit at a time; see the module documentation.
#[derive(Clone)]
pub struct Av1Parser {
    seq: Option<Seq>,
    op_idc: u32,
    refs: [RefSlot; NUM_REF_FRAMES],
    seen_frame_header: bool,
    cur: Hdr,
    lf_ref_deltas: [i8; 8],
    lf_mode_deltas: [i8; 2],
    feature_enabled: [[bool; 8]; 8],
    feature_data: [[i16; 8]; 8],
    prev_gm_params: [[i32; 6]; 8],
}

impl Default for Av1Parser {
    fn default() -> Self {
        Self::new()
    }
}

/// One frame shown by a temporal unit, with its film grain as a decoder applies it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShownFrame {
    pub grain: Option<FilmGrainParams>,
    pub width: u32,
    pub height: u32,
}

impl Av1Parser {
    pub fn new() -> Self {
        let mut p = Av1Parser {
            seq: None,
            op_idc: 0,
            refs: [RefSlot::default(); NUM_REF_FRAMES],
            seen_frame_header: false,
            cur: Hdr::default(),
            lf_ref_deltas: [0; 8],
            lf_mode_deltas: [0; 2],
            feature_enabled: [[false; 8]; 8],
            feature_data: [[0; 8]; 8],
            prev_gm_params: default_gm(),
        };
        p.setup_past_independence();
        p
    }

    /// bit depth, layout and matrix of the stream (after its sequence header)
    pub fn sequence(&self) -> Option<(u32, u32, u32, bool, u32)> {
        self.seq.as_ref().map(|s| (s.bit_depth, s.subsampling_x, s.subsampling_y, s.mono_chrome, s.matrix_coefficients))
    }

    fn setup_past_independence(&mut self) {
        self.feature_enabled = [[false; 8]; 8];
        self.feature_data = [[0; 8]; 8];
        self.prev_gm_params = default_gm();
        self.lf_ref_deltas = [1, 0, 0, 0, -1, 0, -1, -1];
        self.lf_mode_deltas = [0, 0];
    }

    fn load_previous(&mut self, slot: usize) {
        let r = &self.refs[slot];
        self.prev_gm_params = r.gm_params;
        self.lf_ref_deltas = r.lf_ref_deltas;
        self.lf_mode_deltas = r.lf_mode_deltas;
        self.feature_enabled = r.feature_enabled;
        self.feature_data = r.feature_data;
    }

    fn relative_dist(&self, a: u32, b: u32) -> i32 {
        let s = self.seq.as_ref().unwrap();
        if !s.enable_order_hint {
            return 0;
        }
        let diff = a as i32 - b as i32;
        let m = 1i32 << (s.order_hint_bits - 1);
        (diff & (m - 1)) - (diff & m)
    }

    /// Parse one temporal unit: the OBUs of one IVF frame or container packet (each with obu_size). Returns the
    /// frames it shows (one, for streams without spatial layers).
    pub fn temporal_unit(&mut self, data: &[u8]) -> Result<Vec<ShownFrame>, ParamError> {
        let mut out = vec![];
        let mut pos = 0;
        while pos < data.len() {
            let h0 = data[pos];
            if h0 & 0x80 != 0 {
                return err("obu_forbidden_bit set (not an AV1 OBU stream?)");
            }
            let ty = (h0 >> 3) & 15;
            let ext = (h0 >> 2) & 1 != 0;
            let has_size = (h0 >> 1) & 1 != 0;
            let mut p = pos + 1;
            let (mut tid, mut sid) = (0, 0);
            if ext {
                let e = *data.get(p).ok_or_else(|| ParamError("truncated OBU extension".into()))?;
                tid = (e >> 5) as u32;
                sid = ((e >> 3) & 3) as u32;
                p += 1;
            }
            let len = if has_size {
                let mut v = 0u64;
                let mut i = 0;
                loop {
                    let byte = *data.get(p + i).ok_or_else(|| ParamError("bad obu_size".into()))?;
                    v |= ((byte & 0x7f) as u64) << (7 * i);
                    i += 1;
                    if byte & 0x80 == 0 {
                        break;
                    }
                    if i >= 8 {
                        return err("bad obu_size");
                    }
                }
                p += i;
                v as usize
            } else {
                data.len() - p
            };
            if p.checked_add(len).is_none_or(|end| end > data.len()) {
                return err("OBU larger than its temporal unit");
            }
            let payload = &data[p..p + len];
            pos = p + len;
            // operating point 0: drop OBUs of other layers
            if ty != 1 && ty != 2 && ext && self.seq.is_some() && self.op_idc != 0 {
                let in_t = (self.op_idc >> tid) & 1;
                let in_s = (self.op_idc >> (sid + 8)) & 1;
                if in_t == 0 || in_s == 0 {
                    continue;
                }
            }
            match ty {
                1 => {
                    let s = parse_sequence_header(payload)?;
                    self.op_idc = s.operating_point_idc[0];
                    self.seq = Some(s);
                }
                2 => self.seen_frame_header = false,
                3 | 6 | 7 => {
                    if self.seq.is_none() {
                        return err("frame header before the first sequence header");
                    }
                    let mut b = Bits::new(payload);
                    if self.seen_frame_header {
                        // frame_header_copy(): nothing new
                        if ty == 6 {
                            return err("OBU_FRAME after a frame header");
                        }
                        continue;
                    }
                    let save = self.clone();
                    let r = self.frame_header(&mut b, tid, sid);
                    match r {
                        Err(e) => {
                            *self = save;
                            return Err(e);
                        }
                        Ok(()) => {}
                    }
                    if b.overrun {
                        *self = save;
                        return err("frame header truncated");
                    }
                    let h = &self.cur;
                    if h.show_existing_frame || h.show_frame {
                        out.push(ShownFrame { grain: h.fg.to_params(), width: h.upscaled_width, height: h.frame_height });
                    }
                    if ty == 6 {
                        if self.cur.show_existing_frame {
                            return err("OBU_FRAME with show_existing_frame");
                        }
                        b.byte_alignment();
                        self.tile_group(&mut b)?;
                    }
                }
                4 => {
                    let mut b = Bits::new(payload);
                    self.tile_group(&mut b)?;
                }
                _ => {}
            }
        }
        Ok(out)
    }

    fn tile_group(&mut self, b: &mut Bits) -> Result<(), ParamError> {
        if !self.seen_frame_header {
            return Ok(());
        }
        let h = &self.cur;
        let num_tiles = h.tile_cols * h.tile_rows;
        let mut tg_end = num_tiles - 1;
        if num_tiles > 1 && b.flag() {
            let bits = h.tile_cols_log2 + h.tile_rows_log2;
            b.f(bits);
            tg_end = b.f(bits);
        }
        if b.overrun {
            return err("tile group header truncated");
        }
        if tg_end == num_tiles - 1 {
            self.reference_update();
            self.seen_frame_header = false;
        }
        Ok(())
    }

    fn reference_update(&mut self) {
        let h = self.cur.clone();
        for i in 0..NUM_REF_FRAMES {
            if (h.refresh_frame_flags >> i) & 1 == 0 {
                continue;
            }
            self.refs[i] = RefSlot {
                valid: true,
                frame_id: h.current_frame_id,
                frame_type: h.frame_type,
                order_hint: h.order_hint,
                showable: h.showable_frame,
                upscaled_width: h.upscaled_width,
                frame_width: h.frame_width,
                frame_height: h.frame_height,
                render_width: h.render_width,
                render_height: h.render_height,
                gm_params: h.gm_params,
                lf_ref_deltas: self.lf_ref_deltas,
                lf_mode_deltas: self.lf_mode_deltas,
                feature_enabled: self.feature_enabled,
                feature_data: self.feature_data,
                fg: h.fg,
            };
        }
    }

    fn superres_params(&self, b: &mut Bits, h: &mut Hdr) {
        let s = self.seq.as_ref().unwrap();
        let use_superres = s.enable_superres && b.flag();
        let denom = if use_superres { b.f(3) + 9 } else { 8 };
        h.upscaled_width = h.frame_width;
        h.frame_width = (h.upscaled_width * 8 + denom / 2) / denom;
    }

    fn frame_size(&self, b: &mut Bits, h: &mut Hdr) {
        let s = self.seq.as_ref().unwrap();
        if h.frame_size_override_flag {
            h.frame_width = b.f(s.frame_width_bits_minus_1 + 1) + 1;
            h.frame_height = b.f(s.frame_height_bits_minus_1 + 1) + 1;
        } else {
            h.frame_width = s.max_frame_width_minus_1 + 1;
            h.frame_height = s.max_frame_height_minus_1 + 1;
        }
        self.superres_params(b, h);
    }

    fn render_size(b: &mut Bits, h: &mut Hdr) {
        if b.flag() {
            h.render_width = b.f(16) + 1;
            h.render_height = b.f(16) + 1;
        } else {
            h.render_width = h.upscaled_width;
            h.render_height = h.frame_height;
        }
    }

    fn tile_info(&self, b: &mut Bits, h: &mut Hdr) {
        let s = self.seq.as_ref().unwrap();
        let tile_log2 = |blk: u32, target: u32| {
            let mut k = 0;
            while (blk << k) < target {
                k += 1;
            }
            k
        };
        let mi_cols = 2 * ((h.frame_width + 7) >> 3);
        let mi_rows = 2 * ((h.frame_height + 7) >> 3);
        let sb128 = s.use_128x128_superblock;
        let sb_cols = if sb128 { (mi_cols + 31) >> 5 } else { (mi_cols + 15) >> 4 };
        let sb_rows = if sb128 { (mi_rows + 31) >> 5 } else { (mi_rows + 15) >> 4 };
        let sb_size = if sb128 { 7 } else { 6 };
        let max_tile_width_sb = 4096u32 >> sb_size;
        let mut max_tile_area_sb = (4096u32 * 2304) >> (2 * sb_size);
        let min_log2_tile_cols = tile_log2(max_tile_width_sb, sb_cols);
        let max_log2_tile_cols = tile_log2(1, sb_cols.min(64));
        let max_log2_tile_rows = tile_log2(1, sb_rows.min(64));
        let min_log2_tiles = min_log2_tile_cols.max(tile_log2(max_tile_area_sb, sb_rows * sb_cols));
        if b.flag() {
            h.tile_cols_log2 = min_log2_tile_cols;
            while h.tile_cols_log2 < max_log2_tile_cols && !b.overrun {
                if b.flag() {
                    h.tile_cols_log2 += 1;
                } else {
                    break;
                }
            }
            let w = (sb_cols + (1 << h.tile_cols_log2) - 1) >> h.tile_cols_log2;
            h.tile_cols = sb_cols.div_ceil(w);
            let min_log2_tile_rows = min_log2_tiles.saturating_sub(h.tile_cols_log2);
            h.tile_rows_log2 = min_log2_tile_rows;
            while h.tile_rows_log2 < max_log2_tile_rows && !b.overrun {
                if b.flag() {
                    h.tile_rows_log2 += 1;
                } else {
                    break;
                }
            }
            let hgt = (sb_rows + (1 << h.tile_rows_log2) - 1) >> h.tile_rows_log2;
            h.tile_rows = sb_rows.div_ceil(hgt);
        } else {
            let (mut widest, mut start, mut i) = (0, 0, 0);
            while start < sb_cols && !b.overrun {
                let max_width = (sb_cols - start).min(max_tile_width_sb);
                let size = b.ns(max_width) + 1;
                widest = widest.max(size);
                start += size;
                i += 1;
            }
            h.tile_cols = i;
            h.tile_cols_log2 = tile_log2(1, h.tile_cols);
            max_tile_area_sb = if min_log2_tiles > 0 { (sb_rows * sb_cols) >> (min_log2_tiles + 1) } else { sb_rows * sb_cols };
            let max_tile_height_sb = if widest > 0 { (max_tile_area_sb / widest).max(1) } else { 1 };
            start = 0;
            i = 0;
            while start < sb_rows && !b.overrun {
                let max_height = (sb_rows - start).min(max_tile_height_sb);
                start += b.ns(max_height) + 1;
                i += 1;
            }
            h.tile_rows = i;
            h.tile_rows_log2 = tile_log2(1, h.tile_rows);
        }
        if h.tile_cols_log2 > 0 || h.tile_rows_log2 > 0 {
            b.f(h.tile_rows_log2 + h.tile_cols_log2); // context_update_tile_id
            b.f(2); // tile_size_bytes_minus_1
        }
    }

    fn set_frame_refs(&self, h: &mut Hdr, last: usize, gold: usize) {
        let s = self.seq.as_ref().unwrap();
        let mut idx = [-1i32; 7];
        let mut used = [false; 8];
        idx[0] = last as i32;
        idx[3] = gold as i32;
        used[last] = true;
        used[gold] = true;
        let cur = 1i32 << (s.order_hint_bits - 1);
        let shifted: Vec<i32> = (0..8).map(|i| cur + self.relative_dist(self.refs[i].order_hint, h.order_hint)).collect();
        let mut best = 0;
        // ALTREF: latest backward
        let mut r = -1i32;
        for i in 0..8 {
            if !used[i] && shifted[i] >= cur && (r < 0 || shifted[i] >= best) {
                r = i as i32;
                best = shifted[i];
            }
        }
        if r >= 0 {
            idx[6] = r;
            used[r as usize] = true;
        }
        // BWDREF, ALTREF2: earliest backward
        for k in 0..2 {
            r = -1;
            for i in 0..8 {
                if !used[i] && shifted[i] >= cur && (r < 0 || shifted[i] < best) {
                    r = i as i32;
                    best = shifted[i];
                }
            }
            if r >= 0 {
                idx[if k == 0 { 4 } else { 5 }] = r;
                used[r as usize] = true;
            }
        }
        // the rest: latest forward
        for &rf in &[1usize, 2, 4, 5, 6] {
            if idx[rf] >= 0 {
                continue;
            }
            r = -1;
            for i in 0..8 {
                if !used[i] && shifted[i] < cur && (r < 0 || shifted[i] >= best) {
                    r = i as i32;
                    best = shifted[i];
                }
            }
            if r >= 0 {
                idx[rf] = r;
                used[r as usize] = true;
            }
        }
        r = -1;
        for i in 0..8 {
            if r < 0 || shifted[i] < best {
                r = i as i32;
                best = shifted[i];
            }
        }
        for i in 0..REFS_PER_FRAME {
            h.ref_frame_idx[i] = if idx[i] < 0 { r as usize } else { idx[i] as usize };
        }
    }

    fn read_global_param(&self, b: &mut Bits, h: &mut Hdr, ty: u32, rf: usize, idx: usize) {
        let (mut abs_bits, mut prec_bits) = (12u32, 15u32);
        if idx < 2 {
            if ty == 1 {
                let hp = !h.allow_high_precision_mv as u32;
                abs_bits = 9 - hp;
                prec_bits = 3 - hp;
            } else {
                abs_bits = 12;
                prec_bits = 6;
            }
        }
        let prec_diff = WARPEDMODEL_PREC_BITS - prec_bits;
        let round = if idx % 3 == 2 { 1i32 << WARPEDMODEL_PREC_BITS } else { 0 };
        let sub = if idx % 3 == 2 { 1i32 << prec_bits } else { 0 };
        let mx = 1i32 << abs_bits;
        let r = (self.prev_gm_params[rf][idx] >> prec_diff) - sub;
        let v = decode_signed_subexp_with_ref(b, -mx, mx + 1, r);
        h.gm_params[rf][idx] = ((v as u32) << prec_diff) as i32 + round;
    }

    fn film_grain_params(&self, b: &mut Bits, h: &mut Hdr) -> Result<(), ParamError> {
        let s = self.seq.as_ref().unwrap();
        h.fg = Grain::default();
        if !s.film_grain_params_present || (!h.show_frame && !h.showable_frame) {
            return Ok(());
        }
        let g = &mut h.fg;
        g.apply_grain = b.flag();
        if !g.apply_grain {
            *g = Grain::default();
            return Ok(());
        }
        let seed = b.f(16) as u16;
        g.grain_seed = seed;
        let update = if h.frame_type == INTER_FRAME { b.flag() } else { true };
        if !update {
            let idx = b.f(3) as usize;
            if !h.ref_frame_idx.contains(&idx) {
                return err(format!("film_grain_params_ref_idx {idx} is not a reference of this frame"));
            }
            if !self.refs[idx].valid {
                return err(format!("film_grain_params_ref_idx {idx} names an empty reference slot"));
            }
            // load_grain_params(): the slot's parameters with this frame's seed. A slot without grain gives all-zero
            // parameters: grain stays "present" (dav1d keeps film_grain.present and exports the seed) but draws nothing
            *g = self.refs[idx].fg;
            g.apply_grain = true;
            g.grain_seed = seed;
            return Ok(());
        }
        g.num_y_points = b.f(4);
        if g.num_y_points > 14 {
            return err("num_y_points > 14");
        }
        for i in 0..g.num_y_points as usize {
            g.y_points[i] = [b.f(8) as u8, b.f(8) as u8];
        }
        g.chroma_scaling_from_luma = if s.mono_chrome { false } else { b.flag() };
        if s.mono_chrome || g.chroma_scaling_from_luma || (s.subsampling_x == 1 && s.subsampling_y == 1 && g.num_y_points == 0) {
            g.num_cb_points = 0;
            g.num_cr_points = 0;
        } else {
            g.num_cb_points = b.f(4);
            if g.num_cb_points > 10 {
                return err("num_cb_points > 10");
            }
            for i in 0..g.num_cb_points as usize {
                g.cb_points[i] = [b.f(8) as u8, b.f(8) as u8];
            }
            g.num_cr_points = b.f(4);
            if g.num_cr_points > 10 {
                return err("num_cr_points > 10");
            }
            for i in 0..g.num_cr_points as usize {
                g.cr_points[i] = [b.f(8) as u8, b.f(8) as u8];
            }
        }
        g.grain_scaling_minus_8 = b.f(2);
        g.ar_coeff_lag = b.f(2);
        let npos = 2 * g.ar_coeff_lag as usize * (g.ar_coeff_lag as usize + 1);
        let mut nuv = npos;
        if g.num_y_points > 0 {
            nuv = npos + 1;
            for i in 0..npos {
                g.ar_coeffs_y_plus_128[i] = b.f(8) as u8;
            }
        }
        if g.chroma_scaling_from_luma || g.num_cb_points > 0 {
            for i in 0..nuv {
                g.ar_coeffs_cb_plus_128[i] = b.f(8) as u8;
            }
        }
        if g.chroma_scaling_from_luma || g.num_cr_points > 0 {
            for i in 0..nuv {
                g.ar_coeffs_cr_plus_128[i] = b.f(8) as u8;
            }
        }
        g.ar_coeff_shift_minus_6 = b.f(2);
        g.grain_scale_shift = b.f(2);
        if g.num_cb_points > 0 {
            g.cb_mult = b.f(8);
            g.cb_luma_mult = b.f(8);
            g.cb_offset = b.f(9);
        }
        if g.num_cr_points > 0 {
            g.cr_mult = b.f(8);
            g.cr_luma_mult = b.f(8);
            g.cr_offset = b.f(9);
        }
        g.overlap_flag = b.flag();
        g.clip_to_restricted_range = b.flag();
        Ok(())
    }

    fn frame_header(&mut self, b: &mut Bits, tid: u32, sid: u32) -> Result<(), ParamError> {
        let s = self.seq.clone().unwrap();
        let mut h = Hdr::default();
        let id_len = if s.frame_id_numbers_present {
            s.additional_frame_id_length_minus_1 + s.delta_frame_id_length_minus_2 + 3
        } else {
            0
        };
        let all_frames = (1u32 << NUM_REF_FRAMES) - 1;
        let frame_is_intra;
        if s.still_picture_reduced {
            h.frame_type = KEY_FRAME;
            frame_is_intra = true;
            h.show_frame = true;
            h.error_resilient_mode = true;
        } else {
            h.show_existing_frame = b.flag();
            if h.show_existing_frame {
                h.frame_to_show_map_idx = b.f(3) as usize;
                if s.decoder_model_info_present && !s.equal_picture_interval {
                    b.f(s.frame_presentation_time_length_minus_1 + 1);
                }
                if s.frame_id_numbers_present {
                    b.f(id_len);
                }
                let r = self.refs[h.frame_to_show_map_idx];
                if !r.valid {
                    return err(format!("show_existing_frame of an empty slot {}", h.frame_to_show_map_idx));
                }
                h.frame_type = r.frame_type;
                h.refresh_frame_flags = if h.frame_type == KEY_FRAME { all_frames } else { 0 };
                h.show_frame = true;
                h.showable_frame = r.showable;
                h.order_hint = r.order_hint;
                h.upscaled_width = r.upscaled_width;
                h.frame_width = r.frame_width;
                h.frame_height = r.frame_height;
                h.render_width = r.render_width;
                h.render_height = r.render_height;
                h.current_frame_id = r.frame_id;
                if s.film_grain_params_present {
                    h.fg = r.fg;
                }
                // a shown key frame is loaded and refreshes every slot
                if h.frame_type == KEY_FRAME {
                    for i in 0..NUM_REF_FRAMES {
                        self.refs[i] = r;
                    }
                }
                self.cur = h;
                self.seen_frame_header = false;
                return Ok(());
            }
            h.frame_type = b.f(2);
            frame_is_intra = h.frame_type == INTRA_ONLY_FRAME || h.frame_type == KEY_FRAME;
            h.show_frame = b.flag();
            if h.show_frame && s.decoder_model_info_present && !s.equal_picture_interval {
                b.f(s.frame_presentation_time_length_minus_1 + 1);
            }
            h.showable_frame = if h.show_frame { h.frame_type != KEY_FRAME } else { b.flag() };
            h.error_resilient_mode =
                if h.frame_type == SWITCH_FRAME || (h.frame_type == KEY_FRAME && h.show_frame) { true } else { b.flag() };
        }
        if h.frame_type == KEY_FRAME && h.show_frame {
            for r in self.refs.iter_mut() {
                r.valid = false;
                r.order_hint = 0;
            }
        }
        let disable_cdf_update = b.flag();
        h.allow_screen_content_tools =
            if s.seq_force_screen_content_tools == SELECT { b.f(1) } else { s.seq_force_screen_content_tools };
        h.force_integer_mv = if h.allow_screen_content_tools != 0 {
            if s.seq_force_integer_mv == SELECT { b.f(1) } else { s.seq_force_integer_mv }
        } else {
            0
        };
        if frame_is_intra {
            h.force_integer_mv = 1;
        }
        if s.frame_id_numbers_present {
            h.current_frame_id = b.f(id_len);
            // mark_ref_frames()
            let diff_len = s.delta_frame_id_length_minus_2 + 2;
            if !(h.frame_type == KEY_FRAME && h.show_frame) {
                for r in self.refs.iter_mut() {
                    let (cid, rid) = (h.current_frame_id as i64, r.frame_id as i64);
                    if cid > (1i64 << diff_len) {
                        if rid > cid || rid < cid - (1i64 << diff_len) {
                            r.valid = false;
                        }
                    } else if rid > cid && rid < (1i64 << id_len) + cid - (1i64 << diff_len) {
                        r.valid = false;
                    }
                }
            }
        }
        h.frame_size_override_flag = if h.frame_type == SWITCH_FRAME {
            true
        } else if s.still_picture_reduced {
            false
        } else {
            b.flag()
        };
        h.order_hint = b.f(s.order_hint_bits);
        h.primary_ref_frame = if frame_is_intra || h.error_resilient_mode { PRIMARY_REF_NONE } else { b.f(3) };
        if s.decoder_model_info_present && b.flag() {
            for op in 0..=s.operating_points_cnt_minus_1 {
                if !s.decoder_model_present_for_this_op[op] {
                    continue;
                }
                let idc = s.operating_point_idc[op];
                let in_t = (idc >> tid) & 1;
                let in_s = (idc >> (sid + 8)) & 1;
                if idc == 0 || (in_t != 0 && in_s != 0) {
                    b.f(s.buffer_removal_time_length_minus_1 + 1);
                }
            }
        }
        h.refresh_frame_flags =
            if h.frame_type == SWITCH_FRAME || (h.frame_type == KEY_FRAME && h.show_frame) { all_frames } else { b.f(8) };
        if (!frame_is_intra || h.refresh_frame_flags != all_frames) && h.error_resilient_mode && s.enable_order_hint {
            for r in self.refs.iter_mut() {
                let roh = b.f(s.order_hint_bits);
                if roh != r.order_hint || !r.valid {
                    r.order_hint = roh;
                    r.valid = false;
                }
            }
        }
        if frame_is_intra {
            self.frame_size(b, &mut h);
            Self::render_size(b, &mut h);
            if h.allow_screen_content_tools != 0 && h.upscaled_width == h.frame_width {
                h.allow_intrabc = b.flag();
            }
        } else {
            let mut short_signaling = false;
            if s.enable_order_hint {
                short_signaling = b.flag();
                if short_signaling {
                    let last = b.f(3) as usize;
                    let gold = b.f(3) as usize;
                    self.set_frame_refs(&mut h, last, gold);
                }
            }
            for i in 0..REFS_PER_FRAME {
                if !short_signaling {
                    h.ref_frame_idx[i] = b.f(3) as usize;
                }
                if s.frame_id_numbers_present {
                    b.f(s.delta_frame_id_length_minus_2 + 2);
                }
            }
            if h.frame_size_override_flag && !h.error_resilient_mode {
                let mut found = false;
                for i in 0..REFS_PER_FRAME {
                    if b.overrun {
                        break;
                    }
                    if b.flag() {
                        let r = self.refs[h.ref_frame_idx[i]];
                        h.upscaled_width = r.upscaled_width;
                        h.frame_width = h.upscaled_width;
                        h.frame_height = r.frame_height;
                        h.render_width = r.render_width;
                        h.render_height = r.render_height;
                        found = true;
                        break;
                    }
                }
                if !found {
                    self.frame_size(b, &mut h);
                    Self::render_size(b, &mut h);
                } else {
                    self.superres_params(b, &mut h);
                }
            } else {
                self.frame_size(b, &mut h);
                Self::render_size(b, &mut h);
            }
            h.allow_high_precision_mv = if h.force_integer_mv != 0 { false } else { b.flag() };
            if !b.flag() {
                b.f(2); // interpolation_filter (is_filter_switchable = 0)
            }
            b.f(1); // is_motion_mode_switchable
            if !(h.error_resilient_mode || !s.enable_ref_frame_mvs) {
                b.f(1); // use_ref_frame_mvs
            }
        }
        if h.frame_width == 0 || h.frame_height == 0 {
            return err("frame size 0");
        }
        if !(s.still_picture_reduced || disable_cdf_update) {
            b.f(1); // disable_frame_end_update_cdf
        }
        if h.primary_ref_frame == PRIMARY_REF_NONE {
            self.setup_past_independence();
        } else {
            self.load_previous(h.ref_frame_idx[h.primary_ref_frame as usize]);
        }
        self.tile_info(b, &mut h);
        // quantization_params()
        let base_q_idx = b.f(8);
        let dq = |b: &mut Bits| if b.flag() { b.su(7) } else { 0 };
        let dq_y_dc = dq(b);
        let (mut u_dc, mut u_ac, mut v_dc, mut v_ac) = (0, 0, 0, 0);
        if s.num_planes > 1 {
            let diff_uv_delta = s.separate_uv_delta_q && b.flag();
            u_dc = dq(b);
            u_ac = dq(b);
            if diff_uv_delta {
                v_dc = dq(b);
                v_ac = dq(b);
            } else {
                v_dc = u_dc;
                v_ac = u_ac;
            }
        }
        if b.flag() {
            // using_qmatrix
            b.f(4);
            b.f(4);
            if s.separate_uv_delta_q {
                b.f(4);
            }
        }
        // segmentation_params()
        const SEG_BITS: [u32; 8] = [8, 6, 6, 6, 6, 3, 0, 0];
        const SEG_SIGNED: [bool; 8] = [true, true, true, true, true, false, false, false];
        const SEG_MAX: [i32; 8] = [255, 63, 63, 63, 63, 7, 0, 0];
        let segmentation_enabled = b.flag();
        if segmentation_enabled {
            let mut update_data = true;
            if h.primary_ref_frame != PRIMARY_REF_NONE {
                if b.flag() {
                    b.f(1); // segmentation_temporal_update
                }
                update_data = b.flag();
            }
            if update_data {
                for i in 0..8 {
                    for j in 0..8 {
                        let mut v = 0;
                        let en = b.flag();
                        self.feature_enabled[i][j] = en;
                        if en {
                            let lim = SEG_MAX[j];
                            if SEG_SIGNED[j] {
                                v = b.su(1 + SEG_BITS[j]).clamp(-lim, lim);
                            } else {
                                v = (b.f(SEG_BITS[j]) as i32).min(lim);
                            }
                        }
                        self.feature_data[i][j] = v as i16;
                    }
                }
            }
        } else {
            self.feature_enabled = [[false; 8]; 8];
            self.feature_data = [[0; 8]; 8];
        }
        // delta_q_params(), delta_lf_params()
        let delta_q_present = base_q_idx > 0 && b.flag();
        if delta_q_present {
            b.f(2);
            if !h.allow_intrabc && b.flag() {
                b.f(2);
                b.f(1);
            }
        }
        let mut coded_lossless = true;
        for seg in 0..8 {
            let mut q = base_q_idx as i32;
            if segmentation_enabled && self.feature_enabled[seg][0] {
                q = (q + self.feature_data[seg][0] as i32).clamp(0, 255);
            }
            if !(q == 0 && dq_y_dc == 0 && u_ac == 0 && u_dc == 0 && v_ac == 0 && v_dc == 0) {
                coded_lossless = false;
            }
        }
        let all_lossless = coded_lossless && h.frame_width == h.upscaled_width;
        // loop_filter_params()
        if coded_lossless || h.allow_intrabc {
            self.lf_ref_deltas = [1, 0, 0, 0, -1, 0, -1, -1];
            self.lf_mode_deltas = [0, 0];
        } else {
            let l0 = b.f(6);
            let l1 = b.f(6);
            if s.num_planes > 1 && (l0 != 0 || l1 != 0) {
                b.f(6);
                b.f(6);
            }
            b.f(3); // sharpness
            if b.flag() && b.flag() {
                // loop_filter_delta_enabled, loop_filter_delta_update
                for i in 0..8 {
                    if b.flag() {
                        self.lf_ref_deltas[i] = b.su(7) as i8;
                    }
                }
                for i in 0..2 {
                    if b.flag() {
                        self.lf_mode_deltas[i] = b.su(7) as i8;
                    }
                }
            }
        }
        // cdef_params()
        if !(coded_lossless || h.allow_intrabc || !s.enable_cdef) {
            b.f(2);
            let cdef_bits = b.f(2);
            for _ in 0..(1 << cdef_bits) {
                b.f(4);
                b.f(2);
                if s.num_planes > 1 {
                    b.f(4);
                    b.f(2);
                }
            }
        }
        // lr_params()
        if !(all_lossless || h.allow_intrabc || !s.enable_restoration) {
            let (mut uses_lr, mut uses_chroma_lr) = (false, false);
            for i in 0..s.num_planes {
                if b.f(2) != 0 {
                    uses_lr = true;
                    if i > 0 {
                        uses_chroma_lr = true;
                    }
                }
            }
            if uses_lr {
                if s.use_128x128_superblock {
                    b.f(1);
                } else if b.flag() {
                    b.f(1);
                }
                if s.subsampling_x != 0 && s.subsampling_y != 0 && uses_chroma_lr {
                    b.f(1);
                }
            }
        }
        // read_tx_mode()
        if !coded_lossless {
            b.f(1);
        }
        // frame_reference_mode()
        let reference_select = !frame_is_intra && b.flag();
        // skip_mode_params()
        let mut skip_mode_allowed = false;
        if !(frame_is_intra || !reference_select || !s.enable_order_hint) {
            let (mut fwd, mut bwd) = (-1i32, -1i32);
            let (mut fwd_hint, mut bwd_hint) = (0u32, 0u32);
            for i in 0..REFS_PER_FRAME {
                let rh = self.refs[h.ref_frame_idx[i]].order_hint;
                if self.relative_dist(rh, h.order_hint) < 0 {
                    if fwd < 0 || self.relative_dist(rh, fwd_hint) > 0 {
                        fwd = i as i32;
                        fwd_hint = rh;
                    }
                } else if self.relative_dist(rh, h.order_hint) > 0
                    && (bwd < 0 || self.relative_dist(rh, bwd_hint) < 0) {
                    bwd = i as i32;
                    bwd_hint = rh;
                }
            }
            if fwd < 0 {
                skip_mode_allowed = false;
            } else if bwd >= 0 {
                skip_mode_allowed = true;
            } else {
                let mut fwd2 = -1i32;
                let mut fwd2_hint = 0u32;
                for i in 0..REFS_PER_FRAME {
                    let rh = self.refs[h.ref_frame_idx[i]].order_hint;
                    if self.relative_dist(rh, fwd_hint) < 0 && (fwd2 < 0 || self.relative_dist(rh, fwd2_hint) > 0) {
                        fwd2 = i as i32;
                        fwd2_hint = rh;
                    }
                }
                skip_mode_allowed = fwd2 >= 0;
            }
        }
        if skip_mode_allowed {
            b.f(1);
        }
        if !(frame_is_intra || h.error_resilient_mode || !s.enable_warped_motion) {
            b.f(1); // allow_warped_motion
        }
        b.f(1); // reduced_tx_set
        // global_motion_params()
        h.gm_params = default_gm();
        if !frame_is_intra {
            for rf in 1..=7 {
                let mut ty = 0; // IDENTITY
                if b.flag() {
                    ty = if b.flag() { 2 } else if b.flag() { 1 } else { 3 }; // ROTZOOM, TRANSLATION, AFFINE
                }
                if ty >= 2 {
                    self.read_global_param(b, &mut h, ty, rf, 2);
                    self.read_global_param(b, &mut h, ty, rf, 3);
                    if ty == 3 {
                        self.read_global_param(b, &mut h, ty, rf, 4);
                        self.read_global_param(b, &mut h, ty, rf, 5);
                    } else {
                        h.gm_params[rf][4] = -h.gm_params[rf][3];
                        h.gm_params[rf][5] = h.gm_params[rf][2];
                    }
                }
                if ty >= 1 {
                    self.read_global_param(b, &mut h, ty, rf, 0);
                    self.read_global_param(b, &mut h, ty, rf, 1);
                }
            }
        }
        self.film_grain_params(b, &mut h)?;
        self.cur = h;
        self.seen_frame_header = true;
        Ok(())
    }
}

fn inverse_recenter(r: i32, v: i32) -> i32 {
    if v > 2 * r {
        v
    } else if v & 1 != 0 {
        r - ((v + 1) >> 1)
    } else {
        r + (v >> 1)
    }
}

fn decode_subexp(b: &mut Bits, num_syms: i32) -> i32 {
    let (mut i, mut mk) = (0, 0);
    let k = 3;
    while !b.overrun {
        let b2 = if i > 0 { k + i - 1 } else { k };
        let a = 1 << b2;
        if num_syms <= mk + 3 * a {
            return b.ns((num_syms - mk) as u32) as i32 + mk;
        } else if b.flag() {
            i += 1;
            mk += a;
        } else {
            return b.f(b2 as u32) as i32 + mk;
        }
    }
    0
}

fn decode_signed_subexp_with_ref(b: &mut Bits, low: i32, high: i32, r: i32) -> i32 {
    let mx = high - low;
    let r = r - low;
    let v = decode_subexp(b, mx);
    let x = if (r << 1) <= mx { inverse_recenter(r, v) } else { mx - 1 - inverse_recenter(mx - 1 - r, v) };
    x + low
}

/// Reads an IVF stream: the 32-byte file header, then (12-byte header + data) per frame.
pub struct IvfReader<R: std::io::Read> {
    inner: R,
}

impl<R: std::io::Read> IvfReader<R> {
    pub fn new(mut inner: R) -> std::io::Result<Self> {
        let mut h = [0u8; 32];
        inner.read_exact(&mut h)?;
        if &h[0..4] != b"DKIF" {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "not an IVF stream"));
        }
        let hl = u16::from_le_bytes([h[6], h[7]]) as usize;
        if hl > 32 {
            let mut skip = vec![0u8; hl - 32];
            inner.read_exact(&mut skip)?;
        }
        Ok(IvfReader { inner })
    }

    /// the next frame (temporal unit), or None at the end
    pub fn next_frame(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        let mut h = [0u8; 12];
        let mut got = 0;
        while got < 12 {
            let k = self.inner.read(&mut h[got..])?;
            if k == 0 {
                break;
            }
            got += k;
        }
        if got == 0 {
            return Ok(None);
        }
        if got < 12 {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "truncated IVF frame header"));
        }
        let n = u32::from_le_bytes([h[0], h[1], h[2], h[3]]) as usize;
        let mut v = vec![0u8; n];
        self.inner.read_exact(&mut v)?;
        Ok(Some(v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The parser's grain equals PARAMS.json written by `av1grain strip` (dav1d's own frame header parser), picture by
    /// picture. AV1FG_TEST_PARAMS names a directory of <stream>.ivf + <stream>.json pairs (CI makes one from the
    /// decoder's test streams and gpu/tests/make-vectors.sh).
    #[test]
    #[ignore = "needs AV1FG_TEST_PARAMS=DIR with <stream>.ivf + <stream>.json (av1grain strip) pairs; run with --ignored"]
    fn matches_av1grain() {
        let dir = std::env::var_os("AV1FG_TEST_PARAMS")
            .expect("AV1FG_TEST_PARAMS: a directory with <stream>.ivf + <stream>.json (av1grain strip) pairs");
        let (mut n, mut pics) = (0, 0);
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let ivf = p.with_extension("ivf");
            let f = std::fs::File::open(&ivf).unwrap_or_else(|e| panic!("{}: {e}", ivf.display()));
            let want = crate::params::ParamsFile::parse(&std::fs::read_to_string(&p).unwrap()).unwrap();
            let mut r = IvfReader::new(std::io::BufReader::new(f)).unwrap();
            let mut parser = Av1Parser::new();
            let mut i = 0;
            while let Some(tu) = r.next_frame().unwrap() {
                for s in parser.temporal_unit(&tu).unwrap() {
                    let got = s.grain.filter(|g| g.has_grain());
                    assert_eq!(got, want.picture(i).filter(|g| g.has_grain()), "{} picture {i}", ivf.display());
                    i += 1;
                }
            }
            assert_eq!(i, want.grain.len(), "{}", ivf.display());
            n += 1;
            pics += i;
        }
        assert!(n > 0, "no <stream>.json + <stream>.ivf pairs in AV1FG_TEST_PARAMS");
        eprintln!("{n} streams, {pics} pictures");
    }

    fn tus(ivf: &[u8]) -> Vec<Vec<u8>> {
        let mut r = IvfReader::new(ivf).unwrap();
        let mut v = vec![];
        while let Some(t) = r.next_frame().unwrap() {
            v.push(t);
        }
        v
    }

    /// Mutated real streams and random OBUs: the parser returns Ok or Err, never panics (run without --release too:
    /// debug builds check every addition and shift for overflow). AV1FG_FUZZ_ITERS sets the number of runs.
    #[test]
    fn fuzz_headers() {
        let seeds = [
            tus(include_bytes!("../tests/data/fgt15_420_10bit_csfl.ivf")),
            tus(include_bytes!("../tests/data/tv05_420_8.ivf")),
            tus(include_bytes!("../tests/data/table_420_10.ivf")),
        ];
        let iters: usize = std::env::var("AV1FG_FUZZ_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(3000);
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        let mut rnd = move |n: usize| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x % n.max(1) as u64) as usize
        };
        let (mut ok, mut errs) = (0usize, 0usize);
        for _ in 0..iters {
            let mut s = seeds[rnd(seeds.len())].clone();
            for _ in 0..1 + rnd(8) {
                let t = rnd(s.len());
                let tu = &mut s[t];
                match rnd(6) {
                    // bit flips, mostly in the headers at the start of a temporal unit
                    0..=2 if !tu.is_empty() => {
                        let at = if rnd(4) > 0 { rnd(tu.len().min(96)) } else { rnd(tu.len()) };
                        tu[at] ^= 1u8 << rnd(8);
                    }
                    3 if !tu.is_empty() => {
                        let at = rnd(tu.len());
                        tu[at] = rnd(256) as u8;
                    }
                    4 => {
                        let l = rnd(tu.len() + 1);
                        tu.truncate(l);
                    }
                    _ => {
                        if rnd(2) == 0 && s.len() > 1 {
                            s.remove(t);
                        } else {
                            let d = s[t].clone();
                            s.insert(t, d);
                        }
                    }
                }
            }
            let mut p = Av1Parser::new();
            for tu in &s {
                match p.temporal_unit(tu) {
                    Ok(_) => ok += 1,
                    Err(_) => errs += 1,
                }
            }
        }
        // random temporal units after a real sequence header
        let seq = &seeds[0][0];
        for _ in 0..iters {
            let mut p = Av1Parser::new();
            let _ = p.temporal_unit(seq);
            for _ in 0..4 {
                let mut tu = vec![0u8; 2 + rnd(64)];
                for b in tu.iter_mut() {
                    *b = rnd(256) as u8;
                }
                tu[0] = [0x12, 0x1a, 0x32, 0x22, 0x0a][rnd(5)]; // TD, frame header, frame, tile group, sequence header
                tu[1] = (tu.len() - 2) as u8;
                match p.temporal_unit(&tu) {
                    Ok(_) => ok += 1,
                    Err(_) => errs += 1,
                }
            }
        }
        eprintln!("{iters} runs: {ok} temporal units parsed, {errs} refused");
        assert!(ok > 0 && errs > 0);
    }
}
