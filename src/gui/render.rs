//! Software rendering: everything is drawn into a `tiny_skia::Pixmap`.
//!
//! Layers, back to front:
//!   1. background + border
//!   2. prompt (query line)
//!   3. visible rows (selection highlight, icon/thumbnail, title + dim subtitle)
//!   4. footer keybinding hints
//!
//! Text: cosmic-text shapes a `Buffer`, we rasterize each glyph through the
//! swash cache and blend it manually (glyphon's math, the reference renderer).

use crate::config::{Layout, Rgba};
use crate::state::{App, CellIcon, HitLayout};
use cosmic_text::{
    Attrs, Buffer, Color as CColor, Family, FontSystem, Metrics, Shaping, SwashCache, SwashContent,
};
use tiny_skia::{Color, Paint, Pixmap, PixmapRef, Rect, Transform};

pub struct Renderer {
    font_system: FontSystem,
    swash_cache: SwashCache,
}

/// Map a resolved family name onto cosmic-text's Family, keeping the
/// generic "monospace" keyword special (it means the default mono font).
fn family_for(name: &str) -> Family<'_> {
    if name.eq_ignore_ascii_case("monospace") {
        Family::Monospace
    } else {
        Family::Name(name)
    }
}

/// A fresh transparent pixmap sized to the window.
pub fn pixmap_for(width: u32, height: u32) -> Pixmap {
    Pixmap::new(width.max(1), height.max(1)).expect("pixmap allocation")
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            font_system: FontSystem::new(), // scans system fonts once
            swash_cache: SwashCache::new(),
        }
    }

    /// Render the whole UI into `pixmap`. `render_scale` is the physical
    /// scale the buffer is drawn at (1.0 on the no-HiDPI fallback path):
    /// all vector metrics (font, spacing, icons, rounding) multiply by it so
    /// text and geometry stay sharp on HiDPI outputs. Raster icons and
    /// thumbnails are still fetched at their logical size and upscaled by
    /// the blit below — a deliberate trade-off to keep the thumbnail cache
    /// small (see README, "Rendering & HiDPI").
    pub fn render(&mut self, app: &mut App, pixmap: &mut Pixmap, render_scale: f32) {
        let (w, h) = (pixmap.width() as f32, pixmap.height() as f32);
        let colors = app.cfg.colors;

        // resolve the configured font once per frame (cheap db lookup);
        // borrows from this local, not from `self`, so draw_* can take &mut self
        let font_name = self.resolve_font(&app.cfg.font);
        let family = family_for(&font_name);

        let border_px = 2.0 * render_scale;
        let pad = 12.0 * render_scale;
        let font_size = app.cfg.font_size * render_scale;
        let line_h = font_size * 1.4;
        let icon = app.cfg.icon_size as f32 * render_scale;
        let row_h = (icon + 10.0 * render_scale).max(line_h * 1.5);

        // background + border. With border-radius > 0 the panel is an
        // anti-aliased rounded rectangle; the pixmap starts fully transparent,
        // so the area outside the rounding stays see-through and the
        // compositor shows the desktop behind it. The frame is stroked on a
        // path inset by half the border width, keeping the full border
        // thickness inside the pixmap.
        let radius = app.cfg.border_radius as f32 * render_scale;
        if radius > 0.0 {
            if let Some(bg) = rounded_rect(0.0, 0.0, w, h, radius) {
                pixmap.fill_path(
                    &bg,
                    &paint_of(colors.background, true),
                    tiny_skia::FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
            let inset = border_px / 2.0;
            if let Some(frame) = rounded_rect(
                inset,
                inset,
                w - border_px,
                h - border_px,
                (radius - inset).max(0.0),
            ) {
                let stroke = tiny_skia::Stroke {
                    width: border_px,
                    ..Default::default()
                };
                pixmap.stroke_path(
                    &frame,
                    &paint_of(colors.border, true),
                    &stroke,
                    Transform::identity(),
                    None,
                );
            }
        } else {
            fill_rect(pixmap, 0.0, 0.0, w, h, colors.background);
            // border frame (square corners)
            fill_rect(pixmap, 0.0, 0.0, w, border_px, colors.border);
            fill_rect(pixmap, 0.0, h - border_px, w, border_px, colors.border);
            fill_rect(pixmap, 0.0, 0.0, border_px, h, colors.border);
            fill_rect(pixmap, w - border_px, 0.0, border_px, h, colors.border);
        }

        // prompt line: "[mode] > query▏" with the caret at its byte offset
        let prompt_x = pad;
        let prompt_y = border_px + pad;
        let prompt_max_w = w - 2.0 * pad;
        let dim = dimmed(colors.foreground, 0.55);
        let mut prompt_spans: Vec<(&str, Rgba)> = Vec::new();
        let badge;
        if app.dmenu || app.in_actions() || app.alt_entries.is_some() {
            badge = format!("[{}] ", app.mode_label());
            prompt_spans.push((badge.as_str(), colors.border));
        }
        prompt_spans.push(("> ", colors.border));
        // caret ▏ sits at the cursor position (mid-string while editing)
        let (before_caret, after_caret) = app.caret_split();
        prompt_spans.push((before_caret, colors.foreground));
        prompt_spans.push(("▏", dim));
        if !after_caret.is_empty() {
            prompt_spans.push((after_caret, colors.foreground));
        }
        self.draw_rich(
            pixmap,
            prompt_x,
            prompt_y,
            &prompt_spans,
            font_size,
            prompt_max_w,
            family,
        );

        // separator under the prompt
        let list_top = prompt_y + line_h * 1.6;
        fill_rect(
            pixmap,
            pad,
            list_top - 5.0,
            prompt_max_w,
            1.0,
            dimmed(colors.border, 0.5),
        );

        // footer hints (mode-dependent)
        let footer_y = h - border_px - pad - line_h;
        let hints = if app.in_actions() {
            // inside an app's actions submenu: getting back out matters most
            "↑/↓/click select · Enter run · Esc/⌫ back".to_string()
        } else if app.dmenu {
            "Enter choose · Esc cancel".to_string()
        } else if let Some(alt) = app.alt_label() {
            format!("↑/↓/click select · Enter run · Tab {alt} · Esc quit")
        } else {
            "↑/↓/click select · Enter run · Esc quit".to_string()
        };
        self.draw_text(
            pixmap,
            pad,
            footer_y,
            &hints,
            font_size - 1.0,
            dim,
            prompt_max_w,
            family,
        );

        // list area
        let usable = footer_y - 6.0 - list_top;
        let total = app.matches.len();
        if total == 0 {
            app.hit = None; // nothing to click on
            self.draw_text(
                pixmap,
                pad,
                list_top + row_h * 0.2,
                "no matches",
                font_size,
                dim,
                prompt_max_w,
                family,
            );
            return;
        }

        // --files grid: thumbnails side by side (config: layout "grid")
        if app.files_mode && app.cfg.layout == Layout::Grid {
            self.render_grid(
                app,
                pixmap,
                list_top,
                usable,
                w,
                pad,
                font_size,
                line_h,
                dim,
                family,
                render_scale,
            );
            return;
        }

        let visible = ((usable / row_h).floor() as usize).max(1);
        let sel = app.selected;
        let start = if sel >= visible { sel + 1 - visible } else { 0 };
        let end = (start + visible).min(total);

        // publish the row geometry for the pointer handlers (physical px)
        app.hit = Some(HitLayout {
            grid: false,
            start,
            total,
            top: list_top,
            row_h,
            rows: end - start,
            width: w,
            pad,
            cell: 0.0,
            cell_h: 0.0,
            gap: 0.0,
            cols: 1,
        });

        for row in start..end {
            let y = list_top + (row - start) as f32 * row_h;
            let entry_index = app.matches[row].index;

            if row == sel {
                fill_rect(
                    pixmap,
                    pad - 6.0,
                    y,
                    w - 2.0 * (pad - 6.0),
                    row_h,
                    colors.selected,
                );
            }

            // icon or thumbnail, centered in its square slot; image
            // thumbnails may still be decoding on a worker thread
            if let CellIcon::Ready(Some(ic)) = app.cell_icon(entry_index) {
                // fit into the physical slot; the blit scales with bilinear
                // filtering, so icons fetched at logical size still fill the
                // slot on HiDPI outputs
                let fit = (icon / ic.w as f32).min(icon / ic.h as f32);
                let dw = ic.w as f32 * fit;
                let dh = ic.h as f32 * fit;
                let ix = pad + (icon - dw) / 2.0;
                let iy = y + (row_h - dh) / 2.0;
                draw_icon(pixmap, &ic, ix, iy, fit);
            }

            // title + dim subtitle
            let entry = &app.entries[entry_index];
            let text_x = pad + icon + 10.0;
            let text_y = y + (row_h - line_h) / 2.0;
            let text_w = w - text_x - pad;

            if !entry.subtitle.is_empty() {
                let mut spans: Vec<(&str, Rgba)> = vec![(entry.title.as_str(), colors.foreground)];
                spans.push(("   ", dim));
                spans.push((entry.subtitle.as_str(), dim));
                self.draw_rich(pixmap, text_x, text_y, &spans, font_size, text_w, family);
            } else {
                self.draw_text(
                    pixmap,
                    text_x,
                    text_y,
                    &entry.title,
                    font_size,
                    colors.foreground,
                    text_w,
                    family,
                );
            }
        }
    }

    /// --files grid mode: thumbnails laid out side by side, filename under
    /// each. Publishes the computed column count back into `App` so that
    /// Up/Down move a whole row (list mode keeps the TUI at 1 column).
    #[allow(clippy::too_many_arguments)]
    fn render_grid(
        &mut self,
        app: &mut App,
        pixmap: &mut Pixmap,
        top: f32,
        usable_h: f32,
        w: f32,
        pad: f32,
        font_size: f32,
        line_h: f32,
        dim: Rgba,
        family: Family,
        render_scale: f32,
    ) {
        let colors = app.cfg.colors;
        let cell = app.cfg.grid_size as f32 * render_scale;
        let gap = 12.0;
        let inner_w = w - 2.0 * pad;
        let cols = ((inner_w + gap) / (cell + gap)).floor().max(1.0) as usize;
        app.grid_cols = cols;

        let label_h = line_h * 1.2;
        let cell_h = cell + label_h + gap;
        let total = app.matches.len();
        let vis_rows = ((usable_h / cell_h).floor() as usize).max(1);

        // scroll rows so the selection stays visible
        let sel = app.selected;
        let sel_row = sel / cols;
        let start_row = if sel_row >= vis_rows {
            sel_row + 1 - vis_rows
        } else {
            0
        };
        let end_row = (start_row + vis_rows).min(total.div_ceil(cols));

        // publish the cell geometry for the pointer handlers (physical px)
        app.hit = Some(HitLayout {
            grid: true,
            start: start_row,
            total,
            top,
            row_h: 0.0,
            rows: end_row - start_row,
            width: w,
            pad,
            cell,
            cell_h,
            gap,
            cols,
        });

        // monospace: one char is roughly 0.6em wide
        let max_label_chars = ((cell / (font_size * 0.62)).floor() as usize).max(3);

        for row in start_row..end_row {
            for col in 0..cols {
                let idx = row * cols + col;
                if idx >= total {
                    break;
                }
                let entry_index = app.matches[idx].index;
                let x = pad + col as f32 * (cell + gap);
                let y = top + (row - start_row) as f32 * cell_h;
                let selected = idx == sel;

                if selected {
                    fill_rect(
                        pixmap,
                        x - 5.0,
                        y - 5.0,
                        cell + 10.0,
                        cell_h - gap + 8.0,
                        colors.selected,
                    );
                }

                // thumbnail fitted into the cell, centered; while a worker
                // decodes it, show a dim placeholder square so the grid does
                // not look broken (the window opens instantly now)
                match app.cell_icon(entry_index) {
                    CellIcon::Ready(Some(ic)) => {
                        let fit = (cell / ic.w as f32).min(cell / ic.h as f32);
                        let dw = ic.w as f32 * fit;
                        let dh = ic.h as f32 * fit;
                        let ix = x + (cell - dw) / 2.0;
                        let iy = y + (cell - dh) / 2.0;
                        draw_icon(pixmap, &ic, ix, iy, fit);
                    }
                    CellIcon::Pending => {
                        fill_rect(pixmap, x, y, cell, cell, dimmed(colors.border, 0.25));
                    }
                    CellIcon::Ready(None) => {}
                }

                // filename underneath, truncated to one cell-width line
                let name = &app.entries[entry_index].title;
                let label: String = if name.chars().count() > max_label_chars {
                    let cut: String = name
                        .chars()
                        .take(max_label_chars.saturating_sub(1))
                        .collect();
                    format!("{cut}…")
                } else {
                    name.clone()
                };
                let color = if selected { colors.foreground } else { dim };
                self.draw_text(
                    pixmap,
                    x,
                    y + cell + 4.0,
                    &label,
                    font_size,
                    color,
                    cell,
                    family,
                );
            }
        }
    }

    /// Match the configured font name against installed families, forgiving
    /// with Nerd Font naming: "ComicShannsMono" finds "ComicShannsMono Nerd
    /// Font", and "ComicShannsMono Nerd" is stripped back to the base name
    /// first. Unknown names fall through to cosmic-text's fallback chain.
    fn resolve_font(&self, name: &str) -> String {
        let name = name.trim();
        if name.is_empty() || name.eq_ignore_ascii_case("monospace") {
            return "monospace".to_string();
        }

        // bases: the name as typed, plus the name with trailing Nerd Font
        // markers stripped ("ComicShannsMono Nerd" -> "ComicShannsMono")
        let mut bases = vec![name.to_string()];
        for marker in [" Nerd Font Mono", " Nerd Font", " Nerd", " Mono"] {
            if let Some(stripped) = name.strip_suffix(marker) {
                if !stripped.trim().is_empty() {
                    bases.push(stripped.trim().to_string());
                }
            }
        }

        // candidates: every base, plus each base with the full family names
        let mut candidates: Vec<String> = Vec::new();
        for base in &bases {
            candidates.push(base.clone());
            candidates.push(format!("{base} Nerd Font Mono"));
            candidates.push(format!("{base} Nerd Font"));
            candidates.push(format!("{base} Mono"));
        }

        for cand in &candidates {
            // a face lists its families as (name, language) pairs; a family
            // matches when any face carries the exact name
            if self
                .font_system
                .db()
                .faces()
                .any(|f| f.families.iter().any(|(n, _)| n == cand))
            {
                return cand.clone();
            }
        }
        name.to_string()
    }

    /// Single-color text.
    // A rasterizer draw call is naturally positional; packing (x, y, size,
    // color, max width, family) into a struct would not make call sites
    // clearer, so the lint is accepted here on purpose.
    #[allow(clippy::too_many_arguments)]
    fn draw_text(
        &mut self,
        pixmap: &mut Pixmap,
        x: f32,
        y: f32,
        text: &str,
        size: f32,
        color: Rgba,
        max_w: f32,
        family: Family,
    ) {
        let metrics = Metrics::new(size, size * 1.4);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_size(&mut self.font_system, Some(max_w), Some(size * 2.0));
        buffer.set_text(
            &mut self.font_system,
            text,
            Attrs::new().family(family),
            Shaping::Advanced,
        );
        buffer.shape_until_scroll(&mut self.font_system, false);
        self.draw_buffer(pixmap, &buffer, x, y, color);
    }

    /// Multi-colored spans, laid out on one line.
    #[allow(clippy::too_many_arguments)]
    fn draw_rich(
        &mut self,
        pixmap: &mut Pixmap,
        x: f32,
        y: f32,
        spans: &[(&str, Rgba)],
        size: f32,
        max_w: f32,
        family: Family,
    ) {
        let metrics = Metrics::new(size, size * 1.4);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_size(&mut self.font_system, Some(max_w), Some(size * 2.0));
        let default = Attrs::new().family(family);
        let span_iter = spans
            .iter()
            .map(|(text, c)| (*text, default.color(CColor::rgba(c[0], c[1], c[2], c[3]))));
        buffer.set_rich_text(&mut self.font_system, span_iter, default, Shaping::Advanced);
        buffer.shape_until_scroll(&mut self.font_system, false);
        self.draw_buffer(pixmap, &buffer, x, y, colors_default(spans));
    }

    /// Rasterize and blend a shaped buffer at (x, y) — glyphon's positioning.
    fn draw_buffer(&mut self, pixmap: &mut Pixmap, buffer: &Buffer, x: f32, y: f32, default: Rgba) {
        for run in buffer.layout_runs() {
            for glyph in run.glyphs.iter() {
                let physical = glyph.physical((x, y), 1.0);
                let Some(image) = self
                    .swash_cache
                    .get_image(&mut self.font_system, physical.cache_key)
                else {
                    continue;
                };

                // per-span color wins over the default
                let tint: Rgba = match glyph.color_opt {
                    Some(c) => [c.r(), c.g(), c.b(), c.a()],
                    None => default,
                };

                let gx = physical.x + image.placement.left;
                let gy = (run.line_y * 1.0).round() as i32 + physical.y - image.placement.top;
                blend_glyph(
                    pixmap,
                    &image.data,
                    image.content,
                    (image.placement.width as i32, image.placement.height as i32),
                    gx,
                    gy,
                    tint,
                );
            }
        }
    }
}

fn colors_default(spans: &[(&str, Rgba)]) -> Rgba {
    spans
        .first()
        .map(|(_, c)| *c)
        .unwrap_or([255, 255, 255, 255])
}

fn dimmed(c: Rgba, factor: f32) -> Rgba {
    [c[0], c[1], c[2], (c[3] as f32 * factor) as u8]
}

/// Fill an axis-aligned rectangle (premultiplied blend by tiny-skia).
fn fill_rect(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, color: Rgba) {
    let Some(rect) = Rect::from_xywh(x, y, w, h) else {
        return;
    };
    let paint = paint_of(color, false);
    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
}

/// Solid-color paint, with optional anti-aliasing.
fn paint_of(color: Rgba, anti_alias: bool) -> Paint<'static> {
    Paint {
        shader: tiny_skia::Shader::SolidColor(Color::from_rgba8(
            color[0], color[1], color[2], color[3],
        )),
        anti_alias,
        ..Default::default()
    }
}

/// A rounded-rectangle path (cubic corners, kappa ≈ 0.5523). `r` is clamped
/// to half the shorter side; r <= 0 degrades to a plain rect. Returns None
/// for empty geometry.
fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let rect = Rect::from_xywh(x, y, w.max(0.0), h.max(0.0))?;
    let r = r.clamp(0.0, rect.width().min(rect.height()) / 2.0);
    if r <= 0.01 {
        return Some(tiny_skia::PathBuilder::from_rect(rect));
    }
    let k = 0.552_284_7 * r;
    let (l, t, rt, b) = (x, y, x + w, y + h);
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(l + r, t);
    pb.line_to(rt - r, t);
    pb.cubic_to(rt - r + k, t, rt, t + r - k, rt, t + r);
    pb.line_to(rt, b - r);
    pb.cubic_to(rt, b - r + k, rt - r + k, b, rt - r, b);
    pb.line_to(l + r, b);
    pb.cubic_to(l + r - k, b, l, b - r + k, l, b - r);
    pb.line_to(l, t + r);
    pb.cubic_to(l, t + r - k, l + r - k, t, l + r, t);
    pb.close();
    pb.finish()
}

/// Draw a premultiplied RGBA icon at integer coordinates.
fn draw_icon(pixmap: &mut Pixmap, icon: &crate::icons::Icon, x: f32, y: f32, scale: f32) {
    let Some(ref_pixmap) = PixmapRef::from_bytes(&icon.rgba, icon.w, icon.h) else {
        return;
    };
    let paint = tiny_skia::PixmapPaint {
        quality: tiny_skia::FilterQuality::Bilinear,
        opacity: 1.0,
        blend_mode: tiny_skia::BlendMode::SourceOver,
    };
    // identity transform at scale 1 (byte-identical to the old 1:1 blit);
    // otherwise scale the icon into its physical slot
    let transform = if (scale - 1.0).abs() < f32::EPSILON {
        Transform::identity()
    } else {
        Transform::from_scale(scale, scale)
    };
    pixmap.draw_pixmap(
        x.round() as i32,
        y.round() as i32,
        ref_pixmap,
        &paint,
        transform,
        None,
    );
}

/// Blend one swash glyph image (alpha mask or color bitmap) into the pixmap.
fn blend_glyph(
    pixmap: &mut Pixmap,
    data: &[u8],
    content: SwashContent,
    (gw, gh): (i32, i32),
    gx: i32,
    gy: i32,
    tint: Rgba,
) {
    let (pw, ph) = (pixmap.width() as i32, pixmap.height() as i32);
    if gw <= 0 || gh <= 0 {
        return;
    }

    let bytes_pp = match content {
        SwashContent::Color => 4,
        _ => 1, // Mask / SubpixelMask → use as alpha
    };

    let dst = pixmap.data_mut();
    for row in 0..gh {
        let py = gy + row;
        if py < 0 || py >= ph {
            continue;
        }
        for col in 0..gw {
            let px = gx + col;
            if px < 0 || px >= pw {
                continue;
            }
            let src_i = ((row * gw + col) * bytes_pp) as usize;
            let (sr, sg, sb, sa) = match content {
                SwashContent::Color => (
                    data[src_i],
                    data[src_i + 1],
                    data[src_i + 2],
                    data[src_i + 3],
                ),
                _ => (
                    tint[0],
                    tint[1],
                    tint[2],
                    ((data[src_i] as u32 * tint[3] as u32) / 255) as u8,
                ),
            };
            if sa == 0 {
                continue;
            }
            let di = ((py * pw + px) * 4) as usize;
            let (dr, dg, db, da) = (dst[di], dst[di + 1], dst[di + 2], dst[di + 3]);
            // straight-alpha source over premultiplied destination
            let da = da as u32;
            let inv = 255 - sa as u32;
            let out_a = sa as u32 + (da * inv) / 255;
            let mix = |sc: u8, dc: u8| -> u8 {
                ((sc as u32 * sa as u32 + dc as u32 * da * inv / 255) / out_a.max(1)) as u8
            };
            let out = [mix(sr, dr), mix(sg, dg), mix(sb, db), out_a as u8];
            dst[di..di + 4].copy_from_slice(&out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounded_corners_leave_transparency() {
        let mut pm = Pixmap::new(100, 60).unwrap();
        let path = rounded_rect(0.0, 0.0, 100.0, 60.0, 12.0).unwrap();
        pm.fill_path(
            &path,
            &paint_of([9, 9, 9, 255], true),
            tiny_skia::FillRule::Winding,
            Transform::identity(),
            None,
        );
        let alpha = |x: u32, y: u32| pm.pixel(x, y).unwrap().alpha();
        // middle of every edge is opaque...
        assert_eq!(alpha(50, 30), 255);
        assert_eq!(alpha(50, 0), 255);
        assert_eq!(alpha(0, 30), 255);
        // ...but the four exact corners stay fully transparent
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(alpha(99, 0), 0);
        assert_eq!(alpha(0, 59), 0);
        assert_eq!(alpha(99, 59), 0);

        // r = 0 degrades to a full square (nothing transparent)
        let sq = rounded_rect(0.0, 0.0, 100.0, 60.0, 0.0).unwrap();
        pm.fill_path(
            &sq,
            &paint_of([9, 9, 9, 255], true),
            tiny_skia::FillRule::Winding,
            Transform::identity(),
            None,
        );
        assert_eq!(pm.pixel(0, 0).unwrap().alpha(), 255);
    }
}
