//! av1-fullgrain-gpu: AV1 film grain drawn full-frame on the GPU, from the command line.
//!
//! Copyright © 2026, divergentnn (av1-fullgrain). SPDX-License-Identifier: BSD-2-Clause

use av1_fullgrain_gpu::{
    list_adapters, parse_showinfo_line, parse_showinfo_line_from, y4m, SideDataFrom, FilmGrainParams, Format, GrainMode, Layout, Options, PassTimes, Plane,
    ParamsFile, Renderer,
};
use clap::{Args, Parser, Subcommand, ValueEnum};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Instant;

type Res<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

macro_rules! bail {
    ($($t:tt)*) => { return Err(format!($($t)*).into()) };
}

#[derive(Parser)]
#[command(name = "av1-fullgrain-gpu", version, about = "AV1 film grain drawn full-frame on the GPU (Vulkan, Metal, DX12), \
bit-identical to av1-fullgrain's dav1d")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq, Debug)]
enum Mode {
    /// full-frame grain
    Full,
    /// standard AV1 grain (32x32 tiles of one 82x73 patch), as stock dav1d draws it
    Standard,
}

impl From<Mode> for GrainMode {
    fn from(m: Mode) -> GrainMode {
        match m {
            Mode::Full => GrainMode::Full,
            Mode::Standard => GrainMode::Standard,
        }
    }
}

#[derive(Args, Clone)]
struct Gpu {
    /// GPU to use: part of its name (see `adapters`); default: the high-performance one
    #[arg(long)]
    adapter: Option<String>,
    /// pictures in flight on the GPU
    #[arg(long, default_value_t = 3)]
    frames_in_flight: usize,
}

impl Gpu {
    fn open(&self, timing: bool) -> Res<Renderer> {
        let r = Renderer::new(&Options {
            adapter: self.adapter.clone(),
            frames_in_flight: self.frames_in_flight,
            timing,
            ..Default::default()
        })?;
        Ok(r)
    }
}

#[derive(Subcommand)]
enum Cmd {
    /// Decode a video with ffmpeg (AV1 grain off, its parameters exported), draw the grain on the GPU, write y4m
    Decode {
        input: PathBuf,
        /// output y4m (- for stdout)
        #[arg(short, long, default_value = "-")]
        output: PathBuf,
        #[arg(long, value_enum, default_value_t = Mode::Full)]
        mode: Mode,
        /// raw planes instead of y4m
        #[arg(long)]
        raw: bool,
        /// start time (ffmpeg -ss, before the input: frame-accurate)
        #[arg(long)]
        ss: Option<String>,
        /// pictures to write
        #[arg(long)]
        frames: Option<usize>,
        /// decode in hardware (ffmpeg -hwaccel, e.g. cuda, vaapi, videotoolbox, d3d11va); the decoder's output is
        /// grain-free and identical to dav1d's
        #[arg(long)]
        hwaccel: Option<String>,
        /// ffmpeg decoder threads
        #[arg(long, default_value_t = 0)]
        threads: usize,
        #[arg(long, default_value = "ffmpeg")]
        ffmpeg: String,
        /// also write the grain as an av1-fullgrain-params JSON file (the browser player's PARAMS.json)
        #[arg(long)]
        params_out: Option<PathBuf>,
        #[command(flatten)]
        gpu: Gpu,
    },
    /// Draw grain on a y4m stream of grain-free pictures, from a grain file
    Render {
        /// grain: av1-fullgrain-params JSON (`av1grain strip`, `decode --params-out`) or an ffmpeg log with
        /// showinfo's film grain side data
        #[arg(long)]
        params: PathBuf,
        /// input y4m (- for stdin)
        #[arg(short, long, default_value = "-")]
        input: PathBuf,
        /// output y4m (- for stdout)
        #[arg(short, long, default_value = "-")]
        output: PathBuf,
        #[arg(long, value_enum, default_value_t = Mode::Full)]
        mode: Mode,
        /// matrix_coefficients of the stream (0 = identity / RGB: restricted-range chroma clips at 235)
        #[arg(long, default_value_t = 2)]
        matrix: u32,
        /// picture index in the grain file of the first y4m frame
        #[arg(long, default_value_t = 0)]
        first: usize,
        #[command(flatten)]
        gpu: Gpu,
    },
    /// Exactness harness: grain drawn on the GPU vs av1-fullgrain's dav1d, picture by picture (SHA-256)
    Check {
        /// AV1 streams (IVF, OBU, or anything ffmpeg reads: then remuxed to IVF for dav1d)
        streams: Vec<PathBuf>,
        /// the dav1d command line tool of av1-fullgrain (build/decoder/dav1d/tools/dav1d)
        #[arg(long)]
        dav1d: PathBuf,
        #[arg(long, default_value = "ffmpeg")]
        ffmpeg: String,
        /// grain modes to check
        #[arg(long, value_delimiter = ',', default_value = "full,standard")]
        modes: Vec<Mode>,
        /// only the first N pictures
        #[arg(long)]
        frames: Option<usize>,
        /// also take the grain from av1grain strip (PARAMS.json) and require it to equal ffmpeg's
        #[arg(long)]
        av1grain: Option<PathBuf>,
        /// decoder threads
        #[arg(long, default_value_t = 4)]
        threads: usize,
        /// ffmpeg -hwaccel for the grain-free decode (checks a hardware decoder's output too)
        #[arg(long)]
        hwaccel: Option<String>,
        /// write the results as JSON lines here
        #[arg(long)]
        json: Option<PathBuf>,
        /// short name of each stream in the table (default: file name), comma-separated
        #[arg(long, value_delimiter = ',')]
        names: Vec<String>,
        #[command(flatten)]
        gpu: Gpu,
    },
    /// Speed on synthetic pictures: GPU time per picture and throughput with upload and readback
    Bench {
        /// picture sizes, e.g. 1920x1080,3840x2160
        #[arg(long, value_delimiter = ',', default_value = "1920x1080,3840x2160")]
        sizes: Vec<String>,
        #[arg(long, default_value_t = 10)]
        bitdepth: u32,
        #[arg(long, default_value_t = 200)]
        frames: usize,
        #[arg(long, value_enum, default_value_t = Mode::Full)]
        mode: Mode,
        /// grain parameters: av1-fullgrain-params JSON (its first set); default: a typical SVT-AV1 set
        #[arg(long)]
        params: Option<PathBuf>,
        #[command(flatten)]
        gpu: Gpu,
    },
    /// List the GPUs wgpu can use
    Adapters,
}

fn main() {
    let cli = Cli::parse();
    let r = match cli.cmd {
        Cmd::Decode { input, output, mode, raw, ss, frames, hwaccel, threads, ffmpeg, params_out, gpu } => {
            decode(&input, &output, mode.into(), raw, ss.as_deref(), frames, hwaccel.as_deref(), threads, &ffmpeg,
                   params_out.as_deref(), &gpu)
        }
        Cmd::Render { params, input, output, mode, matrix, first, gpu } => {
            render(&params, &input, &output, mode.into(), matrix, first, &gpu)
        }
        Cmd::Check { streams, dav1d, ffmpeg, modes, frames, av1grain, threads, hwaccel, json, names, gpu } => {
            check(&streams, &dav1d, &ffmpeg, &modes, frames, av1grain.as_deref(), threads, hwaccel.as_deref(),
                  json.as_deref(), &names, &gpu)
        }
        Cmd::Bench { sizes, bitdepth, frames, mode, params, gpu } => {
            bench(&sizes, bitdepth, frames, mode.into(), params.as_deref(), &gpu)
        }
        Cmd::Adapters => {
            for a in list_adapters() {
                println!("{a}");
            }
            Ok(())
        }
    };
    if let Err(e) = r {
        eprintln!("av1-fullgrain-gpu: {e}");
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------------------------------------------------------

fn open_out(p: &Path) -> Res<Box<dyn Write + Send>> {
    Ok(if p == Path::new("-") {
        Box::new(BufWriter::with_capacity(1 << 20, io::stdout()))
    } else {
        Box::new(BufWriter::with_capacity(1 << 20, std::fs::File::create(p)?))
    })
}

fn planes_of<'a>(f: &Format, buf: &'a [u8]) -> Vec<Plane<'a>> {
    let bps = f.bytes_per_sample();
    let (cw, ch) = f.chroma_size();
    let (y, uv) = buf.split_at(f.width * f.height * bps);
    if f.layout == Layout::I400 {
        return vec![Plane { data: y, stride: f.width * bps }];
    }
    let (u, v) = uv.split_at(cw * ch * bps);
    vec![Plane { data: y, stride: f.width * bps }, Plane { data: u, stride: cw * bps }, Plane { data: v, stride: cw * bps }]
}

/// Runs frames through the renderer with pictures in flight. `next` fills a buffer with the next input picture and
/// returns its grain (Ok(None) at the end); `done` gets each output picture in order.
fn pipeline(
    r: &mut Renderer,
    f: &Format,
    mode: GrainMode,
    mut next: impl FnMut(&mut Vec<u8>) -> Res<Option<Option<FilmGrainParams>>>,
    mut done: impl FnMut(&[u8], Option<PassTimes>) -> Res<()>,
) -> Res<usize> {
    let mut inbuf = vec![0u8; f.frame_bytes()];
    let mut out = vec![0u8; f.frame_bytes()];
    let mut n = 0;
    let mut eof = false;
    loop {
        while !eof && r.can_submit() {
            match next(&mut inbuf)? {
                Some(g) => {
                    r.submit(f, &planes_of(f, &inbuf), g.as_ref(), mode)?;
                }
                None => eof = true,
            }
        }
        match r.receive(&mut out)? {
            Some((_, t)) => {
                done(&out, t)?;
                n += 1;
            }
            None => break,
        }
    }
    Ok(n)
}

// ---------------------------------------------------------------------------------------------------------------
// ffmpeg: grain-free decode + film grain side data

struct Probe {
    format: Format,
    fps: (u32, u32),
    /// the software decoder's pixel format (gbrp* for an identity matrix: G, B, R = dav1d's Y, U, V planes)
    pix_fmt: String,
}

fn probe(ffprobe: &str, input: &Path) -> Res<Probe> {
    let o = Command::new(ffprobe)
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries",
               "stream=codec_name,width,height,pix_fmt,color_space,r_frame_rate", "-of", "default=nw=1"])
        .arg(input)
        .output()
        .map_err(|e| format!("running {ffprobe}: {e}"))?;
    if !o.status.success() {
        bail!("ffprobe {}: {}", input.display(), String::from_utf8_lossy(&o.stderr).trim());
    }
    let mut kv = BTreeMap::new();
    for l in String::from_utf8_lossy(&o.stdout).lines() {
        if let Some((k, v)) = l.split_once('=') {
            kv.insert(k.to_string(), v.to_string());
        }
    }
    let g = |k: &str| kv.get(k).cloned().unwrap_or_default();
    let pf = g("pix_fmt");
    let (layout, bitdepth) = match pf.as_str() {
        "yuv420p" | "yuvj420p" => (Layout::I420, 8),
        "yuv422p" | "yuvj422p" => (Layout::I422, 8),
        "yuv444p" | "yuvj444p" | "gbrp" => (Layout::I444, 8),
        "gray" => (Layout::I400, 8),
        "yuv420p10le" => (Layout::I420, 10),
        "yuv422p10le" => (Layout::I422, 10),
        "yuv444p10le" | "gbrp10le" => (Layout::I444, 10),
        "gray10le" => (Layout::I400, 10),
        "yuv420p12le" => (Layout::I420, 12),
        "yuv422p12le" => (Layout::I422, 12),
        "yuv444p12le" | "gbrp12le" => (Layout::I444, 12),
        "gray12le" => (Layout::I400, 12),
        _ => bail!("{}: pixel format {pf:?} not supported", input.display()),
    };
    let matrix = if g("color_space") == "gbr" || pf.starts_with("gbr") { 0 } else { 2 };
    let fps = g("r_frame_rate").split_once('/').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?))).unwrap_or((24, 1));
    Ok(Probe {
        format: Format {
            width: g("width").parse().map_err(|_| "ffprobe: no width")?,
            height: g("height").parse().map_err(|_| "ffprobe: no height")?,
            bitdepth,
            layout,
            matrix,
        },
        fps,
        pix_fmt: pf,
    })
}

fn ffprobe_of(ffmpeg: &str) -> String {
    // ffprobe next to the given ffmpeg
    let p = Path::new(ffmpeg);
    match (p.parent(), p.file_name().and_then(|n| n.to_str())) {
        (Some(d), Some(n)) if !d.as_os_str().is_empty() => d.join(n.replacen("ffmpeg", "ffprobe", 1)).display().to_string(),
        _ => "ffprobe".to_string(),
    }
}

/// ffmpeg decoding grain-free (native pixel format, raw planes on stdout), with the film grain of every picture
/// read from showinfo on stderr
struct GrainFree {
    child: Child,
    out: Box<dyn Read + Send>,
    grain: mpsc::Receiver<(usize, Option<FilmGrainParams>)>,
    errs: Option<std::thread::JoinHandle<String>>,
    n: usize,
    pending: BTreeMap<usize, Option<FilmGrainParams>>,
}

impl GrainFree {
    fn start(ffmpeg: &str, input: &Path, ss: Option<&str>, frames: Option<usize>, hwaccel: Option<&str>, threads: usize,
             pix_fmt: &str) -> Res<GrainFree> {
        let mut cmd = Command::new(ffmpeg);
        cmd.args(["-hide_banner", "-nostdin", "-nostats", "-v", "info"]);
        if threads > 0 {
            cmd.args(["-threads", &threads.to_string()]);
        }
        match hwaccel {
            Some(h) => {
                cmd.args(["-hwaccel", h]);
            }
            None => {
                cmd.args(["-c:v", "libdav1d"]);
            }
        }
        // export the grain instead of applying it (libdav1d then decodes without grain)
        cmd.args(["-export_side_data", "film_grain"]);
        if let Some(ss) = ss {
            cmd.args(["-ss", ss]);
        }
        cmd.arg("-i").arg(input);
        cmd.args(["-map", "0:v:0", "-vf", "showinfo", "-fps_mode", "passthrough"]);
        if let Some(n) = frames {
            cmd.args(["-frames:v", &n.to_string()]);
        }
        // the picture as the decoder made it, planar (hardware decoders hand back NV12 / P010: converted losslessly)
        cmd.args(["-pix_fmt", pix_fmt, "-f", "rawvideo", "-"]);
        // an av1-fullgrain libdav1d installed as the system's may force its grain on: keep this decode grain-free
        for (k, _) in std::env::vars() {
            if k.starts_with("DAV1D_GRAIN") {
                cmd.env_remove(k);
            }
        }
        cmd.env("DAV1D_GRAIN_FORCE", "0");
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| format!("starting {ffmpeg}: {e}"))?;
        let out = Box::new(BufReader::with_capacity(1 << 22, child.stdout.take().unwrap()));
        let err = child.stderr.take().unwrap();
        let (tx, rx) = mpsc::channel();
        let from = if hwaccel.is_some() { SideDataFrom::FfmpegAv1 } else { SideDataFrom::Libdav1d };
        let errs = std::thread::spawn(move || {
            let mut text = String::new();
            let mut cur: Option<(usize, Option<FilmGrainParams>)> = None;
            let mut bad = None;
            for line in BufReader::new(err).lines().map_while(|l| l.ok()) {
                if line.contains("Parsed_showinfo") {
                    if let Some(p) = line.find(" n:") {
                        if let Some(c) = cur.take() {
                            let _ = tx.send(c);
                        }
                        let n = line[p + 3..].split_whitespace().next().and_then(|v| v.parse().ok());
                        cur = n.map(|n| (n, None));
                    } else if line.contains("Film grain parameters") {
                        match parse_showinfo_line_from(&line, from) {
                            Ok(g) => {
                                if let Some(c) = cur.as_mut() {
                                    c.1 = g;
                                }
                            }
                            Err(e) => bad = Some(e.to_string()),
                        }
                    } else if line.contains("color_range:") {
                        // the last line of a frame's block
                        if let Some(c) = cur.take() {
                            let _ = tx.send(c);
                        }
                    }
                } else if !line.starts_with("  ") && !line.starts_with("Input #") && !line.starts_with("Output #")
                    && !line.starts_with("Stream") && !line.contains("deprecated") && text.len() < 4000 {
                    text.push_str(&line);
                    text.push('\n');
                }
            }
            if let Some(c) = cur.take() {
                let _ = tx.send(c);
            }
            if let Some(b) = bad {
                text.push_str(&format!("film grain side data: {b}\n"));
            }
            text
        });
        Ok(GrainFree { child, out, grain: rx, errs: Some(errs), n: 0, pending: BTreeMap::new() })
    }

    /// the next picture into buf; Ok(None) at the end
    fn next(&mut self, buf: &mut [u8]) -> Res<Option<Option<FilmGrainParams>>> {
        let mut got = 0;
        while got < buf.len() {
            let k = self.out.read(&mut buf[got..])?;
            if k == 0 {
                break;
            }
            got += k;
        }
        if got == 0 {
            return Ok(None);
        }
        if got < buf.len() {
            bail!("ffmpeg: truncated picture ({got} of {} bytes)", buf.len());
        }
        let n = self.n;
        self.n += 1;
        // showinfo logged picture n before ffmpeg wrote it
        while !self.pending.contains_key(&n) {
            match self.grain.recv() {
                Ok((k, g)) => {
                    self.pending.insert(k, g);
                }
                Err(_) => bail!("ffmpeg: no showinfo line for picture {n}"),
            }
        }
        Ok(Some(self.pending.remove(&n).unwrap()))
    }

    fn finish(mut self) -> Res<()> {
        let mut sink = [0u8; 1 << 16];
        while self.out.read(&mut sink).unwrap_or(0) > 0 {}
        let st = self.child.wait()?;
        let text = self.errs.take().map(|h| h.join().unwrap_or_default()).unwrap_or_default();
        if !st.success() || text.contains("film grain side data:") {
            bail!("ffmpeg failed: {}", text.trim());
        }
        Ok(())
    }
}

impl Drop for GrainFree {
    fn drop(&mut self) {
        let _ = self.child.kill(); // our own child, by handle
        let _ = self.child.wait();
    }
}

/// av1-fullgrain-params v1 JSON for a list of pictures
fn params_json(f: &Format, grain: &[Option<FilmGrainParams>]) -> String {
    let mut sets: Vec<FilmGrainParams> = vec![];
    let mut per = vec![];
    for g in grain {
        match g {
            None => per.push("[-1,0]".to_string()),
            Some(g) => {
                let i = match sets.iter().position(|s| s.same_tables(g)) {
                    Some(i) => i,
                    None => {
                        sets.push(*g);
                        sets.len() - 1
                    }
                };
                per.push(format!("[{i},{}]", g.seed));
            }
        }
    }
    let pts = |p: &[[u8; 2]]| p.iter().map(|x| format!("[{},{}]", x[0], x[1])).collect::<Vec<_>>().join(",");
    let ints = |v: &[i8]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",");
    let s: Vec<String> = sets
        .iter()
        .map(|g| {
            let npos = g.npos();
            format!(
                "{{\"num_y_points\":{},\"y_points\":[{}],\"chroma_scaling_from_luma\":{},\"num_uv_points\":[{},{}],\
                 \"uv_points\":[[{}],[{}]],\"scaling_shift\":{},\"ar_coeff_lag\":{},\"ar_coeffs_y\":[{}],\
                 \"ar_coeffs_uv\":[[{}],[{}]],\"ar_coeff_shift\":{},\"grain_scale_shift\":{},\"uv_mult\":[{},{}],\
                 \"uv_luma_mult\":[{},{}],\"uv_offset\":[{},{}],\"overlap_flag\":{},\"clip_to_restricted_range\":{}}}",
                g.num_y_points, pts(&g.y_points[..g.num_y_points as usize]), g.chroma_scaling_from_luma,
                g.num_uv_points[0], g.num_uv_points[1], pts(&g.uv_points[0][..g.num_uv_points[0] as usize]),
                pts(&g.uv_points[1][..g.num_uv_points[1] as usize]), g.scaling_shift, g.ar_coeff_lag,
                ints(&g.ar_coeffs_y[..npos]), ints(&g.ar_coeffs_uv[0][..npos + 1]), ints(&g.ar_coeffs_uv[1][..npos + 1]),
                g.ar_coeff_shift, g.grain_scale_shift, g.uv_mult[0], g.uv_mult[1], g.uv_luma_mult[0], g.uv_luma_mult[1],
                g.uv_offset[0], g.uv_offset[1], g.overlap_flag, g.clip_to_restricted_range
            )
        })
        .collect();
    format!(
        "{{\"format\":\"av1-fullgrain-params\",\"version\":1,\n\"width\":{},\"height\":{},\"bitdepth\":{},\"layout\":{},\
         \"mtrx\":{},\"frames\":{},\n\"sets\":[\n{}],\n\"grain\":[{}]}}\n",
        f.width, f.height, f.bitdepth, f.layout as i32, f.matrix, grain.len(), s.join(",\n"), per.join(",")
    )
}

#[allow(clippy::too_many_arguments)]
fn decode(input: &Path, output: &Path, mode: GrainMode, raw: bool, ss: Option<&str>, frames: Option<usize>,
          hwaccel: Option<&str>, threads: usize, ffmpeg: &str, params_out: Option<&Path>, gpu: &Gpu) -> Res<()> {
    let pr = probe(&ffprobe_of(ffmpeg), input)?;
    let f = pr.format;
    let mut r = gpu.open(false)?;
    eprintln!("av1-fullgrain-gpu: {} {}x{} {}-bit, {} grain on {}", input.display(), f.width, f.height, f.bitdepth,
              mode.name(), r.adapter_name());
    let mut src = GrainFree::start(ffmpeg, input, ss, frames, hwaccel, threads, &pr.pix_fmt)?;
    let mut out = open_out(output)?;
    if !raw {
        y4m::Header::new(f, pr.fps).write(&mut out)?;
    }
    let mut grain_log = vec![];
    let t0 = Instant::now();
    let n = pipeline(&mut r, &f, mode, |buf| {
        let g = src.next(buf)?;
        if let Some(g) = &g {
            grain_log.push(*g);
        }
        Ok(g)
    }, |pic, _| {
        if !raw {
            out.write_all(b"FRAME\n")?;
        }
        out.write_all(pic)?;
        Ok(())
    })?;
    out.flush()?;
    src.finish()?;
    let with = grain_log.iter().filter(|g| g.is_some()).count();
    eprintln!("av1-fullgrain-gpu: {n} pictures ({with} with grain) in {:.1} s", t0.elapsed().as_secs_f64());
    if let Some(p) = params_out {
        std::fs::write(p, params_json(&f, &grain_log))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------

/// the grain of every picture from a file: av1-fullgrain-params JSON, or an ffmpeg log with showinfo lines
fn load_grain(path: &Path) -> Res<(Vec<Option<FilmGrainParams>>, Option<Format>)> {
    let text = std::fs::read_to_string(path)?;
    if text.trim_start().starts_with('{') {
        let p = ParamsFile::parse(&text)?;
        return Ok(((0..p.grain.len()).map(|i| p.picture(i)).collect(), Some(p.format)));
    }
    let mut out: Vec<Option<FilmGrainParams>> = vec![];
    let mut cur: Option<usize> = None;
    for line in text.lines() {
        if !line.contains("Parsed_showinfo") {
            continue;
        }
        if let Some(p) = line.find(" n:") {
            cur = line[p + 3..].split_whitespace().next().and_then(|v| v.parse().ok());
            if let Some(n) = cur {
                if out.len() <= n {
                    out.resize(n + 1, None);
                }
            }
        } else if let (Some(n), Some(g)) = (cur, parse_showinfo_line(line)?) {
            out[n] = Some(g);
        }
    }
    if out.is_empty() {
        bail!("{}: neither av1-fullgrain-params JSON nor an ffmpeg log with showinfo lines", path.display());
    }
    Ok((out, None))
}

fn render(params: &Path, input: &Path, output: &Path, mode: GrainMode, matrix: u32, first: usize, gpu: &Gpu) -> Res<()> {
    let (grain, pf) = load_grain(params)?;
    let rd: Box<dyn BufRead + Send> = if input == Path::new("-") {
        Box::new(BufReader::with_capacity(1 << 22, io::stdin()))
    } else {
        Box::new(BufReader::with_capacity(1 << 22, std::fs::File::open(input)?))
    };
    let mut rd = y4m::Reader::new(rd)?;
    let mut f = rd.header.format;
    f.matrix = pf.map(|p| p.matrix).unwrap_or(matrix);
    if let Some(p) = pf {
        if (p.width, p.height, p.bitdepth, p.layout) != (f.width, f.height, f.bitdepth, f.layout) {
            bail!("the grain file is for {}x{} {}-bit {:?}, the y4m is {}x{} {}-bit {:?}", p.width, p.height, p.bitdepth,
                  p.layout, f.width, f.height, f.bitdepth, f.layout);
        }
    }
    let mut r = gpu.open(false)?;
    let mut out = open_out(output)?;
    rd.header.write(&mut out)?;
    let mut i = first;
    let mut missing = 0;
    let n = pipeline(&mut r, &f, mode, |buf| {
        if !rd.read_frame(buf)? {
            return Ok(None);
        }
        let g = match grain.get(i) {
            Some(g) => *g,
            None => {
                missing += 1;
                None
            }
        };
        i += 1;
        Ok(Some(g))
    }, |pic, _| {
        y4m::write_frame(&mut out, pic)?;
        Ok(())
    })?;
    out.flush()?;
    if missing > 0 {
        eprintln!("av1-fullgrain-gpu: warning: {missing} of {n} pictures had no entry in the grain file (left grain-free)");
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// check

fn hashes_of(mut rd: impl Read, size: usize, limit: Option<usize>) -> Res<Vec<String>> {
    let mut buf = vec![0u8; size];
    let mut out = vec![];
    loop {
        if limit.is_some_and(|l| out.len() >= l) {
            break;
        }
        let mut got = 0;
        while got < size {
            let k = rd.read(&mut buf[got..])?;
            if k == 0 {
                break;
            }
            got += k;
        }
        if got == 0 {
            break;
        }
        if got < size {
            bail!("truncated picture ({got} of {size} bytes)");
        }
        out.push(hex(&Sha256::digest(&buf)));
    }
    Ok(out)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// dav1d (the fork's CLI) decoding with full-frame, standard or no grain -> SHA-256 per picture
fn dav1d_hashes(dav1d: &Path, ivf: &Path, grain: Option<GrainMode>, f: &Format, threads: usize, limit: Option<usize>)
                -> Res<Vec<String>> {
    let mut cmd = Command::new(dav1d);
    cmd.args(["-q", "--muxer", "yuv", "-o", "-", "--threads", &threads.to_string()]).arg("-i").arg(ivf);
    for (k, _) in std::env::vars() {
        if k.starts_with("DAV1D_GRAIN") {
            cmd.env_remove(k);
        }
    }
    match grain {
        Some(m) => {
            cmd.args(["--filmgrain", "1"]).env("DAV1D_GRAIN_MODE", m.name()).env("DAV1D_GRAIN_FORCE", "1");
        }
        None => {
            cmd.args(["--filmgrain", "0"]).env("DAV1D_GRAIN_FORCE", "0");
        }
    }
    if let Some(n) = limit {
        cmd.args(["--limit", &n.to_string()]);
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("starting {}: {e}", dav1d.display()))?;
    let out = child.stdout.take().unwrap();
    let h = hashes_of(BufReader::with_capacity(1 << 22, out), f.frame_bytes(), limit);
    let o = child.wait_with_output()?;
    if !o.status.success() {
        bail!("{} on {}: {}", dav1d.display(), ivf.display(), String::from_utf8_lossy(&o.stderr).trim());
    }
    h
}

#[allow(clippy::too_many_arguments)]
fn check(streams: &[PathBuf], dav1d: &Path, ffmpeg: &str, modes: &[Mode], frames: Option<usize>,
         av1grain: Option<&Path>, threads: usize, hwaccel: Option<&str>, json: Option<&Path>, names: &[String], gpu: &Gpu)
         -> Res<()> {
    let mut r = gpu.open(false)?;
    eprintln!("GPU: {}", r.adapter_name());
    let tmp = std::env::temp_dir().join(format!("av1fg-check-{}", std::process::id()));
    std::fs::create_dir_all(&tmp)?;
    let mut jout = match json {
        Some(p) => Some(std::fs::OpenOptions::new().create(true).append(true).open(p)?),
        None => None,
    };
    let mut all_ok = true;
    println!("| stream | format | pictures (with grain) | grain syntax covered | grain | grain-free decode == dav1d | params == av1grain | GPU == dav1d |");
    println!("|---|---|---|---|---|---|---|---|");
    for (si, s) in streams.iter().enumerate() {
        let name = names.get(si).cloned().unwrap_or_else(|| s.file_name().unwrap().to_string_lossy().to_string());
        let pr = probe(&ffprobe_of(ffmpeg), s)?;
        let f = pr.format;
        // dav1d reads IVF / OBU / Annex B; anything else is remuxed to IVF
        let ext = s.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        let ivf = if ["ivf", "obu"].contains(&ext.as_str()) {
            s.clone()
        } else {
            let p = tmp.join(format!("s{si}.ivf"));
            let st = Command::new(ffmpeg)
                .args(["-hide_banner", "-nostdin", "-v", "error", "-y", "-i"])
                .arg(s)
                .args(["-map", "0:v:0", "-c", "copy"])
                .args(frames.map(|n| vec!["-frames:v".to_string(), n.to_string()]).unwrap_or_default())
                .args(["-f", "ivf"])
                .arg(&p)
                .status()?;
            if !st.success() {
                bail!("remuxing {} to IVF failed", s.display());
            }
            p
        };
        // grain-free pictures + grain from ffmpeg (the `decode` path), rendered on the GPU
        let mut results: Vec<(GrainMode, Vec<String>)> = vec![];
        let mut grain_log: Vec<Option<FilmGrainParams>> = vec![];
        let mut nograin_hashes = vec![];
        for (mi, m) in modes.iter().enumerate() {
            let mode: GrainMode = (*m).into();
            let mut src = GrainFree::start(ffmpeg, s, None, frames, hwaccel, threads, &pr.pix_fmt)?;
            let mut hs = vec![];
            let first = mi == 0;
            let n = pipeline(&mut r, &f, mode, |buf| {
                let g = src.next(buf)?;
                if first {
                    if let Some(g) = &g {
                        grain_log.push(g.filter(|x| x.has_grain()));
                        nograin_hashes.push(hex(&Sha256::digest(&buf[..])));
                    }
                }
                Ok(g)
            }, |pic, _| {
                hs.push(hex(&Sha256::digest(pic)));
                Ok(())
            })?;
            src.finish()?;
            let _ = n;
            results.push((mode, hs));
        }
        let ng = dav1d_hashes(dav1d, &ivf, None, &f, threads, frames)?;
        let nograin_ok = ng == nograin_hashes;
        let params_ok = match av1grain {
            None => "-".to_string(),
            Some(tool) => {
                let pj = tmp.join(format!("s{si}.json"));
                let st = Command::new(tool)
                    .arg("strip")
                    .arg(&ivf)
                    .arg(tmp.join(format!("s{si}.nograin.ivf")))
                    .arg(&pj)
                    .stderr(Stdio::null())
                    .status()?;
                if !st.success() {
                    "av1grain failed".to_string()
                } else {
                    let p = ParamsFile::parse(&std::fs::read_to_string(&pj)?)?;
                    let n = grain_log.len();
                    let same = (0..n).filter(|&i| p.picture(i).filter(|x| x.has_grain()) == grain_log[i]).count();
                    if same == n && p.grain.len() >= n { "yes".to_string() } else { format!("NO ({same}/{n})") }
                }
            }
        };
        let with = grain_log.iter().filter(|g| g.is_some()).count();
        let cov = coverage(ffmpeg, s, &grain_log, frames);
        for (mode, hs) in &results {
            let want = dav1d_hashes(dav1d, &ivf, Some(*mode), &f, threads, frames)?;
            let same = hs.iter().zip(want.iter()).filter(|(a, b)| a == b).count();
            let ok = same == want.len() && hs.len() == want.len() && !want.is_empty();
            all_ok &= ok && nograin_ok && !params_ok.starts_with("NO");
            let fmt = format!("{}x{} {}-bit {}", f.width, f.height, f.bitdepth, match f.layout {
                Layout::I400 => "4:0:0",
                Layout::I420 => "4:2:0",
                Layout::I422 => "4:2:2",
                Layout::I444 => "4:4:4",
            });
            println!("| {name} | {fmt} | {} ({with}) | {cov} | {} | {} | {params_ok} | {} |", want.len(), mode.name(),
                     if nograin_ok { "yes" } else { "NO" },
                     if ok { format!("{same}/{} identical", want.len()) } else { format!("**{same}/{} identical**", want.len()) });
            if let Some(j) = jout.as_mut() {
                writeln!(j, "{{\"stream\":{:?},\"width\":{},\"height\":{},\"bitdepth\":{},\"layout\":{},\"pictures\":{},\
                             \"with_grain\":{with},\"mode\":\"{}\",\"identical\":{same},\"nograin_ok\":{nograin_ok},\
                             \"params\":{:?},\"coverage\":{:?},\"gpu\":{:?},\"hwaccel\":{:?}}}",
                         name, f.width, f.height, f.bitdepth, f.layout as i32, want.len(), mode.name(), params_ok, cov,
                         r.adapter_name(), hwaccel.unwrap_or(""))?;
            }
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    if !all_ok {
        bail!("differences found");
    }
    Ok(())
}

/// which parts of the grain syntax a stream exercises: AR lags, chroma from luma, overlap, clipping, chroma
/// scaling points, distinct seeds, update_grain = 0 (parameters from a reference frame), show_existing_frame
fn coverage(ffmpeg: &str, s: &Path, grain: &[Option<FilmGrainParams>], frames: Option<usize>) -> String {
    let g: Vec<&FilmGrainParams> = grain.iter().flatten().collect();
    let mut lags: Vec<i32> = g.iter().map(|x| x.ar_coeff_lag).collect();
    lags.sort();
    lags.dedup();
    let mut seeds: Vec<u32> = g.iter().map(|x| x.seed).collect();
    seeds.sort();
    seeds.dedup();
    let any = |f: &dyn Fn(&FilmGrainParams) -> bool| g.iter().any(|x| f(x));
    let all = |f: &dyn Fn(&FilmGrainParams) -> bool| g.iter().all(|x| f(x));
    let mut parts = vec![format!("lag {}", lags.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("/"))];
    let flag = |on: bool, off: bool, name: &str| match (on, off) {
        (true, true) => format!("{name} on+off"),
        (true, false) => format!("{name} on"),
        _ => format!("{name} off"),
    };
    parts.push(flag(any(&|x| x.chroma_scaling_from_luma != 0), any(&|x| x.chroma_scaling_from_luma == 0), "cfl"));
    parts.push(flag(any(&|x| x.overlap_flag != 0), any(&|x| x.overlap_flag == 0), "overlap"));
    parts.push(flag(any(&|x| x.clip_to_restricted_range != 0), any(&|x| x.clip_to_restricted_range == 0), "clip"));
    if any(&|x| x.num_uv_points[0] > 0 || x.num_uv_points[1] > 0) {
        parts.push("uv points".into());
    }
    if !all(&|x| x.grain_scale_shift == 0) {
        parts.push("grain_scale_shift".into());
    }
    parts.push(format!("{} seeds", seeds.len()));
    // frame header syntax, from ffmpeg's trace_headers
    let mut cmd = Command::new(ffmpeg);
    cmd.args(["-hide_banner", "-nostdin", "-v", "info", "-i"]).arg(s).args(["-map", "0:v:0", "-c", "copy", "-bsf:v", "trace_headers"]);
    if let Some(n) = frames {
        cmd.args(["-frames:v", &n.to_string()]);
    }
    if let Ok(o) = cmd.args(["-f", "null", "-"]).output() {
        let t = String::from_utf8_lossy(&o.stderr);
        let count = |k: &str, v: &str| t.lines().filter(|l| l.contains(k) && l.trim_end().ends_with(v)).count();
        let inh = count(" update_grain ", "= 0");
        let se = count(" show_existing_frame ", "= 1");
        if inh > 0 {
            parts.push(format!("update_grain=0 x{inh}"));
        }
        if se > 0 {
            parts.push(format!("show_existing x{se}"));
        }
    }
    parts.join(", ")
}

// ---------------------------------------------------------------------------------------------------------------
// bench

/// a typical SVT-AV1 film grain set (film-grain=8-ish, 10-bit, lag 3, no chroma from luma)
fn default_bench_params() -> FilmGrainParams {
    let line = "Film grain parameters: type av1; seed=7391; y_points={ (0,20) (23,20) (46,23) (69,26) (93,30) (116,32) \
(139,33) (162,33) (185,32) (209,30) (232,26) (255,26) }; chroma_scaling_from_luma=0; uv_points[0]={ (0,15) (128,17) (255,16) }; \
uv_points[1]={ (0,14) (128,17) (255,15) }; scaling_shift=11; ar_coeff_lag=3; ar_coeffs_y={ 4 -7 2 4 -2 3 0 -7 \
11 -10 12 -1 7 -1 3 -7 19 -12 37 -15 5 -9 23 99 }; ar_coeffs_uv[0]={ 1 -1 0 1 -2 1 0 0 0 0 3 -1 0 0 0 -2 4 -1 5 -1 1 \
-2 3 29 0 }; ar_coeffs_uv[1]={ 0 -1 0 1 -1 1 0 0 1 -1 2 -1 1 0 1 -2 4 -1 5 -2 1 -2 4 30 0 }; ar_coeff_shift=7; \
grain_scale_shift=0; uv_mult[0] = 0; uv_mult_luma[0] = 64; uv_offset[0] = 0; uv_mult[1] = 0; uv_mult_luma[1] = 64; \
uv_offset[1] = 0; overlap_flag=1; limit_output_range=0;";
    parse_showinfo_line(line).unwrap().unwrap()
}

fn bench(sizes: &[String], bitdepth: u32, frames: usize, mode: GrainMode, params: Option<&Path>, gpu: &Gpu) -> Res<()> {
    let g = match params {
        Some(p) => {
            let (gl, _) = load_grain(p)?;
            gl.into_iter().flatten().next().ok_or("no picture with grain in the parameter file")?
        }
        None => default_bench_params(),
    };
    let mut r = gpu.open(true)?;
    println!("GPU: {}{}", r.adapter_name(), if r.timing() { "" } else { " (no timestamp queries)" });
    println!("| size | mode | GPU ms/picture (AR luma + AR chroma + apply) | pictures/s with upload + readback ({} in flight) | 1 at a time |",
             gpu.frames_in_flight);
    println!("|---|---|---|---|---|");
    for s in sizes {
        let (w, h) = s.split_once('x').ok_or("size: WxH")?;
        let f = Format { width: w.parse()?, height: h.parse()?, bitdepth, layout: Layout::I420, matrix: 2 };
        // a smooth picture with some texture, so the scaling LUTs see many values
        let mut pic = vec![0u8; f.frame_bytes()];
        let bps = f.bytes_per_sample();
        let max = (1u32 << bitdepth) - 1;
        let n = pic.len() / bps;
        for i in 0..n {
            let v = ((i as u64 * 2654435761) % (max as u64 + 1)) as u32 / 4 + (i % f.width) as u32 * max / (2 * f.width as u32);
            let v = v.min(max);
            if bps == 2 {
                pic[2 * i..2 * i + 2].copy_from_slice(&(v as u16).to_le_bytes());
            } else {
                pic[i] = v as u8;
            }
        }
        let mut gpu_ms = vec![];
        let mut parts = PassTimes::default();
        // warm-up + timed pipelined run
        let mut k = 0usize;
        let warm = 10;
        let mut t0 = Instant::now();
        let mut counted = 0usize;
        let total = frames + warm;
        pipeline(&mut r, &f, mode, |buf| {
            if k >= total {
                return Ok(None);
            }
            if buf.len() == pic.len() {
                buf.copy_from_slice(&pic);
            }
            k += 1;
            Ok(Some(Some(FilmGrainParams { seed: (k as u32 * 3381) & 0xffff, ..g })))
        }, |_, t| {
            counted += 1;
            if counted == warm {
                t0 = Instant::now();
            }
            if counted > warm {
                if let Some(t) = t {
                    gpu_ms.push(t.total);
                    parts.ar_luma += t.ar_luma;
                    parts.ar_chroma += t.ar_chroma + t.templates;
                    parts.apply += t.apply;
                }
            }
            Ok(())
        })?;
        let fps = frames as f64 / t0.elapsed().as_secs_f64();
        // one at a time (latency)
        let mut out = vec![0u8; f.frame_bytes()];
        let t1 = Instant::now();
        let m = 30.min(frames);
        for i in 0..m {
            let pl = planes_of(&f, &pic);
            r.render(&f, &pl, Some(&FilmGrainParams { seed: i as u32, ..g }), mode, &mut out)?;
        }
        let fps1 = m as f64 / t1.elapsed().as_secs_f64();
        gpu_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = gpu_ms.get(gpu_ms.len() / 2).copied().unwrap_or(0.0);
        let c = gpu_ms.len().max(1) as f64;
        println!("| {s} {bitdepth}-bit | {} | {med:.2} ({:.2} + {:.2} + {:.2}) | {fps:.0} | {fps1:.0} |", mode.name(),
                 parts.ar_luma / c, parts.ar_chroma / c, parts.apply / c);
    }
    Ok(())
}
