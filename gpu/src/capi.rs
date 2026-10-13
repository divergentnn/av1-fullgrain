//! C ABI (include/av1_fullgrain_gpu.h): for players and tools in C or C++.
//!
//! Copyright © 2026, divergentnn (av1-fullgrain). SPDX-License-Identifier: BSD-2-Clause

use crate::gpu::{GrainMode, Options, Plane, PlaneMut, Renderer};
use crate::params::{FilmGrainParams, Format, Layout};
use std::ffi::{c_char, c_int, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;

/// The renderer behind `AV1FGRenderer *`
pub struct AV1FGRenderer {
    r: Renderer,
    error: CString,
    name: CString,
}

/// A picture: `AV1FGPicture` in the header
#[repr(C)]
pub struct AV1FGPicture {
    pub width: c_int,
    pub height: c_int,
    /// 8, 10 or 12
    pub bitdepth: c_int,
    /// 0 I400, 1 I420, 2 I422, 3 I444 (dav1d's DAV1D_PIXEL_LAYOUT_*)
    pub layout: c_int,
    /// matrix_coefficients (0, identity: restricted-range chroma clips at 235)
    pub matrix: c_int,
    /// planes: 8-bit samples, or 16-bit (native endian = little endian on every supported GPU host) for 10/12-bit
    pub data: [*mut u8; 3],
    /// bytes per row
    pub stride: [isize; 3],
}

fn set_err(h: *mut AV1FGRenderer, m: &str) {
    if let Some(h) = unsafe { h.as_mut() } {
        h.error = CString::new(m.replace('\0', " ")).unwrap_or_default();
    }
}

fn fmt_of(p: &AV1FGPicture) -> Result<Format, String> {
    let layout = Layout::from_i32(p.layout).ok_or("bad layout")?;
    if p.width <= 0 || p.height <= 0 {
        return Err("bad picture size".into());
    }
    Ok(Format { width: p.width as usize, height: p.height as usize, bitdepth: p.bitdepth as u32, layout, matrix: p.matrix as u32 })
}

/// the planes of a picture as slices (rows of `stride` bytes; the last row needs only width samples)
unsafe fn planes_in<'a>(p: &AV1FGPicture, f: &Format) -> Result<Vec<Plane<'a>>, String> {
    let bps = f.bytes_per_sample();
    let (cw, ch) = f.chroma_size();
    let n = if f.layout == Layout::I400 { 1 } else { 3 };
    let mut v = Vec::with_capacity(n);
    for i in 0..n {
        let (w, h) = if i == 0 { (f.width, f.height) } else { (cw, ch) };
        if p.data[i].is_null() || p.stride[i] < (w * bps) as isize {
            return Err(format!("plane {i}: null or stride too small"));
        }
        let len = p.stride[i] as usize * (h - 1) + w * bps;
        v.push(Plane { data: std::slice::from_raw_parts(p.data[i], len), stride: p.stride[i] as usize });
    }
    Ok(v)
}

unsafe fn planes_out<'a>(p: &AV1FGPicture, f: &Format) -> Result<Vec<PlaneMut<'a>>, String> {
    let bps = f.bytes_per_sample();
    let (cw, ch) = f.chroma_size();
    let n = if f.layout == Layout::I400 { 1 } else { 3 };
    let mut v = Vec::with_capacity(n);
    for i in 0..n {
        let (w, h) = if i == 0 { (f.width, f.height) } else { (cw, ch) };
        if p.data[i].is_null() || p.stride[i] < (w * bps) as isize {
            return Err(format!("output plane {i}: null or stride too small"));
        }
        let len = p.stride[i] as usize * (h - 1) + w * bps;
        v.push(PlaneMut { data: std::slice::from_raw_parts_mut(p.data[i], len), stride: p.stride[i] as usize });
    }
    Ok(v)
}

fn mode_of(m: c_int) -> Result<GrainMode, String> {
    match m {
        0 => Ok(GrainMode::Full),
        1 => Ok(GrainMode::Standard),
        _ => Err("mode: 0 (full-frame) or 1 (standard)".into()),
    }
}

/// Open a GPU. `adapter`: part of the GPU's name, or NULL for the high-performance one. `frames_in_flight`: how many
/// pictures av1fg_submit may queue (0: 3). On failure returns NULL and writes the reason to `err` (if not NULL).
#[no_mangle]
pub unsafe extern "C" fn av1fg_open(adapter: *const c_char, frames_in_flight: c_int, err: *mut c_char, errlen: usize)
                                    -> *mut AV1FGRenderer {
    let r = catch_unwind(|| {
        let adapter = if adapter.is_null() { None } else { Some(CStr::from_ptr(adapter).to_string_lossy().into_owned()) };
        let opts = Options {
            adapter,
            frames_in_flight: if frames_in_flight > 0 { frames_in_flight as usize } else { 3 },
            ..Default::default()
        };
        Renderer::new(&opts).map_err(|e| e.0)
    });
    let r = match r {
        Ok(r) => r,
        Err(_) => Err("panic while opening the GPU".to_string()),
    };
    match r {
        Ok(r) => {
            let name = CString::new(r.adapter_name()).unwrap_or_default();
            Box::into_raw(Box::new(AV1FGRenderer { r, error: CString::default(), name }))
        }
        Err(e) => {
            if !err.is_null() && errlen > 0 {
                let b = e.as_bytes();
                let n = b.len().min(errlen - 1);
                ptr::copy_nonoverlapping(b.as_ptr(), err as *mut u8, n);
                *err.add(n) = 0;
            }
            ptr::null_mut()
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn av1fg_close(h: *mut AV1FGRenderer) {
    if !h.is_null() {
        drop(Box::from_raw(h));
    }
}

/// The GPU's name and backend.
#[no_mangle]
pub unsafe extern "C" fn av1fg_adapter_name(h: *const AV1FGRenderer) -> *const c_char {
    h.as_ref().map_or(ptr::null(), |h| h.name.as_ptr())
}

/// The last error of this renderer ("" if none).
#[no_mangle]
pub unsafe extern "C" fn av1fg_last_error(h: *const AV1FGRenderer) -> *const c_char {
    h.as_ref().map_or(c"no renderer".as_ptr(), |h| h.error.as_ptr())
}

#[no_mangle]
pub extern "C" fn av1fg_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char
}

/// Queue a grain-free picture and its film grain (`grain` NULL: none; the layout of dav1d's Dav1dFilmGrainData, so
/// `&pic->frame_hdr->film_grain.data` can be passed as it is). Returns 0, or -1 (av1fg_last_error).
#[no_mangle]
pub unsafe extern "C" fn av1fg_submit(h: *mut AV1FGRenderer, pic: *const AV1FGPicture, grain: *const FilmGrainParams,
                                      mode: c_int) -> c_int {
    let Some(hr) = h.as_mut() else { return -1 };
    let Some(pic) = pic.as_ref() else {
        set_err(h, "no picture");
        return -1;
    };
    let r = catch_unwind(AssertUnwindSafe(|| -> Result<(), String> {
        let f = fmt_of(pic)?;
        let planes = planes_in(pic, &f)?;
        hr.r.submit(&f, &planes, grain.as_ref(), mode_of(mode)?).map_err(|e| e.0)?;
        Ok(())
    }));
    match r {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => {
            set_err(h, &e);
            -1
        }
        Err(_) => {
            set_err(h, "panic in av1fg_submit");
            -1
        }
    }
}

/// Wait for the oldest queued picture and write it to `out` (its width, height, bitdepth and layout must match the
/// submitted picture's; data and stride say where). Returns 1, 0 when nothing is queued, or -1 (av1fg_last_error).
#[no_mangle]
pub unsafe extern "C" fn av1fg_receive(h: *mut AV1FGRenderer, out: *const AV1FGPicture) -> c_int {
    let Some(hr) = h.as_mut() else { return -1 };
    let Some(out) = out.as_ref() else {
        set_err(h, "no output picture");
        return -1;
    };
    let r = catch_unwind(AssertUnwindSafe(|| -> Result<c_int, String> {
        let f = fmt_of(out)?;
        let mut planes = planes_out(out, &f)?;
        Ok(match hr.r.receive_planes(&mut planes).map_err(|e| e.0)? {
            Some(_) => 1,
            None => 0,
        })
    }));
    match r {
        Ok(Ok(n)) => n,
        Ok(Err(e)) => {
            set_err(h, &e);
            -1
        }
        Err(_) => {
            set_err(h, "panic in av1fg_receive");
            -1
        }
    }
}

/// Room for another av1fg_submit before an av1fg_receive (1 or 0).
#[no_mangle]
pub unsafe extern "C" fn av1fg_can_submit(h: *const AV1FGRenderer) -> c_int {
    h.as_ref().map_or(0, |h| h.r.can_submit() as c_int)
}

/// One picture, synchronously: av1fg_submit + av1fg_receive. `in` and `out` may not overlap.
#[no_mangle]
pub unsafe extern "C" fn av1fg_render(h: *mut AV1FGRenderer, pic: *const AV1FGPicture, grain: *const FilmGrainParams,
                                      mode: c_int, out: *const AV1FGPicture) -> c_int {
    if let Some(hr) = h.as_ref() {
        if hr.r.pending() > 0 {
            set_err(h, "av1fg_render with pictures queued: receive them first");
            return -1;
        }
    }
    if av1fg_submit(h, pic, grain, mode) != 0 {
        return -1;
    }
    match av1fg_receive(h, out) {
        1 => 0,
        _ => -1,
    }
}
