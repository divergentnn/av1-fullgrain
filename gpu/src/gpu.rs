//! The renderer: AV1 film grain on decoded grain-free pictures, with wgpu compute shaders (`grain.wgsl`, shared with
//! the browser kit). Runs on Vulkan, Metal and DX12, so on NVIDIA, AMD, Intel and Apple GPUs.
//!
//! Copyright © 2026, divergentnn (av1-fullgrain). SPDX-License-Identifier: BSD-2-Clause

use crate::params::{FilmGrainParams, Format, Layout};
use crate::tables;
use std::borrow::Cow;
use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

/// The shader source (also the browser kit's, through browser/web/grain-wgsl.mjs).
pub const WGSL: &str = include_str!("grain.wgsl");

// the shader's constants (a test checks them against grain.wgsl)
pub(crate) const ROWS: usize = 144;
pub(crate) const S: usize = 4;
pub(crate) const K: usize = 2;
pub(crate) const TPL: usize = 6068;
pub(crate) const GSTRIDE: usize = 24;
pub(crate) const NPARAM: usize = 144;

/// parameter block indices (P_* in grain.wgsl)
#[allow(dead_code)]
pub(crate) mod p {
    pub const W: usize = 0;
    pub const H: usize = 1;
    pub const SSX: usize = 2;
    pub const SSY: usize = 3;
    pub const BD: usize = 4;
    pub const CW: usize = 5;
    pub const CH: usize = 6;
    pub const NG: usize = 7;
    pub const LAG: usize = 8;
    pub const ARSH: usize = 9;
    pub const GMIN: usize = 10;
    pub const GMAX: usize = 11;
    pub const HASY: usize = 12;
    pub const HASU: usize = 13;
    pub const HASV: usize = 14;
    pub const KEYY: usize = 15;
    pub const KEYU: usize = 16;
    pub const KEYV: usize = 17;
    pub const SCSH: usize = 18;
    pub const YMIN: usize = 19;
    pub const YMAX: usize = 20;
    pub const CMIN: usize = 21;
    pub const CMAX: usize = 22;
    pub const CSFL: usize = 23;
    pub const ULM: usize = 24;
    pub const UM: usize = 25;
    pub const UOFF: usize = 26;
    pub const VLM: usize = 27;
    pub const VM: usize = 28;
    pub const VOFF: usize = 29;
    pub const LCU: usize = 30;
    pub const LCV: usize = 31;
    pub const RSL: usize = 32;
    pub const LPITCH: usize = 33;
    pub const LBW: usize = 34;
    pub const NCHL: usize = 35;
    pub const RSC: usize = 36;
    pub const CPITCH: usize = 37;
    pub const CBW: usize = 38;
    pub const NCHC: usize = 39;
    pub const SOY: usize = 40;
    pub const SSY_: usize = 41;
    pub const SOU: usize = 42;
    pub const SSU: usize = 43;
    pub const SOV: usize = 44;
    pub const SSV: usize = 45;
    pub const BPS: usize = 46;
    pub const DOY: usize = 47;
    pub const DOU: usize = 48;
    pub const DOV: usize = 49;
    pub const DSY: usize = 50;
    pub const DSC: usize = 51;
    pub const MONO: usize = 52;
    pub const SCPU: usize = 53;
    pub const SCPV: usize = 54;
    pub const PMAX: usize = 55;
    pub const CPLANE: usize = 56;
    pub const NTC: usize = 57;
    pub const MODE: usize = 58;
    pub const SEED: usize = 59;
    pub const OVL: usize = 60;
    pub const NBR: usize = 61;
    pub const NBC: usize = 62;
    pub const STEPY: usize = 63;
    pub const GRID: usize = 64;
    pub const STEPC: usize = 136;
}

/// How the grain is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrainMode {
    /// full-frame grain (av1-fullgrain's default): no 32x32 copies of one small patch
    Full,
    /// AV1 grain as the specification defines it (tiled 32x32 blocks from an 82x73 template), as stock dav1d
    Standard,
}

impl GrainMode {
    pub fn name(self) -> &'static str {
        match self {
            GrainMode::Full => "full",
            GrainMode::Standard => "standard",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<crate::params::ParamError> for Error {
    fn from(e: crate::params::ParamError) -> Self {
        Error(e.0)
    }
}

fn err<T>(m: impl Into<String>) -> Result<T, Error> {
    Err(Error(m.into()))
}

/// One input plane: `data` holds `height` rows of `stride` bytes (the last row may be short: width samples).
#[derive(Clone, Copy)]
pub struct Plane<'a> {
    pub data: &'a [u8],
    pub stride: usize,
}

/// One output plane: `data` holds `height` rows of `stride` bytes.
pub struct PlaneMut<'a> {
    pub data: &'a mut [u8],
    pub stride: usize,
}

/// Renderer options.
#[derive(Clone, Debug)]
pub struct Options {
    /// pick the first adapter whose name contains this (case-insensitive); default: the high-performance one
    pub adapter: Option<String>,
    /// backends to consider (default: Vulkan, Metal, DX12, or WGPU_BACKEND from the environment)
    pub backends: Option<wgpu::Backends>,
    /// pictures queued on the GPU before `submit` waits for the oldest (pipelining; default 3)
    pub frames_in_flight: usize,
    /// measure GPU time per pass (timestamp queries, where the adapter has them)
    pub timing: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { adapter: None, backends: None, frames_in_flight: 3, timing: false }
    }
}

/// GPU time of one picture, in milliseconds (only with `Options::timing` on an adapter with timestamp queries)
#[derive(Clone, Copy, Debug, Default)]
pub struct PassTimes {
    pub ar_luma: f64,
    pub ar_chroma: f64,
    pub templates: f64,
    pub apply: f64,
    pub total: f64,
}

struct Pipes {
    ar_luma: wgpu::ComputePipeline,
    ar_chroma: wgpu::ComputePipeline,
    lumaterm: wgpu::ComputePipeline,
    std_tpl_luma: wgpu::ComputePipeline,
    std_tpl_chroma: wgpu::ComputePipeline,
    std_offsets: wgpu::ComputePipeline,
    apply_y: wgpu::ComputePipeline,
    apply_c: wgpu::ComputePipeline,
}

/// buffers and constants of one picture format
struct FormatState {
    fmt: Format,
    p: [i32; NPARAM],
    cw: usize,
    ch: usize,
    ng: usize,
    src: wgpu::Buffer,
    src_bytes: usize,
    dst: wgpu::Buffer,
    dst_bytes: usize,
    bind: wgpu::BindGroup,
    // keep the field buffers alive with the bind group
    _fields: [wgpu::Buffer; 4],
}

struct InFlight {
    id: u64,
    sub: wgpu::SubmissionIndex,
    rb: wgpu::Buffer,
    state: Arc<AtomicU8>, // 0 pending, 1 mapped, 2 failed
    fmt: Format,
    p: [i32; NPARAM],
    timed: Option<Vec<u8>>, // with timing: the kind of each timed pass (0 AR luma, 1 AR chroma, 2 templates, 3 apply)
}

const QUERIES: u32 = 16;

pub struct Renderer {
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipes: Pipes,
    bgl: wgpu::BindGroupLayout,
    uni: wgpu::Buffer,
    tabs: wgpu::Buffer,
    state: Option<FormatState>,
    tables_for: Option<(u32, FilmGrainParams)>,
    in_flight: VecDeque<InFlight>,
    free_rb: Vec<wgpu::Buffer>,
    max_in_flight: usize,
    queries: Option<(wgpu::QuerySet, wgpu::Buffer)>,
    ts_period: f64,
    next_id: u64,
    errors: Arc<Mutex<Vec<String>>>,
}

fn as_bytes(v: &[i32]) -> &[u8] {
    // SAFETY: i32 has no padding and any byte alignment is valid for u8
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

fn align(x: usize, a: usize) -> usize {
    x.div_ceil(a) * a
}

/// AR coefficients (AV1 order: rows -lag..-1 x columns -lag..lag, then -lag..-1 in the row) as the shader's fixed
/// 3 x 7 grid of previous-row taps (row -3 first) and the in-row taps c1, c2, c3 (columns -1, -2, -3)
fn grid(pb: &mut [i32; NPARAM], at: usize, co: &[i8], lag: i32) {
    pb[at..at + GSTRIDE].fill(0);
    let mut k = 0;
    for dy in -lag..0 {
        for dx in -lag..=lag {
            pb[(at as i32 + (dy + 3) * 7 + dx + 3) as usize] = co[k] as i32;
            k += 1;
        }
    }
    for dx in -lag..0 {
        pb[(at as i32 + 20 - dx) as usize] = co[k] as i32;
        k += 1;
    }
}

/// List the adapters wgpu finds (name, backend, type).
pub fn list_adapters() -> Vec<String> {
    let instance = wgpu::Instance::new(instance_desc(None));
    pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()))
        .into_iter()
        .map(|a| {
            let i = a.get_info();
            format!("{} ({:?}, {:?}, driver {} {})", i.name, i.backend, i.device_type, i.driver, i.driver_info)
        })
        .collect()
}

fn instance_desc(backends: Option<wgpu::Backends>) -> wgpu::InstanceDescriptor {
    let mut d = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
    if let Some(b) = backends {
        d.backends = b;
    } else if std::env::var_os("WGPU_BACKEND").is_none() {
        d.backends = wgpu::Backends::VULKAN | wgpu::Backends::METAL | wgpu::Backends::DX12;
    }
    d
}

impl Renderer {
    /// Open a GPU and compile the shaders.
    pub fn new(opts: &Options) -> Result<Renderer, Error> {
        let instance = wgpu::Instance::new(instance_desc(opts.backends));
        let adapter = if let Some(want) = &opts.adapter {
            let want = want.to_lowercase();
            let all = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
            let names: Vec<String> = all.iter().map(|a| a.get_info().name).collect();
            all.into_iter()
                .find(|a| a.get_info().name.to_lowercase().contains(&want))
                .ok_or_else(|| Error(format!("no GPU adapter matches {want:?} (found: {})", names.join(", "))))?
        } else {
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
                apply_limit_buckets: false,
            }))
            .map_err(|e| Error(format!("no GPU adapter: {e}")))?
        };
        let al = adapter.limits();
        let mut limits = wgpu::Limits::default();
        limits.max_storage_buffer_binding_size = al.max_storage_buffer_binding_size;
        limits.max_buffer_size = al.max_buffer_size;
        let timing = opts.timing && adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("av1-fullgrain"),
            required_features: if timing { wgpu::Features::TIMESTAMP_QUERY } else { wgpu::Features::empty() },
            required_limits: limits,
            ..Default::default()
        }))
        .map_err(|e| Error(format!("GPU device: {e}")))?;
        let errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        {
            let errors = errors.clone();
            device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
                errors.lock().unwrap().push(e.to_string());
            }));
        }
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("grain.wgsl"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(WGSL)),
        });
        let entry = |binding: u32, ty: wgpu::BufferBindingType| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer { ty, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        use wgpu::BufferBindingType::{Storage, Uniform};
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("grain"),
            entries: &[
                entry(0, Uniform),
                entry(1, Storage { read_only: true }),
                entry(2, Storage { read_only: false }),
                entry(3, Storage { read_only: false }),
                entry(4, Storage { read_only: false }),
                entry(5, Storage { read_only: true }),
                entry(6, Storage { read_only: false }),
                entry(7, Storage { read_only: false }),
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("grain"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipe = |name: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(name),
                layout: Some(&layout),
                module: &module,
                entry_point: Some(name),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let pipes = Pipes {
            ar_luma: pipe("ar_luma"),
            ar_chroma: pipe("ar_chroma"),
            lumaterm: pipe("lumaterm"),
            std_tpl_luma: pipe("std_tpl_luma"),
            std_tpl_chroma: pipe("std_tpl_chroma"),
            std_offsets: pipe("std_offsets"),
            apply_y: pipe("apply_y"),
            apply_c: pipe("apply_c"),
        };
        if let Some(e) = pollster::block_on(scope.pop()) {
            return err(format!("shader: {e}"));
        }
        let uni = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("params"),
            size: (NPARAM * 4) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let tabs = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tables"),
            size: ((2048 + 3 * 4096) * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let queries = timing.then(|| {
            let qs = device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("pass times"),
                ty: wgpu::QueryType::Timestamp,
                count: QUERIES,
            });
            let qb = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("pass times"),
                size: (QUERIES * 8) as u64,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            (qs, qb)
        });
        let ts_period = queue.get_timestamp_period() as f64;
        Ok(Renderer {
            adapter,
            device,
            queue,
            pipes,
            bgl,
            uni,
            tabs,
            state: None,
            tables_for: None,
            in_flight: VecDeque::new(),
            free_rb: Vec::new(),
            max_in_flight: opts.frames_in_flight.max(1),
            queries,
            ts_period,
            next_id: 0,
            errors,
        })
    }

    /// "name (backend, type)"
    pub fn adapter_name(&self) -> String {
        let i = self.adapter.get_info();
        format!("{} ({:?}, {:?})", i.name, i.backend, i.device_type)
    }

    pub fn adapter_info(&self) -> wgpu::AdapterInfo {
        self.adapter.get_info()
    }

    /// GPU timing is on (requested, and the adapter has timestamp queries)
    pub fn timing(&self) -> bool {
        self.queries.is_some()
    }

    fn check_errors(&self) -> Result<(), Error> {
        let mut e = self.errors.lock().unwrap();
        if e.is_empty() {
            return Ok(());
        }
        let m = e.join("; ");
        e.clear();
        err(format!("GPU: {m}"))
    }

    fn configure(&mut self, f: &Format) -> Result<(), Error> {
        if self.state.as_ref().is_some_and(|s| s.fmt == *f) {
            return Ok(());
        }
        f.validate()?;
        let (w, h) = (f.width, f.height);
        let mono = f.layout == Layout::I400;
        let (ssx, ssy) = (f.layout.ssx(), f.layout.ssy());
        let (cw, ch) = f.chroma_size();
        let ng = h.div_ceil(128);
        let lbw = 16 + w + 8;
        let nchl = lbw.div_ceil(S);
        let lpitch = align(lbw, 4);
        let rsc = ROWS >> ssy;
        let cbw = (16 >> ssx) + cw + (8 >> ssx);
        let nchc = cbw.div_ceil(S);
        let cpitch = align(cbw, 4);
        let ntc = nchc + K * (rsc - 1);
        let mut pb = [0i32; NPARAM];
        let set = |pb: &mut [i32; NPARAM], i: usize, v: usize| pb[i] = v as i32;
        set(&mut pb, p::W, w);
        set(&mut pb, p::H, h);
        set(&mut pb, p::SSX, ssx);
        set(&mut pb, p::SSY, ssy);
        set(&mut pb, p::BD, f.bitdepth as usize);
        set(&mut pb, p::CW, cw);
        set(&mut pb, p::CH, ch);
        set(&mut pb, p::NG, ng);
        set(&mut pb, p::RSL, ROWS);
        set(&mut pb, p::LPITCH, lpitch);
        set(&mut pb, p::LBW, lbw);
        set(&mut pb, p::NCHL, nchl);
        set(&mut pb, p::RSC, rsc);
        set(&mut pb, p::CPITCH, cpitch);
        set(&mut pb, p::CBW, cbw);
        set(&mut pb, p::NCHC, nchc);
        set(&mut pb, p::NTC, ntc);
        set(&mut pb, p::MONO, mono as usize);
        set(&mut pb, p::PMAX, (1usize << f.bitdepth) - 1);
        // output: 16-bit samples, rows padded to an even count
        let (dsy, dsc) = (align(w, 2), align(cw, 2));
        set(&mut pb, p::DOY, 0);
        set(&mut pb, p::DSY, dsy);
        set(&mut pb, p::DOU, dsy * h);
        set(&mut pb, p::DOV, dsy * h + dsc * ch);
        set(&mut pb, p::DSC, dsc);
        let dst_samples = dsy * h + 2 * dsc * ch;
        set(&mut pb, p::NBR, h.div_ceil(32));
        set(&mut pb, p::NBC, w.div_ceil(32));
        // input: packed planes (upload() packs them)
        let bps = f.bytes_per_sample();
        set(&mut pb, p::SOY, 0);
        set(&mut pb, p::SSY_, w * bps);
        set(&mut pb, p::SOU, w * h * bps);
        set(&mut pb, p::SSU, cw * bps);
        set(&mut pb, p::SOV, (w * h + cw * ch) * bps);
        set(&mut pb, p::SSV, cw * bps);
        set(&mut pb, p::BPS, bps);
        set(&mut pb, p::STEPY, bps);
        set(&mut pb, p::STEPC, bps);
        let dev = &self.device;
        let storage = |label: &str, bytes: usize, extra: wgpu::BufferUsages| {
            dev.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: align(bytes.max(16), 4) as u64,
                usage: wgpu::BufferUsages::STORAGE | extra,
                mapped_at_creation: false,
            })
        };
        let nbr = h.div_ceil(32);
        let nbc = w.div_ceil(32);
        let none = wgpu::BufferUsages::empty();
        let sb = storage("templates", (3 * TPL + nbr * nbc) * 4, none);
        let fl = storage("luma field", ng * ROWS * lpitch * 2, none);
        let fc = storage("chroma fields", if mono { 16 } else { 2 * ng * rsc * cpitch * 2 }, none);
        let lt = storage("luma term", if mono { 16 } else { ng * ntc * S * rsc * 4 }, none);
        let dst_bytes = dst_samples * 2;
        let dst = storage("output", dst_bytes, wgpu::BufferUsages::COPY_SRC);
        let src_bytes = f.frame_bytes();
        let src = storage("input", src_bytes, wgpu::BufferUsages::COPY_DST);
        let limit = dev.limits().max_storage_buffer_binding_size as usize;
        for (n, b) in [("luma field", &fl), ("output", &dst), ("input", &src)] {
            if b.size() as usize > limit {
                return err(format!("{n} buffer ({} bytes) exceeds the GPU's storage binding limit ({limit})", b.size()));
            }
        }
        let bind = dev.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("grain"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uni.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: self.tabs.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: fl.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: fc.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: sb.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: src.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 6, resource: dst.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 7, resource: lt.as_entire_binding() },
            ],
        });
        self.free_rb.clear();
        self.state = Some(FormatState {
            fmt: *f,
            p: pb,
            cw,
            ch,
            ng,
            src,
            src_bytes,
            dst,
            dst_bytes,
            bind,
            _fields: [sb, fl, fc, lt],
        });
        Ok(())
    }

    /// the grain of one picture into the parameter block (None: no grain, the picture is copied)
    fn set_grain(&mut self, g: Option<&FilmGrainParams>, mode: GrainMode) -> Result<[i32; NPARAM], Error> {
        let st = self.state.as_ref().unwrap();
        let f = st.fmt;
        let mut pb = st.p;
        pb[p::MODE] = (mode == GrainMode::Standard) as i32;
        let Some(g) = g else {
            pb[p::HASY] = 0;
            pb[p::HASU] = 0;
            pb[p::HASV] = 0;
            return Ok(pb);
        };
        g.validate()?;
        let bd = f.bitdepth as i32;
        let b8 = bd - 8;
        let mono = f.layout == Layout::I400;
        let csfl = g.chroma_scaling_from_luma != 0;
        pb[p::HASY] = (g.num_y_points > 0) as i32;
        pb[p::HASU] = (!mono && (csfl || g.num_uv_points[0] > 0)) as i32;
        pb[p::HASV] = (!mono && (csfl || g.num_uv_points[1] > 0)) as i32;
        pb[p::LAG] = g.ar_coeff_lag;
        pb[p::ARSH] = g.ar_coeff_shift as i32;
        pb[p::GMIN] = -(128 << b8);
        pb[p::GMAX] = (128 << b8) - 1;
        let keys = tables::full_keys(g.seed);
        pb[p::KEYY] = keys[0] as i32;
        pb[p::KEYU] = keys[1] as i32;
        pb[p::KEYV] = keys[2] as i32;
        pb[p::SCSH] = g.scaling_shift;
        pb[p::CSFL] = csfl as i32;
        pb[p::SEED] = g.seed as i32;
        pb[p::OVL] = g.overlap_flag;
        if g.clip_to_restricted_range != 0 {
            pb[p::YMIN] = 16 << b8;
            pb[p::YMAX] = 235 << b8;
            pb[p::CMIN] = 16 << b8;
            pb[p::CMAX] = (if f.matrix == 0 { 235 } else { 240 }) << b8;
        } else {
            pb[p::YMIN] = 0;
            pb[p::CMIN] = 0;
            pb[p::YMAX] = (1 << bd) - 1;
            pb[p::CMAX] = (1 << bd) - 1;
        }
        let npos = g.npos();
        for pl in 0..2 {
            pb[p::ULM + 3 * pl] = g.uv_luma_mult[pl];
            pb[p::UM + 3 * pl] = g.uv_mult[pl];
            pb[p::UOFF + 3 * pl] = g.uv_offset[pl] * (1 << b8);
            pb[p::LCU + pl] = if g.num_y_points > 0 { g.ar_coeffs_uv[pl][npos] as i32 } else { 0 };
            grid(&mut pb, p::GRID + GSTRIDE * (1 + pl), &g.ar_coeffs_uv[pl], g.ar_coeff_lag);
        }
        grid(&mut pb, p::GRID, &g.ar_coeffs_y, g.ar_coeff_lag);
        pb[p::SCPU] = if csfl { 0 } else { 1 };
        pb[p::SCPV] = if csfl { 0 } else { 2 };
        pb[p::CPLANE] = if pb[p::HASU] != 0 { 1 } else { 2 };
        // tables: once per parameter set
        if !self.tables_for.as_ref().is_some_and(|(b, t)| *b == f.bitdepth && t.same_tables(g)) {
            self.tables_for = Some((f.bitdepth, *g));
            self.queue.write_buffer(&self.tabs, 0, as_bytes(&tables::shader_tables(f.bitdepth, g)));
        }
        Ok(pb)
    }

    /// pack the planes into the input buffer
    fn upload(&self, planes: &[Plane]) -> Result<(), Error> {
        let st = self.state.as_ref().unwrap();
        let f = st.fmt;
        let bps = f.bytes_per_sample();
        let np = if f.layout == Layout::I400 { 1 } else { 3 };
        if planes.len() < np {
            return err(format!("{np} planes needed, {} given", planes.len()));
        }
        let size = wgpu::BufferSize::new(align(st.src_bytes, 4) as u64).unwrap();
        let mut view = self.queue.write_buffer_with(&st.src, 0, size).ok_or_else(|| Error("upload failed".into()))?;
        let mut o = 0;
        for (i, pl) in planes.iter().take(np).enumerate() {
            let (w, h) = if i == 0 { (f.width, f.height) } else { (st.cw, st.ch) };
            let row = w * bps;
            if pl.stride < row || pl.data.len() < pl.stride * (h - 1) + row {
                return err(format!("plane {i}: {} bytes with stride {} is too small for {w}x{h}", pl.data.len(), pl.stride));
            }
            if pl.stride == row {
                view.slice(o..o + row * h).copy_from_slice(&pl.data[..row * h]);
            } else {
                for y in 0..h {
                    view.slice(o + y * row..o + (y + 1) * row).copy_from_slice(&pl.data[y * pl.stride..y * pl.stride + row]);
                }
            }
            o += row * h;
        }
        Ok(())
    }

    /// Queue one picture: `planes` are the decoded grain-free picture (Y, U, V; 8-bit samples, or 16-bit
    /// little-endian for 10/12-bit), `grain` its film grain (None: no grain, the picture passes unchanged).
    /// Returns the picture's id; `receive` returns the pictures in order. Waits for the oldest picture when
    /// `frames_in_flight` are queued (call `receive` to get it).
    pub fn submit(&mut self, f: &Format, planes: &[Plane], grain: Option<&FilmGrainParams>, mode: GrainMode)
                  -> Result<u64, Error> {
        if self.in_flight.len() >= self.max_in_flight {
            return err("too many pictures queued: receive() the oldest first");
        }
        if self.state.as_ref().is_none_or(|s| s.fmt != *f) {
            if !self.in_flight.is_empty() {
                return err("picture format changed with pictures queued: receive() them first");
            }
            self.configure(f)?;
        }
        // a picture whose parameters draw nothing (e.g. all zero, inherited from a slot without grain) passes
        // unchanged, as in dav1d
        let grain = grain.filter(|g| g.has_grain());
        let pb = self.set_grain(grain, mode)?;
        self.upload(planes)?;
        self.queue.write_buffer(&self.uni, 0, as_bytes(&pb));
        let st = self.state.as_ref().unwrap();
        let rb_bytes = (align(st.dst_bytes, 8) + (QUERIES as usize) * 8) as u64;
        let rb = self.free_rb.pop().filter(|b| b.size() == rb_bytes).unwrap_or_else(|| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("readback"),
                size: rb_bytes,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("grain") });
        let mut q = 0u32;
        let mut pass = |enc: &mut wgpu::CommandEncoder, pipe: &wgpu::ComputePipeline, x: usize, y: usize, z: usize| {
            let tw = self.queries.as_ref().map(|(qs, _)| {
                let tw = wgpu::ComputePassTimestampWrites {
                    query_set: qs,
                    beginning_of_pass_write_index: Some(q),
                    end_of_pass_write_index: Some(q + 1),
                };
                q += 2;
                tw
            });
            let mut cp = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None, timestamp_writes: tw });
            cp.set_pipeline(pipe);
            cp.set_bind_group(0, &st.bind, &[]);
            cp.dispatch_workgroups(x as u32, y as u32, z as u32);
        };
        let (hasy, hasu, hasv) = (pb[p::HASY] != 0, pb[p::HASU] != 0, pb[p::HASV] != 0);
        let mut kinds: Vec<u8> = vec![];
        if grain.is_some() {
            match mode {
                GrainMode::Full => {
                    if hasy {
                        pass(&mut enc, &self.pipes.ar_luma, st.ng, 1, 1);
                        kinds.push(0);
                    }
                    if hasu || hasv {
                        if pb[p::LCU] != 0 || pb[p::LCV] != 0 {
                            pass(&mut enc, &self.pipes.lumaterm, (pb[p::CBW] as usize).div_ceil(64), pb[p::RSC] as usize, st.ng);
                            kinds.push(1);
                        }
                        pass(&mut enc, &self.pipes.ar_chroma, st.ng, if hasu && hasv { 2 } else { 1 }, 1);
                        kinds.push(1);
                    }
                }
                GrainMode::Standard => {
                    if hasy || pb[p::LCU] != 0 || pb[p::LCV] != 0 {
                        pass(&mut enc, &self.pipes.std_tpl_luma, 1, 1, 1);
                        kinds.push(2);
                    }
                    if hasu || hasv {
                        pass(&mut enc, &self.pipes.std_tpl_chroma, if hasu && hasv { 2 } else { 1 }, 1, 1);
                        kinds.push(2);
                    }
                    pass(&mut enc, &self.pipes.std_offsets, (pb[p::NBR] as usize).div_ceil(64), 1, 1);
                    kinds.push(2);
                }
            }
        }
        let f = st.fmt;
        pass(&mut enc, &self.pipes.apply_y, f.width.div_ceil(32), f.height.div_ceil(8), 1);
        kinds.push(3);
        if f.layout != Layout::I400 {
            pass(&mut enc, &self.pipes.apply_c, st.cw.div_ceil(32), st.ch.div_ceil(8), 2);
            kinds.push(3);
        }
        enc.copy_buffer_to_buffer(&st.dst, 0, &rb, 0, Some(align(st.dst_bytes, 4) as u64));
        let timed = if let Some((qs, qb)) = &self.queries {
            enc.resolve_query_set(qs, 0..q, qb, 0);
            enc.copy_buffer_to_buffer(qb, 0, &rb, align(st.dst_bytes, 8) as u64, Some(q as u64 * 8));
            Some(kinds)
        } else {
            None
        };
        let sub = self.queue.submit([enc.finish()]);
        let state = Arc::new(AtomicU8::new(0));
        {
            let state = state.clone();
            rb.map_async(wgpu::MapMode::Read, .., move |r| state.store(if r.is_ok() { 1 } else { 2 }, Ordering::Release));
        }
        self.check_errors()?;
        let id = self.next_id;
        self.next_id += 1;
        let p_copy = pb;
        self.in_flight.push_back(InFlight {
            id,
            sub,
            rb,
            state,
            fmt: f,
            p: p_copy,
            timed,
        });
        Ok(id)
    }

    /// pictures queued
    pub fn pending(&self) -> usize {
        self.in_flight.len()
    }

    /// room for another submit without receiving first
    pub fn can_submit(&self) -> bool {
        self.in_flight.len() < self.max_in_flight
    }

    /// Wait for the oldest queued picture and write it to `out` as packed planes (Y, U, V; rows without padding;
    /// 8-bit samples, or 16-bit little-endian for 10/12-bit: `Format::frame_bytes()`). Returns its id and, with
    /// timing, its GPU time. None when nothing is queued.
    pub fn receive(&mut self, out: &mut [u8]) -> Result<Option<(u64, Option<PassTimes>)>, Error> {
        let Some(f) = self.in_flight.front().map(|x| x.fmt) else { return Ok(None) };
        let need = f.frame_bytes();
        if out.len() < need {
            return err(format!("output buffer: {} bytes, {need} needed", out.len()));
        }
        let bps = f.bytes_per_sample();
        let (cw, ch) = f.chroma_size();
        let (y, uv) = out[..need].split_at_mut(f.width * f.height * bps);
        let (u, v) = uv.split_at_mut(cw * ch * bps);
        let mut planes = [PlaneMut { data: y, stride: f.width * bps }, PlaneMut { data: u, stride: cw * bps },
                          PlaneMut { data: v, stride: cw * bps }];
        self.receive_planes(&mut planes)
    }

    /// `receive` into separate planes with their own strides (bytes).
    pub fn receive_planes(&mut self, out: &mut [PlaneMut]) -> Result<Option<(u64, Option<PassTimes>)>, Error> {
        let Some(fl) = self.in_flight.pop_front() else { return Ok(None) };
        self.device
            .poll(wgpu::PollType::Wait { submission_index: Some(fl.sub.clone()), timeout: None })
            .map_err(|e| Error(format!("GPU wait: {e}")))?;
        self.check_errors()?;
        match fl.state.load(Ordering::Acquire) {
            1 => {}
            0 => return err("GPU readback not mapped after the wait"),
            _ => return err("GPU readback failed"),
        }
        let f = fl.fmt;
        let (cw, ch) = f.chroma_size();
        let planes = if f.layout == Layout::I400 { 1 } else { 3 };
        let bps = f.bytes_per_sample();
        for (i, o) in out.iter().enumerate().take(planes) {
            let (w, h) = if i == 0 { (f.width, f.height) } else { (cw, ch) };
            if o.stride < w * bps || o.data.len() < o.stride * (h - 1) + w * bps {
                fl.rb.unmap();
                return err(format!("output plane {i}: {} bytes with stride {} is too small", o.data.len(), o.stride));
            }
        }
        if out.len() < planes {
            fl.rb.unmap();
            return err(format!("{planes} output planes needed"));
        }
        let mut times = None;
        {
            let view = fl.rb.get_mapped_range(..).map_err(|e| Error(format!("readback: {e}")))?;
            let data: &[u8] = &view;
            for (pl, o) in out.iter_mut().enumerate().take(planes) {
                let (w, h, off, stride) = if pl == 0 {
                    (f.width, f.height, fl.p[p::DOY] as usize, fl.p[p::DSY] as usize)
                } else {
                    (cw, ch, fl.p[if pl == 1 { p::DOU } else { p::DOV }] as usize, fl.p[p::DSC] as usize)
                };
                for y in 0..h {
                    let src = &data[(off + y * stride) * 2..(off + y * stride + w) * 2];
                    let dst = &mut o.data[y * o.stride..y * o.stride + w * bps];
                    if bps == 2 {
                        dst.copy_from_slice(src);
                    } else {
                        for (d, s) in dst.iter_mut().zip(src.chunks_exact(2)) {
                            *d = s[0];
                        }
                    }
                }
            }
            if let Some(kinds) = &fl.timed {
                let n = kinds.len();
                let base = align(self.state.as_ref().map_or(0, |s| s.dst_bytes), 8);
                let ts: Vec<u64> = (0..2 * n)
                    .map(|i| u64::from_le_bytes(data[base + 8 * i..base + 8 * i + 8].try_into().unwrap()))
                    .collect();
                let ms = |i: usize| {
                    if ts[2 * i + 1] > ts[2 * i] { (ts[2 * i + 1] - ts[2 * i]) as f64 * self.ts_period / 1e6 } else { 0.0 }
                };
                let mut t = PassTimes::default();
                for (i, k) in kinds.iter().enumerate() {
                    let v = ms(i);
                    match k {
                        0 => t.ar_luma += v,
                        1 => t.ar_chroma += v,
                        2 => t.templates += v,
                        _ => t.apply += v,
                    }
                }
                t.total = if ts[2 * n - 1] > ts[0] { (ts[2 * n - 1] - ts[0]) as f64 * self.ts_period / 1e6 } else { 0.0 };
                times = Some(t);
            }
        }
        fl.rb.unmap();
        if self.free_rb.len() < self.max_in_flight + 1 {
            self.free_rb.push(fl.rb);
        }
        Ok(Some((fl.id, times)))
    }

    /// submit + receive: one picture, synchronously
    pub fn render(&mut self, f: &Format, planes: &[Plane], grain: Option<&FilmGrainParams>, mode: GrainMode,
                  out: &mut [u8]) -> Result<Option<PassTimes>, Error> {
        if !self.in_flight.is_empty() {
            return err("render() with pictures queued: receive() them first");
        }
        self.submit(f, planes, grain, mode)?;
        Ok(self.receive(out)?.and_then(|(_, t)| t))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_the_shader() {
        let mut found = std::collections::HashMap::new();
        for line in WGSL.lines() {
            if let Some(rest) = line.strip_prefix("const ") {
                if let Some((name, v)) = rest.split_once(" = ") {
                    if let Ok(n) = v.split(';').next().unwrap().trim().parse::<usize>() {
                        found.insert(name.trim().to_string(), n);
                    }
                }
            }
        }
        let want: &[(&str, usize)] = &[
            ("ROWS", ROWS), ("S", S), ("K", K), ("TPL", TPL), ("GSTRIDE", GSTRIDE), ("NPARAM", NPARAM),
            ("P_W", p::W), ("P_H", p::H), ("P_SSX", p::SSX), ("P_SSY", p::SSY), ("P_BD", p::BD), ("P_CW", p::CW),
            ("P_CH", p::CH), ("P_NG", p::NG), ("P_LAG", p::LAG), ("P_ARSH", p::ARSH), ("P_GMIN", p::GMIN),
            ("P_GMAX", p::GMAX), ("P_HASY", p::HASY), ("P_HASU", p::HASU), ("P_HASV", p::HASV), ("P_KEYY", p::KEYY),
            ("P_KEYU", p::KEYU), ("P_KEYV", p::KEYV), ("P_SCSH", p::SCSH), ("P_YMIN", p::YMIN), ("P_YMAX", p::YMAX),
            ("P_CMIN", p::CMIN), ("P_CMAX", p::CMAX), ("P_CSFL", p::CSFL), ("P_ULM", p::ULM), ("P_UM", p::UM),
            ("P_UOFF", p::UOFF), ("P_VLM", p::VLM), ("P_VM", p::VM), ("P_VOFF", p::VOFF), ("P_LCU", p::LCU),
            ("P_LCV", p::LCV), ("P_RSL", p::RSL), ("P_LPITCH", p::LPITCH), ("P_LBW", p::LBW), ("P_NCHL", p::NCHL),
            ("P_RSC", p::RSC), ("P_CPITCH", p::CPITCH), ("P_CBW", p::CBW), ("P_NCHC", p::NCHC), ("P_SOY", p::SOY),
            ("P_SSY_", p::SSY_), ("P_SOU", p::SOU), ("P_SSU", p::SSU), ("P_SOV", p::SOV), ("P_SSV", p::SSV),
            ("P_BPS", p::BPS), ("P_DOY", p::DOY), ("P_DOU", p::DOU), ("P_DOV", p::DOV), ("P_DSY", p::DSY),
            ("P_DSC", p::DSC), ("P_MONO", p::MONO), ("P_SCPU", p::SCPU), ("P_SCPV", p::SCPV), ("P_PMAX", p::PMAX),
            ("P_CPLANE", p::CPLANE), ("P_NTC", p::NTC), ("P_MODE", p::MODE), ("P_SEED", p::SEED), ("P_OVL", p::OVL),
            ("P_NBR", p::NBR), ("P_NBC", p::NBC), ("P_STEPY", p::STEPY), ("P_GRID", p::GRID), ("P_STEPC", p::STEPC),
        ];
        for (n, v) in want {
            assert_eq!(found.get(*n), Some(v), "{n}");
        }
        for k in found.keys().filter(|k| k.starts_with("P_")) {
            assert!(want.iter().any(|(n, _)| n == k), "{k} is in the shader, not in gpu.rs");
        }
    }

    #[test]
    fn browser_copy_in_sync() {
        // browser/web/grain-wgsl.mjs must be grain.wgsl, wrapped (browser/sync-wgsl.sh writes it)
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../browser/web/grain-wgsl.mjs");
        let Ok(js) = std::fs::read_to_string(path) else { return };
        let a = js.find("export const WGSL = `").expect("grain-wgsl.mjs: no WGSL export") + "export const WGSL = `".len();
        let b = js.rfind("`;").unwrap();
        assert!(js[a..b] == *WGSL, "browser/web/grain-wgsl.mjs is out of date: run browser/sync-wgsl.sh");
    }
}
