/*
 * libdav1d decodes an IVF file without grain; av1-fullgrain-gpu draws the grain on the GPU.
 * Writes the pictures as raw planes (like `dav1d --muxer yuv`), so the output can be compared with
 * av1-fullgrain's dav1d:
 *
 *   cc -O2 examples/dav1d_gpu_grain.c -Iinclude -o dav1d_gpu_grain \
 *       $(pkg-config --cflags --libs dav1d) -L$TARGET/release -lav1_fullgrain_gpu
 *   DAV1D_GRAIN_FORCE=0 ./dav1d_gpu_grain in.ivf out.yuv [full|standard]
 *   dav1d -i in.ivf --muxer yuv -o ref.yuv && cmp out.yuv ref.yuv
 *
 * Copyright © 2026, divergentnn (av1-fullgrain). SPDX-License-Identifier: BSD-2-Clause
 */
#include <dav1d/dav1d.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "av1_fullgrain_gpu.h"

_Static_assert(sizeof(AV1FGFilmGrainData) == sizeof(Dav1dFilmGrainData), "Dav1dFilmGrainData layout");

static int read_ivf_frame(FILE *f, uint8_t **buf, size_t *len) {
    uint8_t h[12];
    if (fread(h, 1, 12, f) != 12) return 0;
    *len = h[0] | h[1] << 8 | h[2] << 16 | (size_t) h[3] << 24;
    *buf = malloc(*len);
    return *buf && fread(*buf, 1, *len, f) == *len;
}

static void free_buf(const uint8_t *data, void *cookie) { (void) cookie; free((void *) data); }

static int output(AV1FGRenderer *r, Dav1dPicture *p, int mode, FILE *out) {
    const int bps = p->p.bpc > 8 ? 2 : 1;
    const int ssx = p->p.layout == DAV1D_PIXEL_LAYOUT_I420 || p->p.layout == DAV1D_PIXEL_LAYOUT_I422;
    const int ssy = p->p.layout == DAV1D_PIXEL_LAYOUT_I420;
    const int cw = (p->p.w + ssx) >> ssx, ch = (p->p.h + ssy) >> ssy;
    const int mono = p->p.layout == DAV1D_PIXEL_LAYOUT_I400;
    AV1FGPicture in = { p->p.w, p->p.h, p->p.bpc, p->p.layout, p->seq_hdr->mtrx,
                        { p->data[0], p->data[1], p->data[2] }, { p->stride[0], p->stride[1], p->stride[1] } };
    const size_t ysz = (size_t) p->p.w * p->p.h * bps, csz = mono ? 0 : (size_t) cw * ch * bps;
    uint8_t *buf = malloc(ysz + 2 * csz);
    AV1FGPicture o = in;
    o.data[0] = buf; o.data[1] = buf + ysz; o.data[2] = buf + ysz + csz;
    o.stride[0] = (ptrdiff_t) p->p.w * bps; o.stride[1] = o.stride[2] = (ptrdiff_t) cw * bps;
    const Dav1dFilmGrainData *g = p->frame_hdr->film_grain.present ? &p->frame_hdr->film_grain.data : NULL;
    if (av1fg_render(r, &in, (const AV1FGFilmGrainData *) g, mode, &o)) {
        fprintf(stderr, "av1fg_render: %s\n", av1fg_last_error(r));
        free(buf);
        return -1;
    }
    fwrite(buf, 1, ysz + 2 * csz, out);
    free(buf);
    return 0;
}

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(stderr, "usage: %s IN.ivf OUT.yuv [full|standard]\n", argv[0]);
        return 2;
    }
    const int mode = argc > 3 && !strcmp(argv[3], "standard") ? AV1FG_MODE_STANDARD : AV1FG_MODE_FULL;
    FILE *in = fopen(argv[1], "rb"), *out = fopen(argv[2], "wb");
    uint8_t hdr[32];
    if (!in || !out || fread(hdr, 1, 32, in) != 32 || memcmp(hdr, "DKIF", 4)) {
        fprintf(stderr, "cannot open %s / %s\n", argv[1], argv[2]);
        return 1;
    }
    char err[256];
    AV1FGRenderer *r = av1fg_open(NULL, 0, err, sizeof(err));
    if (!r) {
        fprintf(stderr, "av1fg_open: %s\n", err);
        return 1;
    }
    fprintf(stderr, "GPU: %s\n", av1fg_adapter_name(r));
    Dav1dSettings s;
    dav1d_default_settings(&s);
    s.apply_grain = 0;
    s.n_threads = 4;
    Dav1dContext *c;
    if (dav1d_open(&c, &s)) return 1;
    uint8_t *buf;
    size_t len;
    int n = 0, res = 0;
    Dav1dData data = { 0 };
    while (!res) {
        if (!data.sz) {
            if (!read_ivf_frame(in, &buf, &len)) break;
            dav1d_data_wrap(&data, buf, len, free_buf, NULL);
        }
        int e = dav1d_send_data(c, &data);
        if (e && e != DAV1D_ERR(EAGAIN)) { res = 1; break; }
        Dav1dPicture p = { 0 };
        e = dav1d_get_picture(c, &p);
        if (!e) { res = output(r, &p, mode, out); dav1d_picture_unref(&p); n++; }
        else if (e != DAV1D_ERR(EAGAIN)) res = 1;
    }
    while (!res) {   /* drain: EAGAIN = nothing left */
        Dav1dPicture p = { 0 };
        if (dav1d_get_picture(c, &p)) break;
        if (output(r, &p, mode, out)) { res = 1; dav1d_picture_unref(&p); break; }
        dav1d_picture_unref(&p);
        n++;
    }
    fprintf(stderr, "%d pictures\n", n);
    dav1d_close(&c);
    av1fg_close(r);
    fclose(out);
    fclose(in);
    return res;
}
