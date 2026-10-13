//! A small YUV4MPEG2 (y4m) reader and writer for the command line tool.
//!
//! Copyright © 2026, divergentnn (av1-fullgrain). SPDX-License-Identifier: BSD-2-Clause

use crate::params::{Format, Layout};
use std::io::{self, BufRead, Write};

#[derive(Debug, Clone)]
pub struct Header {
    pub format: Format,
    /// the header line without "YUV4MPEG2 " and the newline, as read (written back unchanged)
    pub tags: String,
}

fn bad(m: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, m.into())
}

impl Header {
    /// a header for a format (W H, frame rate num/den, C tag)
    pub fn new(format: Format, fps: (u32, u32)) -> Header {
        let c = match (format.layout, format.bitdepth) {
            (Layout::I400, 8) => "mono".to_string(),
            (Layout::I400, d) => format!("mono{d}"),
            (l, 8) => match l {
                Layout::I420 => "420jpeg".into(),
                Layout::I422 => "422".into(),
                _ => "444".into(),
            },
            (l, d) => format!("{}p{d}", match l {
                Layout::I420 => "420",
                Layout::I422 => "422",
                _ => "444",
            }),
        };
        Header { format, tags: format!("W{} H{} F{}:{} Ip A1:1 C{c}", format.width, format.height, fps.0, fps.1) }
    }

    pub fn parse(line: &str) -> io::Result<Header> {
        let tags = line.strip_prefix("YUV4MPEG2").ok_or_else(|| bad("not a y4m stream"))?.trim().to_string();
        let (mut w, mut h, mut c) = (0usize, 0usize, "420jpeg".to_string());
        for t in tags.split_whitespace() {
            let (k, v) = t.split_at(1);
            match k {
                "W" => w = v.parse().map_err(|_| bad("y4m: bad W"))?,
                "H" => h = v.parse().map_err(|_| bad("y4m: bad H"))?,
                "C" => c = v.to_string(),
                _ => {}
            }
        }
        let (layout, bitdepth) = match c.as_str() {
            "420" | "420jpeg" | "420paldv" | "420mpeg2" => (Layout::I420, 8),
            "422" => (Layout::I422, 8),
            "444" => (Layout::I444, 8),
            "mono" => (Layout::I400, 8),
            "420p10" => (Layout::I420, 10),
            "422p10" => (Layout::I422, 10),
            "444p10" => (Layout::I444, 10),
            "mono10" => (Layout::I400, 10),
            "420p12" => (Layout::I420, 12),
            "422p12" => (Layout::I422, 12),
            "444p12" => (Layout::I444, 12),
            "mono12" => (Layout::I400, 12),
            _ => return Err(bad(format!("y4m: colour space C{c} not supported"))),
        };
        if w == 0 || h == 0 {
            return Err(bad("y4m: no frame size"));
        }
        Ok(Header { format: Format { width: w, height: h, bitdepth, layout, matrix: 2 }, tags })
    }

    pub fn write(&self, out: &mut impl Write) -> io::Result<()> {
        writeln!(out, "YUV4MPEG2 {}", self.tags)
    }
}

pub struct Reader<R: BufRead> {
    pub header: Header,
    inner: R,
}

impl<R: BufRead> Reader<R> {
    pub fn new(mut inner: R) -> io::Result<Reader<R>> {
        let mut line = String::new();
        inner.read_line(&mut line)?;
        let header = Header::parse(line.trim_end_matches('\n'))?;
        Ok(Reader { header, inner })
    }

    /// the next frame into buf (Format::frame_bytes() long); false at the end
    pub fn read_frame(&mut self, buf: &mut [u8]) -> io::Result<bool> {
        let mut line = Vec::new();
        let n = self.inner.read_until(b'\n', &mut line)?;
        if n == 0 {
            return Ok(false);
        }
        if !line.starts_with(b"FRAME") {
            return Err(bad("y4m: expected FRAME"));
        }
        self.inner.read_exact(buf)?;
        Ok(true)
    }
}

pub fn write_frame(out: &mut impl Write, data: &[u8]) -> io::Result<()> {
    out.write_all(b"FRAME\n")?;
    out.write_all(data)
}
