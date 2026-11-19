/* libheif oracle of isobmff-rs (CI only, docs/parity.md): writes the item graph of every input file
 * in the format of "graph.txt". Written from the public libheif API (LGPL-3.0 library, linked
 * statically into this CI tool, never shipped); no libheif code is copied.
 *
 *   heif-oracle <in-dir> <out-dir>
 *   heif-oracle --bench <iters> <in-dir>      "<name>\t<ns per open+walk>" per file
 *
 * Every regular file below <in-dir> is read (name = relative path with / replaced by __). A file
 * above 512 MiB is skipped. Security limits are off.
 */
#define _POSIX_C_SOURCE 200809L
#include <dirent.h>
#include <errno.h>
#include <libheif/heif.h>
#include <libheif/heif_items.h>
#include <libheif/heif_properties.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>

#define MAX_FILE ((uint64_t)512 << 20)

static volatile uint64_t sink;

/* ---- output helpers ---- */

/* A string: bytes 0x21..0x7e except '%' as is, the rest %XX; empty is "". */
static void esc(FILE *f, const unsigned char *s, size_t n)
{
    size_t i;
    if (n == 0) {
        fputs("\"\"", f);
        return;
    }
    for (i = 0; i < n; i++) {
        unsigned char c = s[i];
        if (c >= 0x21 && c <= 0x7e && c != '%') {
            fputc(c, f);
        } else {
            fprintf(f, "%%%02X", c);
        }
    }
}

static void escs(FILE *f, const char *s)
{
    esc(f, (const unsigned char *)(s ? s : ""), s ? strlen(s) : 0);
}

/* The bytes of s as Rust's String::from_utf8_lossy gives them (one U+FFFD per maximal invalid
 * sequence). out holds at least 3 * n bytes; returns the length written. */
static size_t utf8_lossy(const unsigned char *s, size_t n, unsigned char *out)
{
    static const unsigned char repl[3] = {0xEF, 0xBF, 0xBD};
    size_t i = 0, o = 0;
    while (i < n) {
        unsigned char c = s[i], lo = 0x80, hi = 0xBF;
        size_t need = 0, k, j;
        if (c < 0x80) {
            out[o++] = c;
            i++;
            continue;
        }
        if (c >= 0xC2 && c <= 0xDF) {
            need = 1;
        } else if (c >= 0xE0 && c <= 0xEF) {
            need = 2;
            if (c == 0xE0) {
                lo = 0xA0;
            } else if (c == 0xED) {
                hi = 0x9F;
            }
        } else if (c >= 0xF0 && c <= 0xF4) {
            need = 3;
            if (c == 0xF0) {
                lo = 0x90;
            } else if (c == 0xF4) {
                hi = 0x8F;
            }
        }
        j = i + 1;
        for (k = 0; k < need; k++, j++) {
            if (j >= n || s[j] < lo || s[j] > hi) {
                break;
            }
            lo = 0x80;
            hi = 0xBF;
        }
        if (need > 0 && k == need) {
            memcpy(out + o, s + i, need + 1);
            o += need + 1;
        } else {
            memcpy(out + o, repl, 3);
            o += 3;
        }
        i = j;
    }
    return o;
}

/* A string that isobmff-rs stores as UTF-8 (item names, content types, URIs, aux types): the
 * lossy conversion first, then esc. */
static void escl(FILE *f, const char *s)
{
    size_t n = s ? strlen(s) : 0;
    unsigned char *buf = (unsigned char *)malloc(n * 3 + 1);
    if (!buf) {
        escs(f, s);
        return;
    }
    esc(f, buf, utf8_lossy((const unsigned char *)(s ? s : ""), n, buf));
    free(buf);
}

/* One line on stderr, "name<TAB>what: msg", with tabs and newlines in msg replaced by spaces. */
static void note(const char *name, const char *what, const char *msg)
{
    const char *p;
    fprintf(stderr, "%s\t%s: ", name, what);
    for (p = msg ? msg : ""; *p; p++) {
        fputc(*p == '\t' || *p == '\n' || *p == '\r' ? ' ' : *p, stderr);
    }
    fputc('\n', stderr);
}

static void fcc(FILE *f, uint32_t v)
{
    unsigned char b[4];
    b[0] = (unsigned char)(v >> 24);
    b[1] = (unsigned char)(v >> 16);
    b[2] = (unsigned char)(v >> 8);
    b[3] = (unsigned char)v;
    esc(f, b, 4);
}

static int cmp_u32(const void *a, const void *b)
{
    uint32_t x = *(const uint32_t *)a, y = *(const uint32_t *)b;
    return x < y ? -1 : x > y;
}

static void ids(FILE *f, const uint32_t *v, size_t n)
{
    size_t i;
    if (n == 0) {
        fputc('-', f);
        return;
    }
    for (i = 0; i < n; i++) {
        fprintf(f, i ? ",%u" : "%u", v[i]);
    }
}

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

/* Reads the whole file; NULL on any failure. *n is the length. */
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
        fprintf(stderr, "heif-oracle: %s: %s\n", dir, strerror(errno));
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
            fprintf(stderr, "heif-oracle: %s: larger than 512 MiB, skipped\n", path);
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

/* ---- the graph ---- */

static void dump_ids(FILE *f, uint32_t *v, int n)
{
    if (n > 1) {
        qsort(v, (size_t)n, sizeof *v, cmp_u32);
    }
    ids(f, v, n > 0 ? (size_t)n : 0);
}

/* Fills a sorted id list through list(h, ids, n); returns the count (0 on any failure). */
typedef int (*count_fn)(const heif_image_handle *);
typedef int (*list_fn)(const heif_image_handle *, heif_item_id *, int);

static int get_list(const heif_image_handle *h, count_fn count, list_fn list, uint32_t **out)
{
    int n = count(h), got;
    *out = NULL;
    if (n <= 0) {
        return 0;
    }
    *out = (uint32_t *)calloc((size_t)n, sizeof(uint32_t));
    if (!*out) {
        return 0;
    }
    got = list(h, *out, n);
    if (got <= 0) {
        free(*out);
        *out = NULL;
        return 0;
    }
    qsort(*out, (size_t)got, sizeof(uint32_t), cmp_u32);
    return got;
}

static int aux_count(const heif_image_handle *h)
{
    return heif_image_handle_get_number_of_auxiliary_images(h, 0);
}

static int aux_list(const heif_image_handle *h, heif_item_id *v, int n)
{
    return heif_image_handle_get_list_of_auxiliary_image_IDs(h, 0, v, n);
}

static int meta_count(const heif_image_handle *h)
{
    return heif_image_handle_get_number_of_metadata_blocks(h, NULL);
}

static int meta_list(const heif_image_handle *h, heif_item_id *v, int n)
{
    return heif_image_handle_get_list_of_metadata_block_IDs(h, NULL, v, n);
}

static void dump_image(FILE *f, const char *name, heif_context *ctx, const char *dir, heif_item_id id)
{
    heif_image_handle *h = NULL;
    uint32_t type = heif_item_get_item_type(ctx, id);
    uint32_t *v;
    int ispe_w, ispe_h, n, i;
    heif_color_profile_nclx *nclx = NULL;
    size_t icc_len;
    heif_error herr = heif_context_get_image_handle(ctx, id, &h);

    if (herr.code != heif_error_Ok || !h) {
        char what[64];
        snprintf(what, sizeof what, "image %u", id);
        note(name, what, herr.message);
        return;
    }
    ispe_w = heif_image_handle_get_ispe_width(h);
    ispe_h = heif_image_handle_get_ispe_height(h);
    fprintf(f, "image %u ", id);
    if (ispe_w == 0 && ispe_h == 0) {
        fputs("ispe=-", f);
    } else {
        fprintf(f, "ispe=%ux%u", (unsigned)ispe_w, (unsigned)ispe_h);
    }
    fprintf(f, " size=%dx%d alpha=%d prem=%d bits=", heif_image_handle_get_width(h),
            heif_image_handle_get_height(h), heif_image_handle_has_alpha_channel(h) ? 1 : 0,
            heif_image_handle_is_premultiplied_alpha(h) ? 1 : 0);
    if (type == heif_fourcc('h', 'v', 'c', '1') || type == heif_fourcc('a', 'v', '0', '1')) {
        fprintf(f, "%d/%d", heif_image_handle_get_luma_bits_per_pixel(h),
                heif_image_handle_get_chroma_bits_per_pixel(h));
    } else {
        fputc('-', f);
    }
    fputc('\n', f);

    /* colr */
    fprintf(f, "colr %u ", id);
    if (heif_image_handle_get_nclx_color_profile(h, &nclx).code == heif_error_Ok && nclx) {
        fprintf(f, "nclx=%d,%d,%d,%d", (int)nclx->color_primaries, (int)nclx->transfer_characteristics,
                (int)nclx->matrix_coefficients, nclx->full_range_flag ? 1 : 0);
    } else {
        fputc('-', f);
    }
    if (nclx) {
        heif_nclx_color_profile_free(nclx);
    }
    icc_len = heif_image_handle_get_raw_color_profile_size(h);
    if (icc_len > 0) {
        uint8_t *icc = (uint8_t *)malloc(icc_len);
        fprintf(f, " icc=%zu\n", icc_len);
        if (icc && heif_image_handle_get_raw_color_profile(h, icc).code == heif_error_Ok) {
            char path[4096];
            if (snprintf(path, sizeof path, "%s/icc-%u.bin", dir, id) < (int)sizeof path) {
                write_file(path, icc, icc_len);
            }
        }
        free(icc);
    } else {
        fputs(" icc=-\n", f);
    }

    /* aux */
    n = get_list(h, aux_count, aux_list, &v);
    for (i = 0; i < n; i++) {
        heif_image_handle *ah = NULL;
        const char *t = NULL;
        heif_error aerr = heif_image_handle_get_auxiliary_image_handle(h, v[i], &ah);
        fprintf(f, "aux %u %u ", id, v[i]);
        if (aerr.code == heif_error_Ok && ah) {
            if (heif_image_handle_get_auxiliary_type(ah, &t).code != heif_error_Ok) {
                t = NULL;
            }
            escl(f, t);
            heif_image_handle_release_auxiliary_type(ah, &t);
            heif_image_handle_release(ah);
        } else {
            char what[64];
            snprintf(what, sizeof what, "aux image %u", v[i]);
            note(name, what, aerr.message);
            fputc('-', f);
        }
        fputc('\n', f);
    }
    free(v);

    /* depth, thumb */
    n = get_list(h, heif_image_handle_get_number_of_depth_images,
                 heif_image_handle_get_list_of_depth_image_IDs, &v);
    for (i = 0; i < n; i++) {
        fprintf(f, "depth %u %u\n", id, v[i]);
    }
    free(v);
    n = get_list(h, heif_image_handle_get_number_of_thumbnails,
                 heif_image_handle_get_list_of_thumbnail_IDs, &v);
    for (i = 0; i < n; i++) {
        fprintf(f, "thumb %u %u\n", id, v[i]);
    }
    free(v);

    /* meta */
    n = get_list(h, meta_count, meta_list, &v);
    for (i = 0; i < n; i++) {
        fprintf(f, "meta %u %u ", id, v[i]);
        escs(f, heif_image_handle_get_metadata_type(h, v[i]));
        fputc(' ', f);
        escl(f, heif_image_handle_get_metadata_content_type(h, v[i]));
        fprintf(f, " %zu\n", heif_image_handle_get_metadata_size(h, v[i]));
    }
    free(v);

    /* grid */
    if (type == heif_fourcc('g', 'r', 'i', 'd')) {
        heif_image_tiling t;
        memset(&t, 0, sizeof t);
        t.version = 1;
        if (heif_image_handle_get_image_tiling(h, 0, &t).code == heif_error_Ok) {
            uint32_t x, y;
            fprintf(f, "grid %u %ux%u tile=%ux%u out=%ux%u tiles=", id, t.num_columns, t.num_rows,
                    t.tile_width, t.tile_height, t.image_width, t.image_height);
            if (t.num_columns == 0 || t.num_rows == 0) {
                fputc('-', f);
            }
            for (y = 0; y < t.num_rows; y++) {
                for (x = 0; x < t.num_columns; x++) {
                    heif_item_id tid = 0;
                    if (heif_image_handle_get_grid_image_tile_id(h, 0, x, y, &tid).code != heif_error_Ok) {
                        tid = 0;
                    }
                    fprintf(f, (x || y) ? ",%u" : "%u", tid);
                }
            }
            fputc('\n', f);
        }
    }
    heif_image_handle_release(h);
}

static int dump_graph(const char *name, heif_context *ctx, const uint8_t *data, size_t len, const char *dir)
{
    char path[4096];
    FILE *f;
    int n_items, n, i, k;
    uint32_t *item_ids = NULL, *top = NULL;
    heif_item_id primary = 0;
    heif_entity_group *groups;
    int n_groups = 0;

    if (snprintf(path, sizeof path, "%s/graph.txt", dir) >= (int)sizeof path || !(f = fopen(path, "wb"))) {
        fprintf(stderr, "heif-oracle: cannot write %s/graph.txt\n", dir);
        return 1;
    }
    fputs("ok\n", f);

    /* brand */
    {
        heif_brand2 major = len <= 0x7fffffff ? heif_read_main_brand(data, (int)len) : 0;
        heif_brand2 *brands = NULL;
        int nb = 0;
        fputs("brand ", f);
        if (major == 0) {
            fputs("\"\"", f);
        } else {
            fcc(f, major);
        }
        fputc(' ', f);
        if (len <= 0x7fffffff && heif_list_compatible_brands(data, (int)len, &brands, &nb).code == heif_error_Ok &&
            brands && nb > 0) {
            for (i = 0; i < nb; i++) {
                if (i) {
                    fputc(',', f);
                }
                fcc(f, brands[i]);
            }
        } else {
            fputc('-', f);
        }
        if (brands) {
            heif_free_list_of_compatible_brands(brands);
        }
        fputc('\n', f);
    }

    if (heif_context_get_primary_image_ID(ctx, &primary).code == heif_error_Ok) {
        fprintf(f, "primary %u\n", primary);
    } else {
        fputs("primary -\n", f);
    }

    /* item ids, ascending */
    n_items = heif_context_get_number_of_items(ctx);
    if (n_items > 0) {
        item_ids = (uint32_t *)calloc((size_t)n_items, sizeof(uint32_t));
        if (!item_ids) {
            n_items = 0;
        } else {
            n_items = heif_context_get_list_of_item_IDs(ctx, item_ids, n_items);
            if (n_items > 1) {
                qsort(item_ids, (size_t)n_items, sizeof(uint32_t), cmp_u32);
            }
        }
    } else {
        n_items = 0;
    }

    for (i = 0; i < n_items; i++) {
        uint32_t id = item_ids[i], type = heif_item_get_item_type(ctx, id);
        const char *s;
        fprintf(f, "item %u ", id);
        fcc(f, type);
        fprintf(f, " hidden=%d name=", heif_item_is_item_hidden(ctx, id) ? 1 : 0);
        escl(f, heif_item_get_item_name(ctx, id));
        fputs(" ctype=", f);
        if (type == heif_fourcc('m', 'i', 'm', 'e') && (s = heif_item_get_mime_item_content_type(ctx, id))) {
            escl(f, s);
        } else {
            fputc('-', f);
        }
        fputs(" cenc=", f);
        if (type == heif_fourcc('m', 'i', 'm', 'e') && (s = heif_item_get_mime_item_content_encoding(ctx, id))) {
            escl(f, s);
        } else {
            fputc('-', f);
        }
        fputs(" uri=", f);
        if (type == heif_fourcc('u', 'r', 'i', ' ') && (s = heif_item_get_uri_item_uri_type(ctx, id))) {
            escl(f, s);
        } else {
            fputc('-', f);
        }
        fputc('\n', f);
    }

    /* data */
    for (i = 0; i < n_items; i++) {
        uint32_t id = item_ids[i];
        enum heif_metadata_compression comp = heif_metadata_compression_off;
        uint8_t *bytes = NULL;
        size_t blen = 0;
        heif_error derr = heif_item_get_item_data(ctx, id, &comp, &bytes, &blen);
        if (derr.code == heif_error_Ok) {
            fprintf(f, "data %u %zu\n", id, blen);
            if (snprintf(path, sizeof path, "%s/item-%u.bin", dir, id) < (int)sizeof path) {
                write_file(path, bytes, bytes ? blen : 0);
            }
            if (bytes) {
                heif_release_item_data(ctx, &bytes);
            }
        } else {
            char what[64];
            fprintf(f, "data %u error\n", id);
            snprintf(what, sizeof what, "data %u", id);
            note(name, what, derr.message);
        }
    }

    /* ref: a reference with no targets returns 0 but sets the pointer, the end leaves it NULL */
    for (i = 0; i < n_items; i++) {
        uint32_t id = item_ids[i];
        for (k = 0; k < (1 << 20); k++) {
            uint32_t rtype = 0;
            heif_item_id *to = NULL;
            size_t cnt = heif_context_get_item_references(ctx, id, k, &rtype, &to);
            if (cnt == 0 && !to) {
                break;
            }
            fprintf(f, "ref %u ", id);
            fcc(f, rtype);
            fputc(' ', f);
            ids(f, to, to ? cnt : 0);
            fputc('\n', f);
            if (to) {
                heif_release_item_references(ctx, &to);
            }
        }
    }

    /* prop */
    for (i = 0; i < n_items; i++) {
        uint32_t id = item_ids[i];
        int np = heif_item_get_properties_of_type(ctx, id, heif_item_property_type_invalid, NULL, 0);
        heif_property_id *pids;
        if (np <= 0 || !(pids = (heif_property_id *)calloc((size_t)np, sizeof *pids))) {
            continue;
        }
        np = heif_item_get_properties_of_type(ctx, id, heif_item_property_type_invalid, pids, np);
        for (k = 0; k < np; k++) {
            uint32_t pt = (uint32_t)heif_item_get_property_type(ctx, id, pids[k]);
            fprintf(f, "prop %u %u ", id, pids[k]);
            fcc(f, pt);
            if (pt == heif_fourcc('i', 'r', 'o', 't')) {
                fprintf(f, " rot=%d", heif_item_get_property_transform_rotation_ccw(ctx, id, pids[k]));
            } else if (pt == heif_fourcc('i', 'm', 'i', 'r')) {
                fprintf(f, " axis=%d", (int)heif_item_get_property_transform_mirror(ctx, id, pids[k]));
            }
            fputc('\n', f);
        }
        free(pids);
    }

    /* group */
    groups = heif_context_get_entity_groups(ctx, 0, 0, &n_groups);
    if (groups) {
        for (i = 0; i < n_groups; i++) {
            fputs("group ", f);
            fcc(f, groups[i].entity_group_type);
            fprintf(f, " %u ", groups[i].entity_group_id);
            ids(f, groups[i].entities, groups[i].entities ? groups[i].num_entities : 0);
            fputc('\n', f);
        }
        heif_entity_groups_release(groups, n_groups);
    }

    /* images */
    n = heif_context_get_number_of_top_level_images(ctx);
    if (n > 0 && (top = (uint32_t *)calloc((size_t)n, sizeof(uint32_t))) != NULL) {
        n = heif_context_get_list_of_top_level_image_IDs(ctx, top, n);
        if (n > 1) {
            qsort(top, (size_t)n, sizeof(uint32_t), cmp_u32);
        }
        for (i = 0; i < n; i++) {
            dump_image(f, name, ctx, dir, top[i]);
        }
    }
    free(top);
    free(item_ids);
    if (fclose(f) != 0) {
        return 1;
    }
    return 0;
}

static heif_context *open_ctx(void)
{
    heif_context *ctx = heif_context_alloc();
    if (ctx && heif_context_set_security_limits(ctx, heif_get_disabled_security_limits()).code != heif_error_Ok) {
        heif_context_free(ctx);
        return NULL;
    }
    return ctx;
}

static int mkdir_p(const char *p)
{
    return mkdir(p, 0777) == 0 || errno == EEXIST ? 0 : -1;
}

struct dump_arg {
    const char *out;
};

static int dump_one(const char *path, const char *name, void *argp)
{
    const struct dump_arg *arg = (const struct dump_arg *)argp;
    char dir[4096], gp[4096];
    size_t len = 0;
    uint8_t *data;
    heif_context *ctx;
    heif_error rerr;
    int bad = 0;

    if (snprintf(dir, sizeof dir, "%s/%s", arg->out, name) >= (int)sizeof dir || mkdir_p(dir) != 0) {
        fprintf(stderr, "heif-oracle: cannot create output directory for %s\n", name);
        return 1;
    }
    snprintf(gp, sizeof gp, "%s/graph.txt", dir);
    ctx = open_ctx();
    if (!ctx) {
        fprintf(stderr, "heif-oracle: out of memory\n");
        return 1;
    }
    data = read_file(path, &len);
    if (!data) {
        note(name, "open", "cannot read the file");
        bad = write_file(gp, "error\n", 6) != 0;
    } else if ((rerr = heif_context_read_from_file(ctx, path, NULL)).code != heif_error_Ok) {
        note(name, "open", rerr.message);
        bad = write_file(gp, "error\n", 6) != 0;
    } else {
        bad = dump_graph(name, ctx, data, len, dir);
    }
    free(data);
    heif_context_free(ctx);
    return bad;
}

/* ---- bench ---- */

static uint64_t now_ns(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000000u + (uint64_t)ts.tv_nsec;
}

/* The work of the parse: open from memory, then enumerate items, references and properties. */
static int open_and_walk(const uint8_t *data, size_t len)
{
    heif_context *ctx = open_ctx();
    int n, i, k;
    uint32_t *v;
    uint64_t acc = 0;
    if (!ctx) {
        return -1;
    }
    if (heif_context_read_from_memory_without_copy(ctx, data, len, NULL).code != heif_error_Ok) {
        heif_context_free(ctx);
        return -1;
    }
    n = heif_context_get_number_of_items(ctx);
    v = n > 0 ? (uint32_t *)calloc((size_t)n, sizeof *v) : NULL;
    if (v) {
        n = heif_context_get_list_of_item_IDs(ctx, v, n);
        for (i = 0; i < n; i++) {
            const char *name;
            int np;
            acc += heif_item_get_item_type(ctx, v[i]) + (uint64_t)heif_item_is_item_hidden(ctx, v[i]);
            name = heif_item_get_item_name(ctx, v[i]);
            acc += name ? strlen(name) : 0;
            for (k = 0; k < (1 << 20); k++) {
                uint32_t rt = 0;
                heif_item_id *to = NULL;
                size_t c = heif_context_get_item_references(ctx, v[i], k, &rt, &to);
                if (c == 0 && !to) {
                    break;
                }
                acc += rt + c;
                if (to) {
                    heif_release_item_references(ctx, &to);
                }
            }
            np = heif_item_get_properties_of_type(ctx, v[i], heif_item_property_type_invalid, NULL, 0);
            if (np > 0) {
                heif_property_id *p = (heif_property_id *)calloc((size_t)np, sizeof *p);
                if (p) {
                    np = heif_item_get_properties_of_type(ctx, v[i], heif_item_property_type_invalid, p, np);
                    for (k = 0; k < np; k++) {
                        acc += (uint32_t)heif_item_get_property_type(ctx, v[i], p[k]);
                    }
                    free(p);
                }
            }
        }
    }
    free(v);
    sink += acc;
    heif_context_free(ctx);
    return 0;
}

struct bench_arg {
    long iters;
};

static int bench_one(const char *path, const char *name, void *argp)
{
    const struct bench_arg *arg = (const struct bench_arg *)argp;
    size_t len = 0;
    uint8_t *data = read_file(path, &len);
    uint64_t t0, t1;
    long i;
    if (!data) {
        return 0;
    }
    if (open_and_walk(data, len) != 0) { /* a file that does not open is not timed */
        free(data);
        return 0;
    }
    t0 = now_ns();
    for (i = 0; i < arg->iters; i++) {
        open_and_walk(data, len);
    }
    t1 = now_ns();
    printf("%s\t%llu\n", name, (unsigned long long)((t1 - t0) / (uint64_t)arg->iters));
    free(data);
    return 0;
}

int main(int argc, char **argv)
{
    int bad;
    heif_init(NULL);
    if (argc == 4 && !strcmp(argv[1], "--bench")) {
        struct bench_arg a;
        a.iters = strtol(argv[2], NULL, 10);
        if (a.iters < 1) {
            fprintf(stderr, "heif-oracle: iters must be >= 1\n");
            return 2;
        }
        bad = walk(argv[3], "", bench_one, &a);
    } else if (argc == 3) {
        struct dump_arg a;
        a.out = argv[2];
        if (mkdir_p(argv[2]) != 0) {
            fprintf(stderr, "heif-oracle: %s: %s\n", argv[2], strerror(errno));
            return 2;
        }
        bad = walk(argv[1], "", dump_one, &a);
    } else {
        fprintf(stderr, "usage: heif-oracle <in-dir> <out-dir> | heif-oracle --bench <iters> <in-dir>\n");
        return 2;
    }
    heif_deinit();
    return bad ? 1 : 0;
}
