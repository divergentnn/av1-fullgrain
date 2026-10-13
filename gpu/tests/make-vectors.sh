#!/bin/sh
# Synthetic AV1 test streams with film grain, for `av1-fullgrain-gpu check`:
#
#   gpu/tests/make-vectors.sh OUTDIR [FRAMES]
#
# Sources are ffmpeg's testsrc2 pattern; the grain comes from aomenc's 16 film grain test vectors (8 and 10 bit),
# aomenc grain tables (update_grain = 0, i.e. parameters inherited from a reference frame, and per-frame seeds),
# HDR PQ / BT.2020 tags, an identity matrix with restricted-range clipping, monochrome, odd sizes, rav1e's photon
# noise and SVT-AV1's film grain model. Needs ffmpeg, aomenc; rav1e and SvtAv1EncApp are used when present.
#
# Copyright © 2026, divergentnn (av1-fullgrain). SPDX-License-Identifier: BSD-2-Clause
set -eu
OUT=$1
N=${2:-10}
mkdir -p "$OUT"
cd "$OUT"
J=${JOBS:-4}

src() { # name size pix_fmt
    [ -f "$1.y4m" ] || ffmpeg -hide_banner -v error -nostdin -y -f lavfi -i "testsrc2=size=$2:rate=24,format=$3" \
        -frames:v "$N" -strict -1 "$1.y4m"
}
src s420_8 640x360 yuv420p
src s420_10 640x360 yuv420p10le
src s420_10_odd 643x361 yuv420p10le
src s444_8 320x240 yuv444p

aom() { # out src bitdepth extra...
    o=$1; s=$2; b=$3; shift 3
    [ -f "$o.ivf" ] || aomenc --ivf -o "$o.ivf" --limit="$N" --cpu-used=6 --end-usage=q --cq-level=40 \
        --threads="$J" -b "$b" --input-bit-depth="$b" --kf-max-dist=30 --lag-in-frames=12 "$@" "$s.y4m" 2>/dev/null
}

# the 16 test vectors, 8 and 10 bit
i=1
while [ $i -le 16 ]; do
    v=$(printf %02d $i)
    aom "tv${v}_420_8" s420_8 8 --film-grain-test=$i
    aom "tv${v}_420_10" s420_10 10 --film-grain-test=$i
    i=$((i + 1))
done
aom tv10_420_10_odd s420_10_odd 10 --film-grain-test=10

# a grain table: update_grain = 0 in the middle (parameters from a reference frame), new seeds per frame,
# then a second parameter set with chroma from luma and no overlap (times in 1/10,000,000 s at 24 fps)
cat > table.txt <<'TBL'
filmgrn1
E 0 1666667 1 1234 1
	p 3 7 0 11 0 1 128 192 256 128 192 256
	sY 6  0 20 40 30 80 34 128 34 200 28 255 24
	sCb 2  0 18 255 16
	sCr 2  0 17 255 15
	cY 4 -7 2 4 -2 3 0 -7 11 -10 12 -1 7 -1 3 -7 19 -12 37 -15 5 -9 23 99
	cCb 1 -1 0 1 -2 1 0 0 0 0 3 -1 0 0 0 -2 4 -1 5 -1 1 -2 3 29 0
	cCr 0 -1 0 1 -1 1 0 0 1 -1 2 -1 1 0 1 -2 4 -1 5 -2 1 -2 4 30 0
E 1666667 3333334 1 4321 0
E 3333334 9999999999 1 777 1
	p 2 8 1 10 1 0 128 192 256 128 192 256
	sY 4  0 30 64 40 160 36 255 20
	sCb 0
	sCr 0
	cY 2 -3 5 -3 2 -6 10 -8 22 -13 30 40
	cCb 1 -1 2 -1 1 -2 3 -2 4 -2 5 9 -20
	cCr 1 -1 2 -1 1 -2 3 -2 4 -2 5 9 -18
TBL
aom table_420_10 s420_10 10 --film-grain-table=table.txt
aom table_420_8 s420_8 8 --film-grain-table=table.txt

# HDR: PQ / BT.2020 tags, 10 bit
aom hdr_pq_420_10 s420_10 10 --film-grain-test=1 --color-primaries=bt2020 --transfer-characteristics=smpte2084 \
    --matrix-coefficients=bt2020ncl
aom hdr_pq_table_420_10 s420_10 10 --film-grain-table=table.txt --color-primaries=bt2020 \
    --transfer-characteristics=smpte2084 --matrix-coefficients=bt2020ncl

# identity matrix (RGB-like 4:4:4) with restricted-range clipping (test vector 3 clips), and monochrome
aom ident_444_8 s444_8 8 --profile=1 --i444 --film-grain-test=3 --matrix-coefficients=identity
aom mono_10 s420_10 10 --monochrome --film-grain-test=3

# other encoders' grain
if command -v rav1e >/dev/null 2>&1 && [ ! -f rav1e_photon_420_10.ivf ]; then
    rav1e -q -s 10 --threads "$J" --photon-noise 16 -o rav1e_photon_420_10.ivf s420_10.y4m 2>/dev/null
fi
if command -v SvtAv1EncApp >/dev/null 2>&1 && [ ! -f svt_fg_420_10.ivf ]; then
    SvtAv1EncApp --preset 10 --crf 40 --film-grain 10 --lp "$J" -i s420_10.y4m -b svt_fg_420_10.ivf >/dev/null 2>&1
fi
ls *.ivf | wc -l
