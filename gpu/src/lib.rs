//! # av1-fullgrain-gpu
//!
//! AV1 film grain drawn on the GPU onto decoded grain-free pictures, **full-frame** (av1-fullgrain's grain: no
//! 32x32 copies of one small patch) or standard (the AV1 specification's tiled grain), bit-identical to
//! av1-fullgrain's dav1d. wgpu runs the shaders on Vulkan, Metal or DX12: NVIDIA, AMD, Intel and Apple GPUs on
//! Linux, Windows and macOS.
//!
//! ```no_run
//! use av1_fullgrain_gpu::{Format, GrainMode, Layout, Options, Plane, Renderer, ParamsFile};
//! let params = ParamsFile::parse(&std::fs::read_to_string("clip.json").unwrap()).unwrap();
//! let f = params.format;
//! let mut r = Renderer::new(&Options::default()).unwrap();
//! let input = vec![0u8; f.frame_bytes()];          // a decoded grain-free picture, packed planes
//! let (cw, ch) = f.chroma_size();
//! let bps = f.bytes_per_sample();
//! let (y, uv) = input.split_at(f.width * f.height * bps);
//! let (u, v) = uv.split_at(cw * ch * bps);
//! let planes = [Plane { data: y, stride: f.width * bps }, Plane { data: u, stride: cw * bps },
//!               Plane { data: v, stride: cw * bps }];
//! let mut out = vec![0u8; f.frame_bytes()];
//! r.render(&f, &planes, params.picture(0).as_ref(), GrainMode::Full, &mut out).unwrap();
//! ```
//!
//! Copyright © 2026, divergentnn (av1-fullgrain). SPDX-License-Identifier: BSD-2-Clause

pub mod capi;
pub mod obu;
mod gpu;
pub mod params;
pub mod tables;
pub mod y4m;

pub use gpu::{list_adapters, Error, GrainMode, Options, PassTimes, Plane, PlaneMut, Renderer, WGSL};
pub use params::{parse_showinfo_line, parse_showinfo_line_from, FilmGrainParams, Format, Layout, ParamError, ParamsFile, SideDataFrom};
