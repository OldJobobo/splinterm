/*
 * Test-only fcft raster evidence probe for Splinterm.
 *
 * Built against the fcft 3.3.3 subproject vendored by pinned Foot 1.27.0 at
 * commit 3c5b584b0eafa772eb4376fb6eaf6643399e190e. This file does not modify
 * Foot or fcft behavior. It emits one JSONL record per glyph, including the
 * tightly packed row-major alpha mask used by Phase 8.1 differential tests.
 */

#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <fcft/fcft.h>

struct bounds {
    int left;
    int top;
    int right;
    int bottom;
    bool any;
};

static uint8_t
alpha_at(const struct fcft_glyph *glyph, int x, int y)
{
    const pixman_format_code_t format = pixman_image_get_format(glyph->pix);
    const int stride = pixman_image_get_stride(glyph->pix);
    const uint8_t *data = (const uint8_t *)pixman_image_get_data(glyph->pix);
    const int bpp = PIXMAN_FORMAT_BPP(format);

    if (bpp == 8)
        return data[y * stride + x];
    if (bpp == 32) {
        const uint32_t *row = (const uint32_t *)(data + y * stride);
        return (uint8_t)(row[x] >> 24);
    }
    if (bpp == 1) {
        const uint32_t *row = (const uint32_t *)(data + y * stride);
        return (row[x / 32] & (1u << (x % 32))) != 0 ? 255 : 0;
    }

    fprintf(stderr, "unsupported pixman bpp: %d\n", bpp);
    exit(EXIT_FAILURE);
}

static struct bounds
ink_bounds(const struct fcft_glyph *glyph)
{
    struct bounds result = {0};
    for (int y = 0; y < glyph->height; y++) {
        for (int x = 0; x < glyph->width; x++) {
            if (alpha_at(glyph, x, y) == 0)
                continue;
            if (!result.any) {
                result = (struct bounds){x, y, x + 1, y + 1, true};
            } else {
                if (x < result.left) result.left = x;
                if (y < result.top) result.top = y;
                if (x + 1 > result.right) result.right = x + 1;
                if (y + 1 > result.bottom) result.bottom = y + 1;
            }
        }
    }
    return result;
}

static void
print_alpha_hex(const struct fcft_glyph *glyph)
{
    static const char hex[] = "0123456789abcdef";
    for (int y = 0; y < glyph->height; y++) {
        for (int x = 0; x < glyph->width; x++) {
            const uint8_t alpha = alpha_at(glyph, x, y);
            putchar(hex[alpha >> 4]);
            putchar(hex[alpha & 0x0f]);
        }
    }
}

static void
print_rgba_hex(const struct fcft_glyph *glyph)
{
    static const char hex[] = "0123456789abcdef";
    const int stride = pixman_image_get_stride(glyph->pix);
    const uint8_t *data = (const uint8_t *)pixman_image_get_data(glyph->pix);
    for (int y = 0; y < glyph->height; y++) {
        const uint32_t *row = (const uint32_t *)(data + y * stride);
        for (int x = 0; x < glyph->width; x++) {
            const uint32_t pixel = row[x];
            const uint8_t channels[] = {
                pixel >> 16, pixel >> 8, pixel, pixel >> 24,
            };
            for (size_t channel = 0; channel < 4; channel++) {
                putchar(hex[channels[channel] >> 4]);
                putchar(hex[channels[channel] & 0x0f]);
            }
        }
    }
}

static void
print_glyph(const char *label, const struct fcft_font *font,
            const struct fcft_glyph *glyph)
{
    if (glyph == NULL) {
        fprintf(stderr, "fcft did not rasterize %s\n", label);
        exit(EXIT_FAILURE);
    }
    const struct bounds bounds = ink_bounds(glyph);
    const pixman_format_code_t format = pixman_image_get_format(glyph->pix);
    printf(
        "{\"schema\":1,\"label\":\"%s\",\"codepoint\":%u,\"cols\":%d,"
        "\"font\":\"%s\",\"font_ascent\":%d,"
        "\"font_descent\":%d,\"font_height\":%d,\"color\":%s,"
        "\"decorations\":{\"underline_position\":%d,\"underline_thickness\":%d,"
        "\"strike_position\":%d,\"strike_thickness\":%d},"
        "\"pixel_format\":%u,\"source_stride\":%d,"
        "\"placement\":{\"x\":%d,\"y\":%d},"
        "\"image\":{\"width\":%d,\"height\":%d},"
        "\"advance\":{\"x\":%d,\"y\":%d},"
        "\"ink\":{\"left\":%d,\"top\":%d,\"right\":%d,\"bottom\":%d},"
        "\"alpha_hex\":\"",
        label, glyph->cp, glyph->cols,
        glyph->font_name != NULL ? glyph->font_name : "unknown",
        font->ascent, font->descent, font->height,
        glyph->is_color_glyph ? "true" : "false", font->underline.position,
        font->underline.thickness, font->strikeout.position,
        font->strikeout.thickness, (unsigned int)format,
        pixman_image_get_stride(glyph->pix), glyph->x, glyph->y, glyph->width,
        glyph->height, glyph->advance.x, glyph->advance.y, bounds.left,
        bounds.top, bounds.right, bounds.bottom);
    print_alpha_hex(glyph);
    putchar('"');
    if (glyph->is_color_glyph) {
        fputs(",\"rgba_hex\":\"", stdout);
        print_rgba_hex(glyph);
        putchar('"');
    }
    puts("}");
}

int
main(void)
{
    if (!fcft_init(FCFT_LOG_COLORIZE_NEVER, false, FCFT_LOG_CLASS_ERROR))
        return EXIT_FAILURE;

    const char *size = getenv("SPLINTERM_EVIDENCE_FONT_SIZE");
    if (size == NULL || size[0] == '\0')
        size = "22";
    const char *style = getenv("SPLINTERM_EVIDENCE_FONT_STYLE");
    if (style == NULL || style[0] == '\0')
        style = "Regular";
    if (strcmp(style, "Regular") != 0 && strcmp(style, "Bold") != 0 &&
        strcmp(style, "Italic") != 0 && strcmp(style, "Bold Italic") != 0) {
        fprintf(stderr, "unsupported evidence font style: %s\n", style);
        return EXIT_FAILURE;
    }
    char primary[192];
    char cjk[128];
    char emoji[128];
    if (snprintf(primary, sizeof(primary), "JetBrains Mono Nerd Font:style=%s:pixelsize=%s", style, size) >= sizeof(primary) ||
        snprintf(cjk, sizeof(cjk), "Noto Sans CJK JP:pixelsize=%s", size) >= sizeof(cjk) ||
        snprintf(emoji, sizeof(emoji), "Noto Color Emoji:pixelsize=%s", size) >= sizeof(emoji))
        return EXIT_FAILURE;
    const char *names[] = {primary, cjk, emoji};
    struct fcft_font *font = fcft_from_name(3, names, NULL);
    if (font == NULL)
        return EXIT_FAILURE;

    for (uint32_t cp = 0x20; cp <= 0x7e; cp++) {
        char label[32];
        snprintf(label, sizeof(label), "ASCII-U+%04X", cp);
        print_glyph(label, font,
                    fcft_rasterize_char_utf32(font, cp, FCFT_SUBPIXEL_NONE));
    }

    print_glyph("CJK", font,
                fcft_rasterize_char_utf32(font, 0x754c, FCFT_SUBPIXEL_NONE));
    print_glyph("emoji", font,
                fcft_rasterize_char_utf32(font, 0x1f642, FCFT_SUBPIXEL_NONE));

    const uint32_t combining[] = {0x0065, 0x0301};
    const struct fcft_grapheme *grapheme = fcft_rasterize_grapheme_utf32(
        font, 2, combining, FCFT_SUBPIXEL_NONE);
    if (grapheme == NULL)
        return EXIT_FAILURE;
    for (size_t i = 0; i < grapheme->count; i++) {
        char label[32];
        snprintf(label, sizeof(label), "combining-%zu", i);
        print_glyph(label, font, grapheme->glyphs[i]);
    }

    fcft_destroy(font);
    fcft_fini();
    return EXIT_SUCCESS;
}
