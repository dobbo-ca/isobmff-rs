/* libheif transform oracle of isobmff-rs (CI only, docs/parity.md "Transforms"): decodes every
 * top-level image of every input file, and the auxiliary images libheif exposes (alpha, depth,
 * gain map), twice. Once with ignore_transformations (raw planes) and once with the transforms
 * applied by libheif (expected planes). Written from the public libheif API (LGPL-3.0 library,
 * linked statically into this CI tool, never shipped); no libheif code is copied.
 *
 *   heif-transform-oracle <in-dir> <raw-dir> <exp-dir>
 *
 * Every regular file below <in-dir> is read (name = relative path with / replaced by __). For the
 * image with item id N of file NAME:
 *   <raw-dir>/NAME/N.planes   the decoded planes without transforms
 *   <exp-dir>/NAME/N.planes   the decoded planes with the transforms of the item
 *   <raw-dir>/NAME/N.err, <exp-dir>/NAME/N.err   one short token instead (see docs/parity.md)
 * An image above 64 Mpixel (ispe) is not decoded (token "toolarge"). The decode runs with the
 * colourspace and chroma left undefined and the nclx passthrough on, so libheif keeps the native
 * planes. The chroma upsampling of libheif is not an option of the conversions that the
 * transforms trigger (docs/parity.md), so the decoding options set no upsampling algorithm.
 */
#define _POSIX_C_SOURCE 200809L
#include <dirent.h>
#include <errno.h>
#include <libheif/heif.h>
#include <libheif/heif_aux_images.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>

#define MAX_FILE ((uint64_t)512 << 20)
#define MAX_PIXELS ((uint64_t)64 << 20)
#define MAX_IMAGES 256

static const char *g_raw, *g_exp;

/* ---- directory walk (as heif_oracle.c) ---- */

typedef int (*file_fn)(const char *path, const char *name);

static int walk(const char *root, const char *rel, file_fn fn)
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
        fprintf(stderr, "heif-transform-oracle: %s: %s\n", dir, strerror(errno));
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
            bad |= walk(root, sub, fn);
            continue;
        }
        if (S_ISLNK(st.st_mode) && stat(path, &st) != 0) {
            continue;
        }
        if (!S_ISREG(st.st_mode)) {
            continue;
        }
        if ((uint64_t)st.st_size > MAX_FILE) {
            fprintf(stderr, "heif-transform-oracle: %s: larger than 512 MiB, skipped\n", path);
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
        bad |= fn(path, name);
    }
    closedir(d);
    return bad;
}

/* ---- output ---- */

struct pl {
    int w, h, bps;
    const uint8_t *p;
    size_t stride;
};

static int write_token(const char *dir, const char *name, uint32_t id, const char *tok)
{
    char p[4096];
    FILE *f;
    if (snprintf(p, sizeof p, "%s/%s/%u.err", dir, name, (unsigned)id) >= (int)sizeof p) {
        return 1;
    }
    f = fopen(p, "wb");
    if (!f) {
        return 1;
    }
    fprintf(f, "%s\n", tok);
    return fclose(f) != 0;
}

/* Writes the planes of img in the .planes format of docs/parity.md. Returns NULL, or the token of
 * an image that cannot be written (not planar YCbCr or monochrome, ...). */
static const char *write_planes(const heif_image *img, const char *path)
{
    heif_colorspace cs = heif_image_get_colorspace(img);
    heif_chroma ch = heif_image_get_chroma_format(img);
    heif_channel chans[4];
    struct pl pls[4];
    const char *cn;
    int n = 0, i;
    FILE *f;
    uint8_t buf[8192];

    if (cs == heif_colorspace_monochrome) {
        if (ch != heif_chroma_monochrome) {
            return "format";
        }
        cn = "400";
        chans[n++] = heif_channel_Y;
    } else if (cs == heif_colorspace_YCbCr) {
        cn = ch == heif_chroma_420 ? "420" : ch == heif_chroma_422 ? "422" : ch == heif_chroma_444 ? "444" : NULL;
        if (!cn) {
            return "format";
        }
        chans[n++] = heif_channel_Y;
        chans[n++] = heif_channel_Cb;
        chans[n++] = heif_channel_Cr;
    } else {
        return "colorspace";
    }
    if (heif_image_has_channel(img, heif_channel_Alpha)) {
        chans[n++] = heif_channel_Alpha;
    }
    for (i = 0; i < n; i++) {
        struct pl *q = &pls[i];
        if (!heif_image_has_channel(img, chans[i])) {
            return "channel";
        }
        q->w = heif_image_get_width(img, chans[i]);
        q->h = heif_image_get_height(img, chans[i]);
        q->bps = heif_image_get_bits_per_pixel_range(img, chans[i]) > 8 ? 2 : 1;
        q->stride = 0;
        q->p = heif_image_get_plane_readonly2(img, chans[i], &q->stride);
        if (q->w <= 0 || q->h <= 0 || !q->p || q->stride < (size_t)q->w * (size_t)q->bps ||
            (uint64_t)q->w * (uint64_t)q->h > MAX_PIXELS) {
            return "plane";
        }
    }
    f = fopen(path, "wb");
    if (!f) {
        return "io";
    }
    fprintf(f, "planes %s %dx%d %d\n", cn, pls[0].w, pls[0].h, n);
    for (i = 0; i < n; i++) {
        fprintf(f, "%dx%d %d\n", pls[i].w, pls[i].h, pls[i].bps);
    }
    for (i = 0; i < n; i++) {
        const struct pl *q = &pls[i];
        int y;
        for (y = 0; y < q->h; y++) {
            const uint8_t *row = q->p + (size_t)y * q->stride;
            if (q->bps == 1) {
                fwrite(row, 1, (size_t)q->w, f);
            } else {
                /* little-endian, whatever the host is */
                size_t x = 0, w = (size_t)q->w;
                while (x < w) {
                    size_t k, m = w - x < sizeof buf / 2 ? w - x : sizeof buf / 2;
                    for (k = 0; k < m; k++) {
                        uint16_t v;
                        memcpy(&v, row + 2 * (x + k), 2);
                        buf[2 * k] = (uint8_t)(v & 0xff);
                        buf[2 * k + 1] = (uint8_t)(v >> 8);
                    }
                    fwrite(buf, 1, 2 * m, f);
                    x += m;
                }
            }
        }
    }
    if (fclose(f) != 0) {
        return "io";
    }
    return NULL;
}

/* ---- one image ---- */

static int do_item(const char *name, const heif_image_handle *h, uint32_t id)
{
    char rp[4096], ep[4096];
    heif_decoding_options *opts;
    heif_image *img = NULL;
    heif_error err;
    const char *tok;
    int bad = 0;
    int iw = heif_image_handle_get_ispe_width(h), ih = heif_image_handle_get_ispe_height(h);

    if (iw < 0 || ih < 0 || (uint64_t)iw * (uint64_t)ih > MAX_PIXELS) {
        return write_token(g_raw, name, id, "toolarge");
    }
    if (snprintf(rp, sizeof rp, "%s/%s/%u.planes", g_raw, name, (unsigned)id) >= (int)sizeof rp ||
        snprintf(ep, sizeof ep, "%s/%s/%u.planes", g_exp, name, (unsigned)id) >= (int)sizeof ep) {
        return 1;
    }
    opts = heif_decoding_options_alloc();
    if (!opts) {
        return 1;
    }
    opts->output_image_nclx_profile_passthrough = 1;

    opts->ignore_transformations = 1;
    err = heif_decode_image(h, &img, heif_colorspace_undefined, heif_chroma_undefined, opts);
    if (err.code != heif_error_Ok) {
        bad = write_token(g_raw, name, id, "decode");
        goto done;
    }
    tok = write_planes(img, rp);
    heif_image_release(img);
    img = NULL;
    if (tok) {
        bad = write_token(g_raw, name, id, tok);
        goto done;
    }

    opts->ignore_transformations = 0;
    err = heif_decode_image(h, &img, heif_colorspace_undefined, heif_chroma_undefined, opts);
    if (err.code != heif_error_Ok) {
        bad = write_token(g_exp, name, id, err.subcode == heif_suberror_Invalid_clean_aperture ? "clap" : "decode");
        goto done;
    }
    tok = write_planes(img, ep);
    heif_image_release(img);
    img = NULL;
    if (tok) {
        bad = write_token(g_exp, name, id, tok);
    }
done:
    heif_decoding_options_free(opts);
    return bad;
}

/* ---- one file ---- */

static int seen_add(uint32_t *seen, int *ns, uint32_t id)
{
    int i;
    for (i = 0; i < *ns; i++) {
        if (seen[i] == id) {
            return 0;
        }
    }
    if (*ns >= MAX_IMAGES) {
        return 0;
    }
    seen[(*ns)++] = id;
    return 1;
}

static int do_aux(const char *name, const heif_image_handle *mh, uint32_t *seen, int *ns)
{
    heif_item_id ids[64];
    int n, got, k, bad = 0;

    n = heif_image_handle_get_number_of_auxiliary_images(mh, 0);
    if (n > 64) {
        n = 64;
    }
    got = n > 0 ? heif_image_handle_get_list_of_auxiliary_image_IDs(mh, 0, ids, n) : 0;
    for (k = 0; k < got; k++) {
        heif_image_handle *ah = NULL;
        if (seen_add(seen, ns, ids[k]) &&
            heif_image_handle_get_auxiliary_image_handle(mh, ids[k], &ah).code == heif_error_Ok) {
            bad |= do_item(name, ah, ids[k]);
            heif_image_handle_release(ah);
        }
    }
    n = heif_image_handle_get_number_of_depth_images(mh);
    if (n > 64) {
        n = 64;
    }
    got = n > 0 ? heif_image_handle_get_list_of_depth_image_IDs(mh, ids, n) : 0;
    for (k = 0; k < got; k++) {
        heif_image_handle *dh = NULL;
        if (seen_add(seen, ns, ids[k]) &&
            heif_image_handle_get_depth_image_handle(mh, ids[k], &dh).code == heif_error_Ok) {
            bad |= do_item(name, dh, ids[k]);
            heif_image_handle_release(dh);
        }
    }
    return bad;
}

static int mkdir_p(const char *p)
{
    return mkdir(p, 0777) == 0 || errno == EEXIST ? 0 : -1;
}

static int do_file(const char *path, const char *name)
{
    char d1[4096], d2[4096];
    heif_context *ctx;
    heif_item_id *ids;
    uint32_t seen[MAX_IMAGES];
    int n, got, k, ns = 0, bad = 0;

    if (snprintf(d1, sizeof d1, "%s/%s", g_raw, name) >= (int)sizeof d1 ||
        snprintf(d2, sizeof d2, "%s/%s", g_exp, name) >= (int)sizeof d2 || mkdir_p(d1) != 0 ||
        mkdir_p(d2) != 0) {
        fprintf(stderr, "heif-transform-oracle: cannot create output directories for %s\n", name);
        return 1;
    }
    ctx = heif_context_alloc();
    if (!ctx) {
        return 1;
    }
    if (heif_context_set_security_limits(ctx, heif_get_disabled_security_limits()).code != heif_error_Ok) {
        heif_context_free(ctx);
        return 1;
    }
    if (heif_context_read_from_file(ctx, path, NULL).code != heif_error_Ok) {
        heif_context_free(ctx);
        return 0;
    }
    n = heif_context_get_number_of_top_level_images(ctx);
    if (n > MAX_IMAGES) {
        n = MAX_IMAGES;
    }
    ids = n > 0 ? (heif_item_id *)malloc((size_t)n * sizeof *ids) : NULL;
    got = ids ? heif_context_get_list_of_top_level_image_IDs(ctx, ids, n) : 0;
    for (k = 0; k < got; k++) {
        heif_image_handle *h = NULL;
        if (!seen_add(seen, &ns, ids[k]) ||
            heif_context_get_image_handle(ctx, ids[k], &h).code != heif_error_Ok) {
            continue;
        }
        bad |= do_item(name, h, ids[k]);
        bad |= do_aux(name, h, seen, &ns);
        heif_image_handle_release(h);
    }
    free(ids);
    heif_context_free(ctx);
    return bad;
}

int main(int argc, char **argv)
{
    int bad;
    if (argc != 4) {
        fprintf(stderr, "usage: heif-transform-oracle <in-dir> <raw-dir> <exp-dir>\n");
        return 2;
    }
    g_raw = argv[2];
    g_exp = argv[3];
    if (mkdir_p(g_raw) != 0 || mkdir_p(g_exp) != 0) {
        fprintf(stderr, "heif-transform-oracle: cannot create the output directories\n");
        return 2;
    }
    heif_init(NULL);
    bad = walk(argv[1], "", do_file);
    heif_deinit();
    return bad ? 1 : 0;
}
