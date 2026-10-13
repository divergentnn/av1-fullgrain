//! AV1 film grain parameters of one picture, and where to get them:
//! - av1-fullgrain's `PARAMS.json` (`av1grain strip`, format `av1-fullgrain-params` version 1);
//! - ffmpeg's film grain side data, as its `showinfo` filter prints it (`-export_side_data film_grain`);
//! - a C caller's `Dav1dFilmGrainData` (the same layout as [`FilmGrainParams`]).
//!
//! Copyright © 2026, divergentnn (av1-fullgrain). SPDX-License-Identifier: BSD-2-Clause

use std::fmt;

/// The film grain of one picture, with the values dav1d uses (signed, offsets removed). The layout is that of
/// dav1d's `Dav1dFilmGrainData` (`include/dav1d/headers.h`), so a C caller can pass dav1d's struct as it is.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct FilmGrainParams {
    /// grain_seed (16 bits)
    pub seed: u32,
    pub num_y_points: i32,
    /// [value, scaling]
    pub y_points: [[u8; 2]; 14],
    pub chroma_scaling_from_luma: i32,
    pub num_uv_points: [i32; 2],
    pub uv_points: [[[u8; 2]; 10]; 2],
    /// 8..=11
    pub scaling_shift: i32,
    /// 0..=3
    pub ar_coeff_lag: i32,
    pub ar_coeffs_y: [i8; 24],
    /// 2 lag (lag + 1) taps, then the luma tap when num_y_points > 0
    pub ar_coeffs_uv: [[i8; 28]; 2],
    /// 6..=9
    pub ar_coeff_shift: u64,
    /// 0..=3
    pub grain_scale_shift: i32,
    /// cb_mult - 128, cr_mult - 128
    pub uv_mult: [i32; 2],
    /// cb_luma_mult - 128, cr_luma_mult - 128
    pub uv_luma_mult: [i32; 2],
    /// cb_offset - 256, cr_offset - 256
    pub uv_offset: [i32; 2],
    pub overlap_flag: i32,
    pub clip_to_restricted_range: i32,
}

impl Default for FilmGrainParams {
    fn default() -> Self {
        FilmGrainParams {
            seed: 0, num_y_points: 0, y_points: [[0; 2]; 14], chroma_scaling_from_luma: 0, num_uv_points: [0; 2],
            uv_points: [[[0; 2]; 10]; 2], scaling_shift: 8, ar_coeff_lag: 0, ar_coeffs_y: [0; 24],
            ar_coeffs_uv: [[0; 28]; 2], ar_coeff_shift: 6, grain_scale_shift: 0, uv_mult: [0; 2], uv_luma_mult: [0; 2],
            uv_offset: [0; 2], overlap_flag: 0, clip_to_restricted_range: 0,
        }
    }
}

impl fmt::Debug for FilmGrainParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let npos = self.npos();
        f.debug_struct("FilmGrainParams")
            .field("seed", &self.seed)
            .field("y_points", &&self.y_points[..self.num_y_points.clamp(0, 14) as usize])
            .field("chroma_scaling_from_luma", &self.chroma_scaling_from_luma)
            .field("uv_points", &[&self.uv_points[0][..self.num_uv_points[0].clamp(0, 10) as usize],
                                  &self.uv_points[1][..self.num_uv_points[1].clamp(0, 10) as usize]])
            .field("scaling_shift", &self.scaling_shift)
            .field("ar_coeff_lag", &self.ar_coeff_lag)
            .field("ar_coeffs_y", &&self.ar_coeffs_y[..npos])
            .field("ar_coeffs_uv", &[&self.ar_coeffs_uv[0][..npos + 1], &self.ar_coeffs_uv[1][..npos + 1]])
            .field("ar_coeff_shift", &self.ar_coeff_shift)
            .field("grain_scale_shift", &self.grain_scale_shift)
            .field("uv_mult", &self.uv_mult)
            .field("uv_luma_mult", &self.uv_luma_mult)
            .field("uv_offset", &self.uv_offset)
            .field("overlap_flag", &self.overlap_flag)
            .field("clip_to_restricted_range", &self.clip_to_restricted_range)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamError(pub String);

impl fmt::Display for ParamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParamError {}

fn err<T>(m: impl Into<String>) -> Result<T, ParamError> {
    Err(ParamError(m.into()))
}

impl FilmGrainParams {
    /// AR taps per plane before the chroma luma tap: 2 lag (lag + 1)
    pub fn npos(&self) -> usize {
        let l = self.ar_coeff_lag.clamp(0, 3) as usize;
        2 * l * (l + 1)
    }

    /// The parameters without the seed: pictures that differ only in this share their tables.
    pub fn same_tables(&self, o: &FilmGrainParams) -> bool {
        FilmGrainParams { seed: 0, ..*self } == FilmGrainParams { seed: 0, ..*o }
    }

    /// Checks what the renderer relies on. Parameters a frame inherits from a reference slot that never had grain
    /// (update_grain = 0, which libaom writes) are all zero, shifts included: allowed, they draw no grain.
    pub fn validate(&self) -> Result<(), ParamError> {
        if !(0..=14).contains(&self.num_y_points) {
            return err("num_y_points > 14");
        }
        for pl in 0..2 {
            if !(0..=10).contains(&self.num_uv_points[pl]) {
                return err("num_uv_points > 10");
            }
        }
        if !(0..=11).contains(&self.scaling_shift) {
            return err("scaling_shift outside 8..11");
        }
        if !(0..=3).contains(&self.ar_coeff_lag) {
            return err("ar_coeff_lag outside 0..3");
        }
        if self.ar_coeff_shift > 9 {
            return err("ar_coeff_shift outside 6..9");
        }
        if !(0..=3).contains(&self.grain_scale_shift) {
            return err("grain_scale_shift outside 0..3");
        }
        let pts = |p: &[[u8; 2]]| p.windows(2).all(|w| w[0][0] < w[1][0]);
        if !pts(&self.y_points[..self.num_y_points as usize]) {
            return err("y_points not increasing");
        }
        for pl in 0..2 {
            if !pts(&self.uv_points[pl][..self.num_uv_points[pl] as usize]) {
                return err("uv_points not increasing");
            }
            if !(-128..=127).contains(&self.uv_mult[pl]) || !(-128..=127).contains(&self.uv_luma_mult[pl])
                || !(-256..=255).contains(&self.uv_offset[pl]) {
                return err("chroma multiplier or offset out of range");
            }
        }
        Ok(())
    }

    /// dav1d draws grain on a picture only when this holds (otherwise the picture passes unchanged)
    pub fn has_grain(&self) -> bool {
        self.num_y_points > 0 || self.num_uv_points[0] > 0 || self.num_uv_points[1] > 0
            || (self.clip_to_restricted_range != 0 && self.chroma_scaling_from_luma != 0)
    }
}

/// dav1d's pixel layouts
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum Layout {
    I400 = 0,
    I420 = 1,
    I422 = 2,
    I444 = 3,
}

impl Layout {
    pub fn from_i32(v: i32) -> Option<Layout> {
        Some(match v {
            0 => Layout::I400,
            1 => Layout::I420,
            2 => Layout::I422,
            3 => Layout::I444,
            _ => return None,
        })
    }
    pub fn ssx(self) -> usize {
        matches!(self, Layout::I420 | Layout::I422) as usize
    }
    pub fn ssy(self) -> usize {
        matches!(self, Layout::I420) as usize
    }
}

/// A picture format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    pub width: usize,
    pub height: usize,
    /// 8, 10 or 12
    pub bitdepth: u32,
    pub layout: Layout,
    /// matrix_coefficients (only 0, identity, matters: clip_to_restricted_range then clips chroma to 235, not 240)
    pub matrix: u32,
}

impl Format {
    pub fn chroma_size(&self) -> (usize, usize) {
        if self.layout == Layout::I400 {
            return (0, 0);
        }
        let (sx, sy) = (self.layout.ssx(), self.layout.ssy());
        ((self.width + sx) >> sx, (self.height + sy) >> sy)
    }
    pub fn bytes_per_sample(&self) -> usize {
        if self.bitdepth > 8 { 2 } else { 1 }
    }
    /// bytes of one packed picture (planes one after another, rows without padding)
    pub fn frame_bytes(&self) -> usize {
        let (cw, ch) = self.chroma_size();
        (self.width * self.height + 2 * cw * ch) * self.bytes_per_sample()
    }
    pub fn validate(&self) -> Result<(), ParamError> {
        if self.width == 0 || self.height == 0 || self.width > 16384 || self.height > 16384 {
            return err(format!("picture size {}x{} not supported", self.width, self.height));
        }
        if ![8, 10, 12].contains(&self.bitdepth) {
            return err(format!("bit depth {} not supported", self.bitdepth));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------------------------
// PARAMS.json (av1grain strip)

/// A whole clip's grain from av1-fullgrain's `PARAMS.json`.
#[derive(Debug, Clone)]
pub struct ParamsFile {
    pub format: Format,
    pub sets: Vec<FilmGrainParams>,
    /// per output picture: None (no grain) or (set, seed)
    pub grain: Vec<Option<(usize, u32)>>,
    pub pts: Vec<i64>,
}

fn ji(v: &serde_json::Value, k: &str) -> Result<i64, ParamError> {
    v.get(k).and_then(|x| x.as_i64()).ok_or_else(|| ParamError(format!("PARAMS.json: missing {k}")))
}

fn jints(v: &serde_json::Value) -> Result<Vec<i64>, ParamError> {
    v.as_array()
        .ok_or_else(|| ParamError("PARAMS.json: expected an array".into()))?
        .iter()
        .map(|x| x.as_i64().ok_or_else(|| ParamError("PARAMS.json: expected an integer".into())))
        .collect()
}

fn jpoints(v: &serde_json::Value, max: usize) -> Result<Vec<[u8; 2]>, ParamError> {
    let a = v.as_array().ok_or_else(|| ParamError("PARAMS.json: points: expected an array".into()))?;
    if a.len() > max {
        return err("PARAMS.json: too many scaling points");
    }
    a.iter()
        .map(|p| {
            let p = jints(p)?;
            if p.len() != 2 || p.iter().any(|&x| !(0..=255).contains(&x)) {
                return err("PARAMS.json: bad scaling point");
            }
            Ok([p[0] as u8, p[1] as u8])
        })
        .collect()
}

fn set_from_json(s: &serde_json::Value) -> Result<FilmGrainParams, ParamError> {
    let mut g = FilmGrainParams::default();
    let yp = jpoints(s.get("y_points").unwrap_or(&serde_json::Value::Null), 14)?;
    g.num_y_points = yp.len() as i32;
    g.y_points[..yp.len()].copy_from_slice(&yp);
    g.chroma_scaling_from_luma = ji(s, "chroma_scaling_from_luma")? as i32;
    let uvp = s.get("uv_points").and_then(|x| x.as_array()).ok_or_else(|| ParamError("PARAMS.json: uv_points".into()))?;
    if uvp.len() != 2 {
        return err("PARAMS.json: uv_points needs 2 lists");
    }
    for pl in 0..2 {
        let p = jpoints(&uvp[pl], 10)?;
        g.num_uv_points[pl] = p.len() as i32;
        g.uv_points[pl][..p.len()].copy_from_slice(&p);
    }
    g.scaling_shift = ji(s, "scaling_shift")? as i32;
    g.ar_coeff_lag = ji(s, "ar_coeff_lag")? as i32;
    if !(0..=3).contains(&g.ar_coeff_lag) {
        return err("PARAMS.json: bad ar_coeff_lag");
    }
    let npos = g.npos();
    let cy = jints(s.get("ar_coeffs_y").unwrap_or(&serde_json::Value::Null))?;
    if cy.len() != npos {
        return err("PARAMS.json: ar_coeffs_y has the wrong length");
    }
    for (i, &c) in cy.iter().enumerate() {
        g.ar_coeffs_y[i] = c as i8;
    }
    let cuv = s.get("ar_coeffs_uv").and_then(|x| x.as_array()).ok_or_else(|| ParamError("PARAMS.json: ar_coeffs_uv".into()))?;
    if cuv.len() != 2 {
        return err("PARAMS.json: ar_coeffs_uv needs 2 lists");
    }
    for pl in 0..2 {
        let c = jints(&cuv[pl])?;
        if c.len() != npos + 1 {
            return err("PARAMS.json: ar_coeffs_uv has the wrong length");
        }
        for (i, &v) in c.iter().enumerate() {
            g.ar_coeffs_uv[pl][i] = v as i8;
        }
    }
    g.ar_coeff_shift = ji(s, "ar_coeff_shift")? as u64;
    g.grain_scale_shift = ji(s, "grain_scale_shift")? as i32;
    for (k, dst) in [("uv_mult", &mut g.uv_mult), ("uv_luma_mult", &mut g.uv_luma_mult), ("uv_offset", &mut g.uv_offset)] {
        let v = jints(s.get(k).unwrap_or(&serde_json::Value::Null))?;
        if v.len() != 2 {
            return err(format!("PARAMS.json: {k} needs 2 values"));
        }
        *dst = [v[0] as i32, v[1] as i32];
    }
    g.overlap_flag = ji(s, "overlap_flag")? as i32;
    g.clip_to_restricted_range = ji(s, "clip_to_restricted_range")? as i32;
    g.validate()?;
    Ok(g)
}

impl ParamsFile {
    pub fn parse(text: &str) -> Result<ParamsFile, ParamError> {
        let v: serde_json::Value = serde_json::from_str(text).map_err(|e| ParamError(format!("PARAMS.json: {e}")))?;
        if v.get("format").and_then(|x| x.as_str()) != Some("av1-fullgrain-params") || ji(&v, "version")? != 1 {
            return err("not an av1-fullgrain-params version 1 file");
        }
        let layout = Layout::from_i32(ji(&v, "layout")? as i32).ok_or_else(|| ParamError("PARAMS.json: bad layout".into()))?;
        let format = Format {
            width: ji(&v, "width")? as usize,
            height: ji(&v, "height")? as usize,
            bitdepth: ji(&v, "bitdepth")? as u32,
            layout,
            matrix: v.get("mtrx").and_then(|x| x.as_i64()).unwrap_or(2) as u32,
        };
        format.validate()?;
        let sets = v.get("sets").and_then(|x| x.as_array()).ok_or_else(|| ParamError("PARAMS.json: sets".into()))?
            .iter().map(set_from_json).collect::<Result<Vec<_>, _>>()?;
        let n = ji(&v, "frames")? as usize;
        let gl = v.get("grain").and_then(|x| x.as_array()).ok_or_else(|| ParamError("PARAMS.json: grain".into()))?;
        if gl.len() != n {
            return err("PARAMS.json: the grain list does not match frames");
        }
        let mut grain = Vec::with_capacity(n);
        for g in gl {
            let g = jints(g)?;
            if g.len() != 2 {
                return err("PARAMS.json: bad grain entry");
            }
            if g[0] < 0 {
                grain.push(None);
            } else if (g[0] as usize) < sets.len() {
                grain.push(Some((g[0] as usize, g[1] as u32)));
            } else {
                return err("PARAMS.json: grain names a missing set");
            }
        }
        let pts = match v.get("pts") {
            Some(p) => jints(p)?,
            None => (0..n as i64).collect(),
        };
        Ok(ParamsFile { format, sets, grain, pts })
    }

    /// picture n's grain (None: no grain)
    pub fn picture(&self, n: usize) -> Option<FilmGrainParams> {
        self.grain.get(n).copied().flatten().map(|(s, seed)| FilmGrainParams { seed, ..self.sets[s] })
    }
}

// ---------------------------------------------------------------------------------------------------------------
// ffmpeg showinfo

/// Which ffmpeg decoder exported the film grain side data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideDataFrom {
    /// libdav1d (ffmpeg's software AV1 decoder): dav1d's signed values
    Libdav1d,
    /// ffmpeg's own AV1 decoder (used with -hwaccel): up to ffmpeg 9.0 at least it exports cb_mult, cb_luma_mult and
    /// cb_offset (and the Cr ones) as coded, without removing the 128 / 128 / 256 offsets
    FfmpegAv1,
}

/// Parses the film grain side data line that ffmpeg's `showinfo` filter prints for a frame decoded by libdav1d:
/// `... side data - Film grain parameters: type av1; seed=45231; ...; y_points={ (0,96) }; ...`.
/// Returns Ok(None) for a line that is not AV1 film grain side data.
pub fn parse_showinfo_line(line: &str) -> Result<Option<FilmGrainParams>, ParamError> {
    parse_showinfo_line_from(line, SideDataFrom::Libdav1d)
}

/// [`parse_showinfo_line`] for side data from either of ffmpeg's AV1 decoders.
pub fn parse_showinfo_line_from(line: &str, from: SideDataFrom) -> Result<Option<FilmGrainParams>, ParamError> {
    let Some(at) = line.find("Film grain parameters:") else { return Ok(None) };
    let body = &line[at + "Film grain parameters:".len()..];
    let mut kv: Vec<(String, String)> = Vec::new();
    for item in body.split(';') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        if let Some(t) = item.strip_prefix("type ") {
            kv.push(("type".into(), t.trim().into()));
            continue;
        }
        if let Some((k, v)) = item.split_once('=') {
            kv.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    let get = |k: &str| kv.iter().find(|(a, _)| a == k).map(|(_, b)| b.as_str());
    if get("type") != Some("av1") {
        return Ok(None);
    }
    let int = |k: &str| -> Result<i64, ParamError> {
        get(k).ok_or_else(|| ParamError(format!("showinfo: missing {k}")))?
            .parse::<i64>().map_err(|_| ParamError(format!("showinfo: bad {k}")))
    };
    let list = |k: &str| -> Result<Vec<i64>, ParamError> {
        // ffmpeg leaves a list out when it is empty
        let v = get(k).unwrap_or("{ }");
        v.trim_matches(|c| c == '{' || c == '}' || c == ' ')
            .split_whitespace()
            .map(|x| x.parse::<i64>().map_err(|_| ParamError(format!("showinfo: bad {k}"))))
            .collect()
    };
    let points = |k: &str, max: usize| -> Result<Vec<[u8; 2]>, ParamError> {
        let v = get(k).ok_or_else(|| ParamError(format!("showinfo: missing {k}")))?;
        let mut out = vec![];
        for p in v.split('(').skip(1) {
            let p = p.split(')').next().unwrap_or("");
            let (a, b) = p.split_once(',').ok_or_else(|| ParamError(format!("showinfo: bad {k}")))?;
            let a: i64 = a.trim().parse().map_err(|_| ParamError(format!("showinfo: bad {k}")))?;
            let b: i64 = b.trim().parse().map_err(|_| ParamError(format!("showinfo: bad {k}")))?;
            if !(0..=255).contains(&a) || !(0..=255).contains(&b) {
                return err(format!("showinfo: bad {k}"));
            }
            out.push([a as u8, b as u8]);
        }
        if out.len() > max {
            return err(format!("showinfo: too many {k}"));
        }
        Ok(out)
    };
    let mut g = FilmGrainParams { seed: int("seed")? as u32, ..Default::default() };
    let yp = points("y_points", 14)?;
    g.num_y_points = yp.len() as i32;
    g.y_points[..yp.len()].copy_from_slice(&yp);
    g.chroma_scaling_from_luma = int("chroma_scaling_from_luma")? as i32;
    for pl in 0..2 {
        let p = points(&format!("uv_points[{pl}]"), 10)?;
        g.num_uv_points[pl] = p.len() as i32;
        g.uv_points[pl][..p.len()].copy_from_slice(&p);
    }
    g.scaling_shift = int("scaling_shift")? as i32;
    g.ar_coeff_lag = int("ar_coeff_lag")? as i32;
    if !(0..=3).contains(&g.ar_coeff_lag) {
        return err("showinfo: bad ar_coeff_lag");
    }
    let npos = g.npos();
    let cy = list("ar_coeffs_y")?;
    if cy.len() != npos {
        return err("showinfo: ar_coeffs_y has the wrong length");
    }
    for (i, c) in cy.into_iter().enumerate() {
        g.ar_coeffs_y[i] = c as i8;
    }
    for pl in 0..2 {
        let c = list(&format!("ar_coeffs_uv[{pl}]"))?;
        // ffmpeg prints npos + 1 values when there are luma points, else npos
        if c.len() != npos + (g.num_y_points > 0) as usize {
            return err("showinfo: ar_coeffs_uv has the wrong length");
        }
        for (i, v) in c.into_iter().enumerate() {
            g.ar_coeffs_uv[pl][i] = v as i8;
        }
    }
    g.ar_coeff_shift = int("ar_coeff_shift")? as u64;
    g.grain_scale_shift = int("grain_scale_shift")? as i32;
    for pl in 0..2 {
        g.uv_mult[pl] = int(&format!("uv_mult[{pl}]"))? as i32;
        g.uv_luma_mult[pl] = int(&format!("uv_mult_luma[{pl}]"))? as i32;
        g.uv_offset[pl] = int(&format!("uv_offset[{pl}]"))? as i32;
    }
    g.overlap_flag = int("overlap_flag")? as i32;
    g.clip_to_restricted_range = int("limit_output_range")? as i32;
    if from == SideDataFrom::FfmpegAv1 {
        for pl in 0..2 {
            g.uv_mult[pl] -= 128;
            g.uv_luma_mult[pl] -= 128;
            g.uv_offset[pl] -= 256;
        }
    }
    g.validate()?;
    Ok(Some(g))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = "[Parsed_showinfo_0 @ 0x7f59dc0032c0]   side data - Film grain parameters: type av1; seed=45231; width=320; height=240; subsampling_x=1; subsampling_y=1; color_range=tv; color_primaries=unknown; color_trc=unknown; color_space=unknown; bit_depth_luma=0; bit_depth_chroma=0; y_points={ (0,96) }; chroma_scaling_from_luma=1; uv_points[0]={ }; uv_points[1]={ }; scaling_shift=11; ar_coeff_lag=2; ar_coeffs_y={ 5 -15 -10 -19 0 -12 6 51 30 -5 -12 56 }; ar_coeffs_uv[0]={ 2 2 -24 -5 1 1 -18 37 -2 0 -15 39 -70 }; ar_coeffs_uv[1]={ 2 3 -24 -5 -1 0 -18 38 -2 0 -15 39 -55 }; ar_coeff_shift=7; grain_scale_shift=0; uv_mult[0] = 0; uv_mult_luma[0] = 0; uv_offset[0] = 0; uv_mult[1] = 0; uv_mult_luma[1] = 0; uv_offset[1] = 0; overlap_flag=1; limit_output_range=0; ";

    const JSON: &str = r#"{"format":"av1-fullgrain-params","version":1,
"width":320,"height":240,"bitdepth":10,"layout":1,"mtrx":2,"trc":2,"pri":2,"color_range":0,"frames":2,
"sets":[
{"num_y_points":1,"y_points":[[0,96]],"chroma_scaling_from_luma":1,"num_uv_points":[0,0],"uv_points":[[],[]],"scaling_shift":11,"ar_coeff_lag":2,"ar_coeffs_y":[5,-15,-10,-19,0,-12,6,51,30,-5,-12,56],"ar_coeffs_uv":[[2,2,-24,-5,1,1,-18,37,-2,0,-15,39,-70],[2,3,-24,-5,-1,0,-18,38,-2,0,-15,39,-55]],"ar_coeff_shift":7,"grain_scale_shift":0,"uv_mult":[0,0],"uv_luma_mult":[0,0],"uv_offset":[0,0],"overlap_flag":1,"clip_to_restricted_range":0}],
"grain":[[0,45231],[-1,0]],
"pts":[0,1]}"#;

    #[test]
    fn showinfo_matches_params_json() {
        let a = parse_showinfo_line(LINE).unwrap().unwrap();
        let f = ParamsFile::parse(JSON).unwrap();
        assert_eq!(f.picture(0).unwrap(), a);
        assert!(f.picture(1).is_none());
        assert_eq!(a.seed, 45231);
        assert_eq!(a.ar_coeffs_uv[1][12], -55);
    }

    #[test]
    fn not_grain() {
        assert_eq!(parse_showinfo_line("n:   2 pts: 2").unwrap(), None);
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn layout_matches_dav1d() {
        // Dav1dFilmGrainData on 64-bit targets
        assert_eq!(std::mem::size_of::<FilmGrainParams>(), 224);
        assert_eq!(std::mem::offset_of!(FilmGrainParams, ar_coeff_shift), 176);
        assert_eq!(std::mem::offset_of!(FilmGrainParams, clip_to_restricted_range), 216);
    }
}
