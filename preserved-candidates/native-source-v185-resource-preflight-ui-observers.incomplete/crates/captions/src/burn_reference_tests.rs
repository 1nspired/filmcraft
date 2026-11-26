// Frozen pre-refactor renderer from the pinned local source; independent pixel oracle.
#[allow(dead_code)] // Keep the complete frozen original renderer as an independent oracle.
mod previous {

    use filmcraft_color::srgb_to_linear;
    use filmcraft_project::{Caption, CaptionAlign, CaptionAnchor, CaptionStyle, CaptionTrack, Sequence};
    use filmcraft_time::Tick;

    use filmcraft_text::{ParagraphStyle, TextStyle, layout, render};

    /// A rendered caption: premultiplied linear-light RGBA f32 pixels at `(x, y)` in the frame.
    #[derive(Clone, Debug, PartialEq)]
    pub struct Overlay {
        pub x: i32,
        pub y: i32,
        pub w: usize,
        pub h: usize,
        pub px: Vec<f32>,
    }

    impl Overlay {
        /// Composite over a premultiplied RGBA f32 canvas of `cw`×`ch` pixels.
        pub fn composite_onto(&self, canvas: &mut [f32], cw: usize, ch: usize) {
            for y in 0..self.h {
                let cy = self.y + y as i32;
                if cy < 0 || cy >= ch as i32 {
                    continue;
                }
                for x in 0..self.w {
                    let cx = self.x + x as i32;
                    if cx < 0 || cx >= cw as i32 {
                        continue;
                    }
                    let s = &self.px[(y * self.w + x) * 4..(y * self.w + x) * 4 + 4];
                    if s[3] <= 0.0 {
                        continue;
                    }
                    let i = (cy as usize * cw + cx as usize) * 4;
                    let k = 1.0 - s[3];
                    for c in 0..4 {
                        canvas[i + c] = s[c] + canvas[i + c] * k;
                    }
                }
            }
        }
    }

    /// The caption face: the track's font family (Inter when empty or not installed) in SemiBold at `px`.
    fn caption_style(font: &str, px: f32) -> TextStyle {
        let family = if font.trim().is_empty() { filmcraft_text::fonts::DEFAULT_FAMILY } else { font.trim() };
        TextStyle { family: family.into(), style: "SemiBold".into(), size: px, ..Default::default() }
    }

    fn lin(c: [u8; 4]) -> [f32; 4] {
        let a = c[3] as f32 / 255.0;
        [srgb_to_linear(c[0] as f32 / 255.0) * a, srgb_to_linear(c[1] as f32 / 255.0) * a, srgb_to_linear(c[2] as f32 / 255.0) * a, a]
    }

    /// Word-wrap one line to `max_w` pixels.
    fn wrap(line: &str, ts: &TextStyle, max_w: f32) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        for word in line.split(' ') {
            let cand = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
            if !cur.is_empty() && filmcraft_text::measure(&cand, ts) > max_w {
                out.push(std::mem::take(&mut cur));
                cur = word.to_string();
            } else {
                cur = cand;
            }
        }
        out.push(cur);
        out
    }

    /// Placement overrides from WebVTT cue settings.
    fn vtt_overrides(settings: &str) -> (Option<f32>, Option<CaptionAlign>) {
        let mut line = None;
        let mut align = None;
        for kv in settings.split_whitespace() {
            let Some((k, v)) = kv.split_once(':') else { continue };
            match k {
                "line" => {
                    let v = v.split(',').next().unwrap_or("");
                    if let Some(p) = v.strip_suffix('%').and_then(|p| p.parse::<f32>().ok()) {
                        line = Some((p / 100.0).clamp(0.0, 1.0));
                    }
                }
                "align" => {
                    align = match v {
                        "start" | "left" => Some(CaptionAlign::Left),
                        "end" | "right" => Some(CaptionAlign::Right),
                        "center" | "middle" => Some(CaptionAlign::Center),
                        _ => None,
                    }
                }
                _ => {}
            }
        }
        (line, align)
    }

    /// Lay out and rasterise one caption for a `w`×`h` frame.
    pub fn render_caption(c: &Caption, style: &CaptionStyle, w: usize, h: usize) -> Option<Overlay> {
        if w == 0 || h == 0 {
            return None;
        }
        let scale = h as f32 / 1080.0;
        let px = (style.size * scale).max(4.0);
        let max_w = w as f32 * 0.9;
        let ts = caption_style(&style.font, px);
        let mut lines: Vec<String> = Vec::new();
        // (speaker names are metadata; like Premiere, they are not burned in)
        for l in c.plain_lines() {
            lines.extend(wrap(l.trim(), &ts, max_w));
        }
        lines.retain(|l| !l.is_empty());
        if lines.is_empty() {
            return None;
        }
        let vm = filmcraft_text::fonts::face(filmcraft_text::resolve(&ts.family, &ts.style).face).metrics(px);
        let (asc, desc) = (vm.ascent, vm.descent);
        let lh = px * style.line_spacing.max(0.8);
        let pad_x = (px * 0.3).round();
        let block_h = lh * lines.len() as f32;
        let (line_override, align_override) = vtt_overrides(&c.settings);
        let align = align_override.unwrap_or(style.align);
        let margin = style.margin.clamp(0.0, 0.45) * h as f32;
        let top = match (line_override, style.anchor) {
            (Some(f), _) => (f * h as f32).min(h as f32 - block_h),
            (None, CaptionAnchor::Top) => margin,
            (None, CaptionAnchor::Middle) => (h as f32 - block_h) / 2.0,
            (None, CaptionAnchor::Bottom) => h as f32 - margin - block_h,
        }
        .max(0.0)
        .round();
        let side = w as f32 * 0.05;
        let widths: Vec<f32> = lines.iter().map(|l| filmcraft_text::measure(l, &ts)).collect();
        let xs: Vec<f32> = widths
            .iter()
            .map(|&lw| match align {
                CaptionAlign::Left => side + pad_x,
                CaptionAlign::Right => w as f32 - side - pad_x - lw,
                CaptionAlign::Center => (w as f32 - lw) / 2.0,
            })
            .collect();
        let outline = (style.outline * scale).max(0.0);
        let grow = outline.ceil() + 1.0;
        let bx0 = xs.iter().zip(&widths).map(|(x, _)| x - pad_x - grow).fold(f32::MAX, f32::min).floor();
        let bx1 = xs.iter().zip(&widths).map(|(x, lw)| x + lw + pad_x + grow).fold(f32::MIN, f32::max).ceil();
        let by0 = (top - grow).floor();
        let by1 = (top + block_h + grow).ceil();
        let (ox, oy) = (bx0 as i32, by0 as i32);
        let ow = (bx1 - bx0).max(1.0) as usize;
        let oh = (by1 - by0).max(1.0) as usize;
        let mut cover = vec![0.0f32; ow * oh];
        let mut bg = vec![0.0f32; ow * oh];
        for (i, line) in lines.iter().enumerate() {
            let lt = top + lh * i as f32;
            if style.background && style.background_color[3] > 0 {
                let (x0, x1) = (xs[i] - pad_x, xs[i] + widths[i] + pad_x);
                let (y0, y1) = (lt, lt + lh);
                for y in (y0.floor() as i32).max(oy)..(y1.ceil() as i32).min(oy + oh as i32) {
                    let fy = ((y as f32 + 1.0).min(y1) - (y as f32).max(y0)).clamp(0.0, 1.0);
                    for x in (x0.floor() as i32).max(ox)..(x1.ceil() as i32).min(ox + ow as i32) {
                        let fx = ((x as f32 + 1.0).min(x1) - (x as f32).max(x0)).clamp(0.0, 1.0);
                        let j = (y - oy) as usize * ow + (x - ox) as usize;
                        bg[j] = (bg[j] + fx * fy).min(1.0);
                    }
                }
            }
            let baseline = lt + (lh - (asc + desc)) / 2.0 + asc;
            let l = layout(line, &ts, &ParagraphStyle::default());
            let mut m = filmcraft_text::Mask { w: ow, h: oh, a: std::mem::take(&mut cover) };
            render::draw(&l, &render::at(xs[i], baseline.round()), &mut m, (ox, oy));
            cover = m.a;
        }
        let stroke = if outline > 0.0 { dilate(&cover, ow, oh, outline) } else { Vec::new() };
        let (tc, bc, oc) = (lin(style.color), lin(style.background_color), lin(style.outline_color));
        let mut out = vec![0.0f32; ow * oh * 4];
        for j in 0..ow * oh {
            let mut p = [0.0f32; 4];
            let over = |p: &mut [f32; 4], c: [f32; 4], a: f32| {
                if a > 0.0 {
                    for k in 0..4 {
                        p[k] = c[k] * a + p[k] * (1.0 - c[3] * a);
                    }
                }
            };
            over(&mut p, bc, bg[j]);
            if !stroke.is_empty() {
                over(&mut p, oc, stroke[j]);
            }
            over(&mut p, tc, cover[j]);
            out[j * 4..j * 4 + 4].copy_from_slice(&p);
        }
        Some(Overlay { x: ox, y: oy, w: ow, h: oh, px: out })
    }

    /// Max-filter a coverage mask with a disc of radius `r`.
    fn dilate(m: &[f32], w: usize, h: usize, r: f32) -> Vec<f32> {
        let ri = r.ceil() as i32;
        let mut offs = Vec::new();
        for dy in -ri..=ri {
            for dx in -ri..=ri {
                let d = ((dx * dx + dy * dy) as f32).sqrt();
                if d <= r + 0.5 {
                    offs.push((dx, dy, (r + 0.5 - d).clamp(0.0, 1.0)));
                }
            }
        }
        let mut out = vec![0.0f32; w * h];
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let mut v = 0.0f32;
                for &(dx, dy, wgt) in &offs {
                    let (sx, sy) = (x + dx, y + dy);
                    if sx >= 0 && sy >= 0 && sx < w as i32 && sy < h as i32 {
                        v = v.max(m[sy as usize * w + sx as usize] * wgt);
                    }
                }
                out[y as usize * w + x as usize] = v;
            }
        }
        out
    }

    /// The overlay for a caption track at timeline time `t` (None when nothing shows).
    pub fn track_overlay(track: &CaptionTrack, t: Tick, w: usize, h: usize) -> Option<Overlay> {
        if !track.enabled {
            return None;
        }
        let c = track.caption_at(t)?;
        render_caption(c, &track.style, w, h)
    }

    /// Overlays of every visible caption track of a sequence at `t`, bottom track first.
    pub fn sequence_overlays(seq: &Sequence, t: Tick, w: usize, h: usize) -> Vec<Overlay> {
        seq.caption_tracks.iter().rev().filter_map(|tr| track_overlay(tr, t, w, h)).collect()
    }
}

use super::*;
use filmcraft_project::{CaptionAlign, CaptionAnchor, ClipId};
use filmcraft_time::Tick;
fn cue(text: &str, settings: &str) -> Caption {
    Caption { id: ClipId(1), start: Tick(0), duration: Tick(1), text: text.into(), speaker: None, cue_id: None, settings: settings.into() }
}
#[test]
fn shared_caption_layout_keeps_original_raster_pixels_for_wrapping_unicode_fonts_alignment_anchors_and_vtt_overrides() {
    let mut count = 0;
    for font in ["", "Inter", "Noto Serif", "Poema intentionally missing caption family"] {
        for align in [CaptionAlign::Left, CaptionAlign::Center, CaptionAlign::Right] {
            for anchor in [CaptionAnchor::Top, CaptionAnchor::Middle, CaptionAnchor::Bottom] {
                for text in ["Words that wrap across a narrow canvas with punctuation.", "A😀é together", "שלום עולם مرحبا", "First line\nSecond line"]
                {
                    for settings in ["", "line:23% align:start", "line:80% align:end"] {
                        for (w, h) in [(160, 90), (480, 270)] {
                            let style = CaptionStyle { font: font.into(), align, anchor, ..Default::default() };
                            let c = cue(text, settings);
                            let before = previous::render_caption(&c, &style, w, h);
                            let after = render_caption(&c, &style, w, h);
                            match (before, after) {
                                (Some(a), Some(b)) => {
                                    assert_eq!((a.x, a.y, a.w, a.h), (b.x, b.y, b.w, b.h));
                                    assert_eq!(a.px, b.px, "{font} {align:?} {anchor:?} {text} {settings} {w}x{h}");
                                }
                                (None, None) => {}
                                _ => panic!("renderer visibility changed"),
                            };
                            count += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(count, 864);
}
#[test]
fn shared_caption_layout_keeps_original_empty_zero_canvas_and_line_shapes_without_pixel_allocation() {
    for text in ["", " ", "\n", "<v Speaker>Visible words"] {
        for (w, h) in [(0, 90), (160, 0), (160, 90)] {
            let c = cue(text, "");
            let style = CaptionStyle::default();
            assert_eq!(render_caption(&c, &style, w, h).is_some(), previous::render_caption(&c, &style, w, h).is_some());
        }
    }
    let c = cue("Exact prepared words", "line:45% align:right");
    let p = layout_caption(&c, &CaptionStyle::default(), 480, 270).unwrap();
    assert_eq!(p.style.style, "SemiBold");
    assert_eq!(p.lines.len(), p.xs.len());
    for (i, line) in p.lines.iter().enumerate() {
        assert_eq!(*p.line_layout(i).unwrap(), *filmcraft_text::layout(line, &p.style, &ParagraphStyle::default()));
    }
    assert!(p.line_layout(p.lines.len()).is_none());
}
