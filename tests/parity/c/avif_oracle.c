/* libavif oracle of isobmff-rs (CI only, docs/parity.md): writes "avif.txt" for the primary item of
 * every AVIF file (libavif v1.4.2, BSD-2-Clause, built without codecs).
 *
 *   avif-oracle <in-dir> <out-dir>
 *   avif-oracle --bench <iters> <in-dir>      "<name>\t<ns per open+parse>" per file
 *
 * Only files whose first box is an ftyp with `avif` as major or compatible brand are read; others
 * are skipped. Name = relative path with / replaced by __. A file above 512 MiB is skipped.
 */
#define _POSIX_C_SOURCE 200809L
#include <avif/avif.h>
#include <dirent.h>
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>

#define MAX_FILE ((uint64_t)512 << 20)

static volatile uint64_t sink;

static int write_file(const char *path, const void *data, size_t n)
{
    FILE *f = fopen(path, "wb");
    int ok;
    if (!f) {
        return -1;
    }
    ok = (n == 0 || fwrite(data, 1, n, f) == n);
    if (fclose(f) != 0) {
        ok = 0;
    }
    return ok ? 0 : -1;
}

static uint8_t *read_file(const char *path, size_t *n)
{
    FILE *f = fopen(path, "rb");
    uint8_t *buf;
    long sz;
    if (!f) {
        return NULL;
    }
    if (fseek(f, 0, SEEK_END) != 0 || (sz = ftell(f)) < 0 || fseek(f, 0, SEEK_SET) != 0 ||
        (uint64_t)sz > MAX_FILE) {
        fclose(f);
        return NULL;
    }
    buf = (uint8_t *)malloc((size_t)sz + 1);
    if (!buf || fread(buf, 1, (size_t)sz, f) != (size_t)sz) {
        free(buf);
        fclose(f);
        return NULL;
    }
    fclose(f);
    *n = (size_t)sz;
    return buf;
}

static uint32_t be32(const uint8_t *p)
{
    return ((uint32_t)p[0] << 24) | ((uint32_t)p[1] << 16) | ((uint32_t)p[2] << 8) | p[3];
}

/* 1 when the first box is an ftyp whose major or a compatible brand is `avif`. */
static int has_avif_brand(const uint8_t *d, size_t n)
{
    uint64_t size, off = 8, end, p;
    if (n < 16 || memcmp(d + 4, "ftyp", 4) != 0) {
        return 0;
    }
    size = be32(d);
    if (size == 1) {
        if (n < 24) {
            return 0;
        }
        size = ((uint64_t)be32(d + 8) << 32) | be32(d + 12);
        off = 16;
    } else if (size == 0) {
        size = n;
    }
    end = size < n ? size : n;
    if (end < off + 8) {
        return 0;
    }
    if (!memcmp(d + off, "avif", 4)) {
        return 1;
    }
    for (p = off + 8; p + 4 <= end; p += 4) { /* skip the minor version at off + 4 */
        if (!memcmp(d + p, "avif", 4)) {
            return 1;
        }
    }
    return 0;
}

/* ---- directory walk ---- */

typedef int (*file_fn)(const char *path, const char *name, void *arg);

static int walk(const char *root, const char *rel, file_fn fn, void *arg)
{
    char dir[4096];
    DIR *d;
    struct dirent *e;
    int bad = 0;
    if (snprintf(dir, sizeof dir, "%s%s%s", root, *rel ? "/" : "", rel) >= (int)sizeof dir) {
        return 1;
    }
    d = opendir(dir);
    if (!d) {
        fprintf(stderr, "avif-oracle: %s: %s\n", dir, strerror(errno));
        return 1;
    }
    while ((e = readdir(d)) != NULL) {
        char path[4096], sub[4096], name[4096];
        struct stat st;
        size_t i, o = 0;
        if (!strcmp(e->d_name, ".") || !strcmp(e->d_name, "..")) {
            continue;
        }
        if (snprintf(path, sizeof path, "%s/%s", dir, e->d_name) >= (int)sizeof path ||
            snprintf(sub, sizeof sub, "%s%s%s", rel, *rel ? "/" : "", e->d_name) >= (int)sizeof sub) {
            bad = 1;
            continue;
        }
        if (lstat(path, &st) != 0) {
            continue;
        }
        if (S_ISDIR(st.st_mode)) {
            bad |= walk(root, sub, fn, arg);
            continue;
        }
        if (S_ISLNK(st.st_mode) && stat(path, &st) != 0) {
            continue;
        }
        if (!S_ISREG(st.st_mode)) {
            continue;
        }
        if ((uint64_t)st.st_size > MAX_FILE) {
            fprintf(stderr, "avif-oracle: %s: larger than 512 MiB, skipped\n", path);
            continue;
        }
        for (i = 0; sub[i] && o + 2 < sizeof name; i++) {
            if (sub[i] == '/') {
                name[o++] = '_';
                name[o++] = '_';
            } else {
                name[o++] = sub[i];
            }
        }
        name[o] = 0;
        bad |= fn(path, name, arg);
    }
    closedir(d);
    return bad;
}

/* ---- the decoder ---- */

/* The settings of docs/parity.md. imageSizeLimit stays at its default. */
static avifDecoder *parse(const uint8_t *data, size_t len, int ignore_exif, avifResult *res)
{
    avifDecoder *d = avifDecoderCreate();
    if (!d) {
        *res = AVIF_RESULT_OUT_OF_MEMORY;
        return NULL;
    }
    d->requestedSource = AVIF_DECODER_SOURCE_PRIMARY_ITEM;
    d->strictFlags = AVIF_STRICT_DISABLED;
    d->imageContentToDecode = AVIF_IMAGE_CONTENT_ALL;
    d->imageDimensionLimit = 0;
    d->imageCountLimit = 0;
    d->ignoreExif = ignore_exif ? AVIF_TRUE : AVIF_FALSE;
    *res = avifDecoderSetIOMemory(d, data, len);
    if (*res == AVIF_RESULT_OK) {
        *res = avifDecoderParse(d);
    }
    return d;
}

static void frac_s(FILE *f, avifSignedFraction v)
{
    fprintf(f, "%d/%u", v.n, v.d);
}

static void frac_u(FILE *f, avifUnsignedFraction v)
{
    fprintf(f, "%u/%u", v.n, v.d);
}

static void frac3_s(FILE *f, const char *label, const avifSignedFraction v[3])
{
    int c;
    fprintf(f, " %s=", label);
    for (c = 0; c < 3; c++) {
        if (c) {
            fputc(',', f);
        }
        frac_s(f, v[c]);
    }
}

static void frac3_u(FILE *f, const char *label, const avifUnsignedFraction v[3])
{
    int c;
    fprintf(f, " %s=", label);
    for (c = 0; c < 3; c++) {
        if (c) {
            fputc(',', f);
        }
        frac_u(f, v[c]);
    }
}

static int yuv_name(avifPixelFormat f)
{
    switch (f) {
    case AVIF_PIXEL_FORMAT_YUV444:
        return 444;
    case AVIF_PIXEL_FORMAT_YUV422:
        return 422;
    case AVIF_PIXEL_FORMAT_YUV420:
        return 420;
    case AVIF_PIXEL_FORMAT_YUV400:
        return 400;
    default:
        return 0;
    }
}

static int dump_one(const char *path, const char *name, void *argp)
{
    const char *out = (const char *)argp;
    char dir[4096], p[4096];
    size_t len = 0;
    uint8_t *data = read_file(path, &len);
    avifDecoder *d;
    avifResult res;
    int exif_error = 0, bad = 0;
    FILE *f;

    if (!data) {
        return 0;
    }
    if (!has_avif_brand(data, len)) {
        free(data);
        return 0;
    }
    if (snprintf(dir, sizeof dir, "%s/%s", out, name) >= (int)sizeof dir || (mkdir(dir, 0777) != 0 && errno != EEXIST)) {
        fprintf(stderr, "avif-oracle: cannot create output directory for %s\n", name);
        free(data);
        return 1;
    }
    snprintf(p, sizeof p, "%s/avif.txt", dir);
    f = fopen(p, "wb");
    if (!f) {
        fprintf(stderr, "avif-oracle: cannot write %s\n", p);
        free(data);
        return 1;
    }

    d = parse(data, len, 0, &res);
    if (res == AVIF_RESULT_INVALID_EXIF_PAYLOAD) {
        if (d) {
            avifDecoderDestroy(d);
        }
        exif_error = 1;
        d = parse(data, len, 1, &res);
    }
    if (!d || res != AVIF_RESULT_OK || !d->image) {
        fputs("error\n", f);
        fprintf(stderr, "%s\t%s: %s\n", name, avifResultToString(res), d ? d->diag.error : "");
    } else {
        const avifImage *im = d->image;
        avifExtent ext;
        fputs("ok\n", f);
        fprintf(f, "size %ux%u depth=%u yuv=%d\n", im->width, im->height, im->depth, yuv_name(im->yuvFormat));
        fprintf(f, "alpha %d prem %d\n", d->alphaPresent ? 1 : 0, im->alphaPremultiplied ? 1 : 0);
        fprintf(f, "icc %zu\n", im->icc.size);
        if (im->icc.size > 0 && im->icc.data) {
            snprintf(p, sizeof p, "%s/avif-icc.bin", dir);
            bad |= write_file(p, im->icc.data, im->icc.size) != 0;
        }
        if (exif_error) {
            fputs("exif error\n", f);
        } else {
            fprintf(f, "exif %zu\n", im->exif.size);
            if (im->exif.size > 0 && im->exif.data) {
                snprintf(p, sizeof p, "%s/avif-exif.bin", dir);
                bad |= write_file(p, im->exif.data, im->exif.size) != 0;
            }
        }
        fprintf(f, "xmp %zu\n", im->xmp.size);
        if (im->xmp.size > 0 && im->xmp.data) {
            snprintf(p, sizeof p, "%s/avif-xmp.bin", dir);
            bad |= write_file(p, im->xmp.data, im->xmp.size) != 0;
        }
        if (im->transformFlags & AVIF_TRANSFORM_IROT) {
            fprintf(f, "irot %d\n", (int)im->irot.angle * 90);
        } else {
            fputs("irot -\n", f);
        }
        if (im->transformFlags & AVIF_TRANSFORM_IMIR) {
            fprintf(f, "imir %d\n", (int)im->imir.axis);
        } else {
            fputs("imir -\n", f);
        }
        if (im->transformFlags & AVIF_TRANSFORM_CLAP) {
            fprintf(f, "clap %u/%u,%u/%u,%d/%u,%d/%u\n", im->clap.widthN, im->clap.widthD, im->clap.heightN,
                    im->clap.heightD, (int32_t)im->clap.horizOffN, im->clap.horizOffD, (int32_t)im->clap.vertOffN,
                    im->clap.vertOffD);
        } else {
            fputs("clap -\n", f);
        }
        if (im->transformFlags & AVIF_TRANSFORM_PASP) {
            fprintf(f, "pasp %u/%u\n", im->pasp.hSpacing, im->pasp.vSpacing);
        } else {
            fputs("pasp -\n", f);
        }
        memset(&ext, 0, sizeof ext);
        if (avifDecoderNthImageMaxExtent(d, 0, &ext) == AVIF_RESULT_OK) {
            fprintf(f, "extent %llu %zu\n", (unsigned long long)ext.offset, ext.size);
        } else {
            fputs("extent error\n", f);
        }
        if (im->gainMap) {
            const avifGainMap *g = im->gainMap;
            fputs("gainmap 1\n", f);
            fputs("gainmap-meta base=", f);
            frac_u(f, g->baseHdrHeadroom);
            fputs(" alt=", f);
            frac_u(f, g->alternateHdrHeadroom);
            frac3_s(f, "min", g->gainMapMin);
            frac3_s(f, "max", g->gainMapMax);
            frac3_u(f, "gamma", g->gainMapGamma);
            frac3_s(f, "baseoff", g->baseOffset);
            frac3_s(f, "altoff", g->alternateOffset);
            fprintf(f, " base_cs=%d\n", g->useBaseColorSpace ? 1 : 0);
        } else {
            fputs("gainmap 0\n", f);
        }
    }
    if (fclose(f) != 0) {
        bad = 1;
    }
    if (d) {
        avifDecoderDestroy(d);
    }
    free(data);
    return bad;
}

/* ---- bench ---- */

static uint64_t now_ns(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000000u + (uint64_t)ts.tv_nsec;
}

/* open + parse, the work of the isobmff-rs parse; 0 when the file parses. */
static int open_and_parse(const uint8_t *data, size_t len)
{
    avifResult res;
    avifDecoder *d = parse(data, len, 0, &res);
    if (d && res == AVIF_RESULT_OK && d->image) {
        sink += d->image->width + d->image->height + d->image->exif.size + d->image->xmp.size + d->image->icc.size;
    }
    if (d) {
        avifDecoderDestroy(d);
    }
    return res == AVIF_RESULT_OK ? 0 : -1;
}

static int bench_one(const char *path, const char *name, void *argp)
{
    long iters = *(const long *)argp, i;
    size_t len = 0;
    uint8_t *data = read_file(path, &len);
    uint64_t t0, t1;
    if (!data) {
        return 0;
    }
    if (!has_avif_brand(data, len) || open_and_parse(data, len) != 0) {
        free(data);
        return 0;
    }
    t0 = now_ns();
    for (i = 0; i < iters; i++) {
        open_and_parse(data, len);
    }
    t1 = now_ns();
    printf("%s\t%llu\n", name, (unsigned long long)((t1 - t0) / (uint64_t)iters));
    free(data);
    return 0;
}

int main(int argc, char **argv)
{
    if (argc == 4 && !strcmp(argv[1], "--bench")) {
        long iters = strtol(argv[2], NULL, 10);
        if (iters < 1) {
            fprintf(stderr, "avif-oracle: iters must be >= 1\n");
            return 2;
        }
        return walk(argv[3], "", bench_one, &iters) ? 1 : 0;
    }
    if (argc == 3) {
        if (mkdir(argv[2], 0777) != 0 && errno != EEXIST) {
            fprintf(stderr, "avif-oracle: %s: %s\n", argv[2], strerror(errno));
            return 2;
        }
        return walk(argv[1], "", dump_one, argv[2]) ? 1 : 0;
    }
    fprintf(stderr, "usage: avif-oracle <in-dir> <out-dir> | avif-oracle --bench <iters> <in-dir>\n");
    return 2;
}
