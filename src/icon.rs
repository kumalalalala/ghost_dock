//! Single-source-of-truth supersampled SDF icon renderer for GhostDock.

/// Signed distance to an axis-aligned rounded box centered at `(cx, cy)`.
#[inline]
fn sd_rounded_box(px: f32, py: f32, cx: f32, cy: f32, hx: f32, hy: f32, r: f32) -> f32 {
    let qx = (px - cx).abs() - hx + r;
    let qy = (py - cy).abs() - hy + r;
    let out_x = qx.max(0.0);
    let out_y = qy.max(0.0);
    (out_x * out_x + out_y * out_y).sqrt() + qx.max(qy).min(0.0) - r
}

/// Approximate signed distance to a centered squircle (`|x|^p + |y|^p = radius^p`).
#[inline]
fn sd_squircle(px: f32, py: f32, radius: f32, power: f32) -> f32 {
    let ax = px.abs();
    let ay = py.abs();
    let norm = (ax.powf(power) + ay.powf(power)).powf(1.0 / power);
    norm - radius
}

#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn blend_over(dst: [f32; 4], src: [f32; 4]) -> [f32; 4] {
    let sa = src[3].clamp(0.0, 1.0);
    let inv = 1.0 - sa;
    [
        src[0] * sa + dst[0] * inv,
        src[1] * sa + dst[1] * inv,
        src[2] * sa + dst[2] * inv,
        (sa + dst[3] * inv).clamp(0.0, 1.0),
    ]
}

fn sample_pixel(u: f32, v: f32, aa: f32) -> [f32; 4] {
    let mut col = [0.0f32; 4];

    // 1. Outer obsidian squircle badge
    let d_badge = sd_squircle(u, v, 0.88, 4.4);
    let badge_alpha = 1.0 - smoothstep(-aa, aa, d_badge);
    if badge_alpha <= 0.001 {
        return col;
    }

    // Subtle vertical obsidian gradient (#1E1E25 -> #0A0A0C)
    let grad_t = ((v + 0.88) / 1.76).clamp(0.0, 1.0);
    let bg_r = (30.0 * (1.0 - grad_t) + 10.0 * grad_t) / 255.0;
    let bg_g = (30.0 * (1.0 - grad_t) + 10.0 * grad_t) / 255.0;
    let bg_b = (37.0 * (1.0 - grad_t) + 13.0 * grad_t) / 255.0;
    col = [bg_r, bg_g, bg_b, badge_alpha];

    // Specular hairline inner rim (brighter at top, darker at bottom)
    let rim_band = (1.0 - smoothstep(0.0, 0.055 + aa, d_badge.abs())) * badge_alpha;
    if rim_band > 0.0 {
        let top_lit = (1.0 - grad_t * 0.65).clamp(0.25, 1.0);
        let rim = [
            (78.0 * top_lit) / 255.0,
            (78.0 * top_lit) / 255.0,
            (94.0 * top_lit) / 255.0,
            rim_band * 0.85,
        ];
        col = blend_over(col, rim);
    }

    // 2. Back cloaked window (top-right offset, muted slate #68687A)
    let d_back = sd_rounded_box(u, v, 0.14, -0.14, 0.30, 0.24, 0.075);
    let back_fill_a = 1.0 - smoothstep(-aa, aa, d_back);
    if back_fill_a > 0.0 {
        col = blend_over(
            col,
            [26.0 / 255.0, 26.0 / 255.0, 34.0 / 255.0, back_fill_a * 0.9],
        );
        let stroke_w = 0.036;
        let back_stroke = 1.0 - smoothstep(stroke_w - aa, stroke_w + aa, d_back.abs());
        if back_stroke > 0.0 {
            col = blend_over(
                col,
                [108.0 / 255.0, 108.0 / 255.0, 126.0 / 255.0, back_stroke * 0.92],
            );
        }
    }

    // 3. Front active window (bottom-left offset, crisp bone-white #F4F4F6 frame)
    let d_front = sd_rounded_box(u, v, -0.11, 0.12, 0.32, 0.25, 0.075);
    let front_fill_a = 1.0 - smoothstep(-aa, aa, d_front);
    if front_fill_a > 0.0 {
        // Opaque dark interior cleanly masks overlapping back window
        col = blend_over(col, [13.0 / 255.0, 13.0 / 255.0, 16.0 / 255.0, front_fill_a]);

        // Subtle header divider line inside front window
        let in_header_y = 1.0 - smoothstep(-aa, aa, (v - (-0.01)).abs() - 0.014);
        let in_header_x = 1.0 - smoothstep(-aa, aa, (u - (-0.11)).abs() - 0.27);
        let header_a = in_header_y * in_header_x * front_fill_a;
        if header_a > 0.0 {
            col = blend_over(
                col,
                [76.0 / 255.0, 76.0 / 255.0, 90.0 / 255.0, header_a * 0.7],
            );
        }

        // Crisp emerald status dot (#34D399) in front window title area
        let dx = u - (-0.28);
        let dy = v - (-0.055);
        let d_dot = (dx * dx + dy * dy).sqrt() - 0.042;
        let dot_a = (1.0 - smoothstep(-aa, aa, d_dot)) * front_fill_a;
        if dot_a > 0.0 {
            col = blend_over(col, [52.0 / 255.0, 211.0 / 255.0, 153.0 / 255.0, dot_a]);
        }

        // Crisp bone-white outer frame stroke
        let stroke_w = 0.042;
        let front_stroke = 1.0 - smoothstep(stroke_w - aa, stroke_w + aa, d_front.abs());
        if front_stroke > 0.0 {
            col = blend_over(
                col,
                [244.0 / 255.0, 244.0 / 255.0, 246.0 / 255.0, front_stroke],
            );
        }
    }

    col
}

/// Render straight non-premultiplied RGBA8 pixels for a square icon of `size x size`.
pub fn render_ghost_dock_rgba(size: u32) -> Vec<u8> {
    let mut out = vec![0u8; (size * size * 4) as usize];
    let inv_size = 1.0 / size as f32;
    let aa = (1.6 / size as f32).max(0.008);
    let sub_offsets = [0.25f32, 0.75f32];

    for y in 0..size {
        for x in 0..size {
            let mut acc = [0.0f32; 4];
            for sy in sub_offsets {
                for sx in sub_offsets {
                    let u = ((x as f32 + sx) * inv_size) * 2.0 - 1.0;
                    let v = ((y as f32 + sy) * inv_size) * 2.0 - 1.0;
                    let s = sample_pixel(u, v, aa);
                    acc[0] += s[0];
                    acc[1] += s[1];
                    acc[2] += s[2];
                    acc[3] += s[3];
                }
            }
            let idx = ((y * size + x) * 4) as usize;
            out[idx] = ((acc[0] * 0.25).clamp(0.0, 1.0) * 255.0).round() as u8;
            out[idx + 1] = ((acc[1] * 0.25).clamp(0.0, 1.0) * 255.0).round() as u8;
            out[idx + 2] = ((acc[2] * 0.25).clamp(0.0, 1.0) * 255.0).round() as u8;
            out[idx + 3] = ((acc[3] * 0.25).clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }

    out
}
