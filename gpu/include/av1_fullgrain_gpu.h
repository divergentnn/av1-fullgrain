/*
 * av1-fullgrain-gpu: AV1 film grain drawn on the GPU onto decoded grain-free pictures, full-frame
 * (av1-fullgrain's grain) or standard (the AV1 specification's tiled grain), bit-identical to
 * av1-fullgrain's dav1d. Vulkan, Metal or DX12 underneath (wgpu).
 *
 * Link with the library built by `cargo build --release` in gpu/ (libav1_fullgrain_gpu.so / .dylib /
 * .dll, or the static libav1_fullgrain_gpu.a / .lib plus the system libraries cargo lists with
 * --print native-static-libs).
 *
 * Typical use with libdav1d: decode with Dav1dSettings.apply_grain = 0, then
 *     av1fg_render(r, &in, &pic.frame_hdr->film_grain.data, AV1FG_MODE_FULL, &out)
 * for pictures with pic.frame_hdr->film_grain.present (see examples/dav1d_gpu_grain.c).
 * An av1-fullgrain libdav1d installed as the system's one draws its own grain even with
 * apply_grain = 0 unless DAV1D_GRAIN_FORCE=0 is set in the environment.
 *
 * Copyright © 2026, divergentnn (av1-fullgrain). SPDX-License-Identifier: BSD-2-Clause
 */
#ifndef AV1_FULLGRAIN_GPU_H
#define AV1_FULLGRAIN_GPU_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct AV1FGRenderer AV1FGRenderer;

/* The film grain of one picture: the same layout as dav1d's Dav1dFilmGrainData (dav1d/headers.h),
 * signed values as dav1d stores them (uv_mult = cb_mult - 128, uv_offset = cb_offset - 256, ...). */
typedef struct AV1FGFilmGrainData {
    unsigned seed;
    int num_y_points;
    uint8_t y_points[14][2];          /* value, scaling */
    int chroma_scaling_from_luma;
    int num_uv_points[2];
    uint8_t uv_points[2][10][2];
    int scaling_shift;
    int ar_coeff_lag;
    int8_t ar_coeffs_y[24];
    int8_t ar_coeffs_uv[2][25 + 3];
    uint64_t ar_coeff_shift;
    int grain_scale_shift;
    int uv_mult[2];
    int uv_luma_mult[2];
    int uv_offset[2];
    int overlap_flag;
    int clip_to_restricted_range;
} AV1FGFilmGrainData;

enum AV1FGMode {
    AV1FG_MODE_FULL = 0,        /* full-frame grain */
    AV1FG_MODE_STANDARD = 1,    /* standard AV1 grain (32x32 tiles of one 82x73 patch), as stock dav1d */
};

enum AV1FGLayout { AV1FG_I400 = 0, AV1FG_I420 = 1, AV1FG_I422 = 2, AV1FG_I444 = 3 };  /* = DAV1D_PIXEL_LAYOUT_* */

typedef struct AV1FGPicture {
    int width, height;
    int bitdepth;               /* 8, 10 or 12; above 8, samples are 16-bit little endian */
    int layout;                 /* enum AV1FGLayout */
    int matrix;                 /* matrix_coefficients; only 0 (identity) matters */
    uint8_t *data[3];           /* Y, U, V */
    ptrdiff_t stride[3];        /* bytes per row */
} AV1FGPicture;

/* Open a GPU: adapter = part of its name, or NULL for the high-performance one; frames_in_flight =
 * how many pictures av1fg_submit may queue (0: 3). NULL on failure, with the reason in err. */
AV1FGRenderer *av1fg_open(const char *adapter, int frames_in_flight, char *err, size_t errlen);
void av1fg_close(AV1FGRenderer *r);

const char *av1fg_adapter_name(const AV1FGRenderer *r);
const char *av1fg_last_error(const AV1FGRenderer *r);
const char *av1fg_version(void);

/* One picture, synchronously. grain NULL: no grain (the picture is copied). 0 or -1. */
int av1fg_render(AV1FGRenderer *r, const AV1FGPicture *in, const AV1FGFilmGrainData *grain, int mode,
                 const AV1FGPicture *out);

/* Pipelined: queue up to frames_in_flight pictures, receive them in order.
 * av1fg_submit: 0 or -1. av1fg_receive: 1 (a picture), 0 (none queued) or -1. The picture format
 * may change only when nothing is queued. */
int av1fg_submit(AV1FGRenderer *r, const AV1FGPicture *in, const AV1FGFilmGrainData *grain, int mode);
int av1fg_receive(AV1FGRenderer *r, const AV1FGPicture *out);
int av1fg_can_submit(const AV1FGRenderer *r);

#ifdef __cplusplus
}
#endif
#endif
