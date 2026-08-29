//! The tray icon, painted.
//!
//! One square, monotone, drawn from the current readings at whatever size
//! the taskbar wants. The renderer is pure -- a coverage canvas with a
//! handful of shapes and a five-row digit font -- so the same pixels serve
//! the shell (as a 32-bit icon), the settings preview (as a texture) and
//! the review dump (as PNGs), and the layouts can be tested by counting
//! pixels.
//!
//! What the icon shows is a setting: the static logo; the tightest limit
//! across the fleet; one provider's chosen value; or the whole fleet as a
//! row of bars. The value styles are a number, a bar that fills, and a
//! ring that fills.

use crate::app_settings::{TrayIconMetric, TrayIconMode, TrayIconSettings, TrayIconStyle};
use crate::models::{AppUsageData, UsageData};
use crate::providers::{ProviderId, ProviderSet};

/// What to draw this time.
#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    Logo,
    /// One percentage, 0 to 100 (more is clamped when drawn, shown as digits).
    Value { percent: f64, style: TrayIconStyle },
    /// One entry per enabled provider, in provider order; `None` for one
    /// with nothing current.
    Rundown(Vec<Option<f64>>),
}

/// The pixels: straight (non-premultiplied) RGBA, row-major, `size` square.
#[derive(Clone, Debug, PartialEq)]
pub struct Render {
    pub size: usize,
    pub rgba: Vec<u8>,
}

impl Render {
    /// The same pixels as premultiplied BGRA words, the layout a 32-bit
    /// GDI bitmap wants.
    pub fn bgra_premultiplied(&self) -> Vec<u32> {
        self.rgba
            .chunks_exact(4)
            .map(|px| {
                let a = u32::from(px[3]);
                let pre = |c: u8| (u32::from(c) * a + 127) / 255;
                (a << 24) | (pre(px[0]) << 16) | (pre(px[1]) << 8) | pre(px[2])
            })
            .collect()
    }
}

/// The percentage a provider contributes under a metric: its tightest
/// window, or specifically the session or weekly one.
pub fn provider_percent(usage: &UsageData, metric: TrayIconMetric) -> f64 {
    match metric {
        TrayIconMetric::Session => usage.session.percentage,
        TrayIconMetric::Weekly => usage.weekly.percentage,
        TrayIconMetric::Tightest => [usage.session.percentage, usage.weekly.percentage]
            .into_iter()
            .chain(usage.monthly.as_ref().map(|monthly| monthly.percentage))
            .chain(usage.scoped.iter().map(|scoped| scoped.section.percentage))
            .fold(0.0, f64::max),
    }
}

/// Decide what the icon shows from the settings and the latest readings.
/// Anything that needs a reading and has none falls back to the logo, so the
/// icon is never a blank shape.
pub fn content(settings: &TrayIconSettings, data: Option<&AppUsageData>, enabled: ProviderSet) -> Content {
    match settings.mode {
        TrayIconMode::Logo => Content::Logo,
        TrayIconMode::Tightest => {
            let tightest = data.and_then(|data| {
                enabled
                    .iter()
                    .filter_map(|provider| data.get(provider))
                    .map(|usage| provider_percent(usage, TrayIconMetric::Tightest))
                    .fold(None, |best: Option<f64>, value| Some(best.map_or(value, |best| best.max(value))))
            });
            match tightest {
                Some(percent) => Content::Value { percent, style: settings.style },
                None => Content::Logo,
            }
        }
        TrayIconMode::Provider => {
            let chosen = settings
                .provider
                .as_deref()
                .and_then(ProviderId::from_key)
                .or_else(|| enabled.iter().next());
            let percent = chosen
                .and_then(|provider| data.and_then(|data| data.get(provider)))
                .map(|usage| provider_percent(usage, settings.metric));
            match percent {
                Some(percent) => Content::Value { percent, style: settings.style },
                None => Content::Logo,
            }
        }
        TrayIconMode::Rundown => {
            let bars: Vec<Option<f64>> = enabled
                .iter()
                .map(|provider| {
                    data.and_then(|data| data.get(provider))
                        .map(|usage| provider_percent(usage, TrayIconMetric::Tightest))
                })
                .collect();
            if bars.iter().all(Option::is_none) {
                Content::Logo
            } else {
                Content::Rundown(bars)
            }
        }
    }
}

/// Paint `content` at `size` pixels. `light` draws in white (for a dark
/// taskbar); otherwise in near-black.
pub fn render(content: &Content, size: usize, light: bool) -> Render {
    let mut canvas = Canvas::new(size.max(8));
    match content {
        Content::Logo => paint_logo(&mut canvas),
        Content::Value { percent, style } => match style {
            TrayIconStyle::Number => paint_number(&mut canvas, *percent),
            TrayIconStyle::Bar => paint_bar(&mut canvas, *percent),
            TrayIconStyle::Ring => paint_ring(&mut canvas, *percent, true),
        },
        Content::Rundown(bars) => paint_rundown(&mut canvas, bars),
    }
    let tone: u8 = if light { 255 } else { 16 };
    let rgba = canvas
        .coverage
        .iter()
        .flat_map(|&cov| [tone, tone, tone, (cov.clamp(0.0, 1.0) * 255.0).round() as u8])
        .collect();
    Render { size: canvas.size, rgba }
}

// ---------------------------------------------------------------------------
// Layouts
// ---------------------------------------------------------------------------

/// The gauge the app icon is: a three-quarter ring with a sweep, open at the
/// bottom, and a hub.
fn paint_logo(canvas: &mut Canvas) {
    let n = canvas.size as f32;
    let (cx, cy) = (n / 2.0, n / 2.0);
    let (r_out, r_in) = (n * 0.46, n * 0.30);
    canvas.ring_arc(cx, cy, r_in, r_out, 225.0, 270.0, 0.38);
    canvas.ring_arc(cx, cy, r_in, r_out, 225.0, 170.0, 1.0);
    canvas.disc(cx, cy, n * 0.09, 1.0);
}

fn paint_ring(canvas: &mut Canvas, percent: f64, digits_when_room: bool) {
    let n = canvas.size as f32;
    let (cx, cy) = (n / 2.0, n / 2.0);
    let (r_out, r_in) = (n * 0.47, n * 0.33);
    let fraction = (percent / 100.0).clamp(0.0, 1.0) as f32;
    canvas.ring_arc(cx, cy, r_in, r_out, 225.0, 270.0, 0.30);
    if fraction > 0.0 {
        canvas.ring_arc(cx, cy, r_in, r_out, 225.0, 270.0 * fraction, 1.0);
    }
    // Digits inside once there is room for them to be read.
    if digits_when_room && canvas.size >= 32 {
        let text = digits_for(percent);
        let inner = 2.0 * r_in * 0.72;
        let scale = fit_scale(inner, inner, text.len());
        canvas.text(&text, cx, cy, scale, 1.0);
    }
}

fn paint_bar(canvas: &mut Canvas, percent: f64) {
    let n = canvas.size as f32;
    let (x0, x1) = (n * 0.06, n * 0.94);
    let (y0, y1) = (n * 0.34, n * 0.66);
    let fraction = (percent / 100.0).clamp(0.0, 1.0) as f32;
    // Track, then the fill from the left.
    canvas.rect(x0, y0, x1, y1, 0.28);
    let edge = (n * 0.06).max(1.0);
    canvas.rect(x0, y0, x1, y0 + edge, 0.9);
    canvas.rect(x0, y1 - edge, x1, y1, 0.9);
    canvas.rect(x0, y0, x0 + edge, y1, 0.9);
    canvas.rect(x1 - edge, y0, x1, y1, 0.9);
    if fraction > 0.0 {
        canvas.rect(x0, y0, x0 + (x1 - x0) * fraction, y1, 1.0);
    }
}

fn paint_number(canvas: &mut Canvas, percent: f64) {
    let n = canvas.size as f32;
    let text = digits_for(percent);
    let scale = fit_scale(n * 0.92, n * 0.80, text.len());
    canvas.text(&text, n / 2.0, n / 2.0, scale, 1.0);
}

/// One thin bar per provider, filling from the bottom; a provider with
/// nothing current is an outline.
fn paint_rundown(canvas: &mut Canvas, bars: &[Option<f64>]) {
    if bars.is_empty() {
        paint_logo(canvas);
        return;
    }
    let n = canvas.size as f32;
    let count = bars.len() as f32;
    let (top, bottom) = (n * 0.10, n * 0.92);
    let span = n * 0.90;
    let left = (n - span) / 2.0;
    let slot = span / count;
    let width = (slot * 0.62).max(1.0);
    for (index, bar) in bars.iter().enumerate() {
        let x0 = left + slot * index as f32 + (slot - width) / 2.0;
        let x1 = x0 + width;
        match bar {
            Some(percent) => {
                canvas.rect(x0, top, x1, bottom, 0.28);
                let fraction = (percent / 100.0).clamp(0.0, 1.0) as f32;
                let fill_top = bottom - (bottom - top) * fraction;
                if fraction > 0.0 {
                    canvas.rect(x0, fill_top.max(top), x1, bottom, 1.0);
                }
            }
            None => canvas.rect(x0, top, x1, bottom, 0.16),
        }
    }
}

/// "42", "7", "100" -- whole percent, never wider than three digits.
fn digits_for(percent: f64) -> String {
    let value = percent.clamp(0.0, 999.0).round() as u32;
    value.to_string()
}

/// The glyph scale (pixels per font cell) so `digits` digits with one-cell
/// gaps fit in `width` by `height` pixels. A lone digit is sized like one
/// digit of a pair rather than blown up to the full width.
fn fit_scale(width: f32, height: f32, digits: usize) -> f32 {
    let cells = (4 * digits.max(2)).saturating_sub(1) as f32;
    (width / cells).min(height / 5.0).max(0.5)
}

// ---------------------------------------------------------------------------
// Canvas
// ---------------------------------------------------------------------------

/// A square of coverage values, painted with anti-aliased shapes.
struct Canvas {
    size: usize,
    coverage: Vec<f32>,
}

/// Samples per pixel edge for anti-aliasing.
const SUPERSAMPLE: usize = 4;

impl Canvas {
    fn new(size: usize) -> Self {
        Self { size, coverage: vec![0.0; size * size] }
    }

    /// Paint everything `inside` says is covered, at `alpha`, over what is there.
    fn paint(&mut self, alpha: f32, inside: impl Fn(f32, f32) -> bool) {
        let step = 1.0 / SUPERSAMPLE as f32;
        for y in 0..self.size {
            for x in 0..self.size {
                let mut hits = 0;
                for sy in 0..SUPERSAMPLE {
                    for sx in 0..SUPERSAMPLE {
                        let px = x as f32 + (sx as f32 + 0.5) * step;
                        let py = y as f32 + (sy as f32 + 0.5) * step;
                        if inside(px, py) {
                            hits += 1;
                        }
                    }
                }
                if hits > 0 {
                    let a = alpha * hits as f32 / (SUPERSAMPLE * SUPERSAMPLE) as f32;
                    let cell = &mut self.coverage[y * self.size + x];
                    *cell = a + *cell * (1.0 - a);
                }
            }
        }
    }

    fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, alpha: f32) {
        self.paint(alpha, move |x, y| x >= x0 && x < x1 && y >= y0 && y < y1);
    }

    fn disc(&mut self, cx: f32, cy: f32, r: f32, alpha: f32) {
        self.paint(alpha, move |x, y| (x - cx).powi(2) + (y - cy).powi(2) <= r * r);
    }

    /// The part of a ring between `start` degrees (clockwise from twelve)
    /// and `start + span`.
    #[allow(clippy::too_many_arguments)]
    fn ring_arc(&mut self, cx: f32, cy: f32, r_in: f32, r_out: f32, start: f32, span: f32, alpha: f32) {
        self.paint(alpha, move |x, y| {
            let (dx, dy) = (x - cx, y - cy);
            let d2 = dx * dx + dy * dy;
            if d2 < r_in * r_in || d2 > r_out * r_out {
                return false;
            }
            // Clockwise from twelve o'clock, in degrees.
            let angle = (dx.atan2(-dy).to_degrees() + 360.0) % 360.0;
            let from = ((angle - start) % 360.0 + 360.0) % 360.0;
            from <= span
        });
    }

    /// Digits centred on (cx, cy), `scale` pixels per font cell. The whole
    /// string is one shape, so cells that share an edge do not leave a
    /// seam where two anti-aliased rectangles would only half-cover it.
    fn text(&mut self, text: &str, cx: f32, cy: f32, scale: f32, alpha: f32) {
        let glyphs: Vec<&'static [u8; 5]> = text.chars().filter_map(glyph).collect();
        if glyphs.is_empty() {
            return;
        }
        let width = (4 * glyphs.len() - 1) as f32 * scale;
        let height = 5.0 * scale;
        let left = cx - width / 2.0;
        let top = cy - height / 2.0;
        self.paint(alpha, move |x, y| {
            let (fx, fy) = ((x - left) / scale, (y - top) / scale);
            if fx < 0.0 || !(0.0..5.0).contains(&fy) {
                return false;
            }
            let (glyph_index, col) = ((fx / 4.0) as usize, (fx % 4.0) as usize);
            if col >= 3 {
                return false;
            }
            glyphs
                .get(glyph_index)
                .is_some_and(|glyph| glyph[fy as usize] & (0b100 >> col) != 0)
        });
    }
}

/// A three-by-five digit font: each row is three bits, top row first.
fn glyph(c: char) -> Option<&'static [u8; 5]> {
    const DIGITS: [[u8; 5]; 10] = [
        [0b111, 0b101, 0b101, 0b101, 0b111],
        [0b010, 0b110, 0b010, 0b010, 0b111],
        [0b111, 0b001, 0b111, 0b100, 0b111],
        [0b111, 0b001, 0b111, 0b001, 0b111],
        [0b101, 0b101, 0b111, 0b001, 0b001],
        [0b111, 0b100, 0b111, 0b001, 0b111],
        [0b111, 0b100, 0b111, 0b101, 0b111],
        [0b111, 0b001, 0b001, 0b001, 0b001],
        [0b111, 0b101, 0b111, 0b101, 0b111],
        [0b111, 0b101, 0b111, 0b001, 0b111],
    ];
    c.to_digit(10).map(|d| &DIGITS[d as usize])
}

/// Every mode and style at the sizes the taskbar uses, written as PNGs for
/// a look before shipping. `--render-tray-previews <dir>`.
pub fn write_previews(dir: &std::path::Path) -> Result<usize, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut written = 0;
    let contents: Vec<(&str, Content)> = vec![
        ("logo", Content::Logo),
        ("number-7", Content::Value { percent: 7.0, style: TrayIconStyle::Number }),
        ("number-42", Content::Value { percent: 42.0, style: TrayIconStyle::Number }),
        ("number-100", Content::Value { percent: 100.0, style: TrayIconStyle::Number }),
        ("bar-25", Content::Value { percent: 25.0, style: TrayIconStyle::Bar }),
        ("bar-80", Content::Value { percent: 80.0, style: TrayIconStyle::Bar }),
        ("ring-33", Content::Value { percent: 33.0, style: TrayIconStyle::Ring }),
        ("ring-91", Content::Value { percent: 91.0, style: TrayIconStyle::Ring }),
        ("rundown-5", Content::Rundown(vec![Some(21.0), Some(64.0), Some(4.0), None, Some(88.0)])),
        ("rundown-8", Content::Rundown(vec![Some(21.0), Some(64.0), Some(4.0), None, Some(88.0), Some(50.0), None, Some(97.0)])),
    ];
    for (name, content) in &contents {
        for size in [16usize, 20, 24, 32, 64] {
            for (tone, light) in [("dark-taskbar", true), ("light-taskbar", false)] {
                let render = render(content, size, light);
                // Composite onto the taskbar colour so the PNG shows what a
                // person sees, not alpha on a checkerboard.
                let bg: [u8; 3] = if light { [32, 32, 32] } else { [243, 243, 243] };
                let mut rgb = Vec::with_capacity(size * size * 3);
                for px in render.rgba.chunks_exact(4) {
                    let a = f32::from(px[3]) / 255.0;
                    for channel in 0..3 {
                        rgb.push((f32::from(px[channel]) * a + f32::from(bg[channel]) * (1.0 - a)).round() as u8);
                    }
                }
                let image = image::RgbImage::from_raw(size as u32, size as u32, rgb).ok_or("bad buffer")?;
                image
                    .save(dir.join(format!("{name}-{size}-{tone}.png")))
                    .map_err(|e| e.to_string())?;
                written += 1;
            }
        }
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::UsageSection;

    fn usage(session: f64, weekly: f64) -> UsageData {
        UsageData {
            session: UsageSection { percentage: session, resets_at: None },
            weekly: UsageSection { percentage: weekly, resets_at: None },
            ..Default::default()
        }
    }

    fn lit(render: &Render) -> usize {
        render.rgba.chunks_exact(4).filter(|px| px[3] > 64).count()
    }

    /// Solid pixels only: the fill, not the faint track behind it.
    fn solid(render: &Render) -> usize {
        render.rgba.chunks_exact(4).filter(|px| px[3] > 200).count()
    }

    fn lit_columns(render: &Render) -> (usize, usize) {
        let mut min = usize::MAX;
        let mut max = 0;
        for (index, px) in render.rgba.chunks_exact(4).enumerate() {
            if px[3] > 64 {
                let x = index % render.size;
                min = min.min(x);
                max = max.max(x);
            }
        }
        (min, max)
    }

    #[test]
    fn the_tightest_window_is_the_provider_value() {
        let mut data = usage(12.0, 40.0);
        assert_eq!(provider_percent(&data, TrayIconMetric::Tightest), 40.0);
        assert_eq!(provider_percent(&data, TrayIconMetric::Session), 12.0);
        data.monthly = Some(UsageSection { percentage: 77.0, resets_at: None });
        assert_eq!(provider_percent(&data, TrayIconMetric::Tightest), 77.0);
    }

    #[test]
    fn content_follows_the_mode_and_falls_back_to_the_logo() {
        let mut data = AppUsageData::default();
        data.insert(ProviderId::Claude, usage(30.0, 55.0));
        data.insert(ProviderId::Grok, usage(0.0, 80.0));
        let enabled = ProviderSet::from_enabled([ProviderId::Claude, ProviderId::Grok, ProviderId::Codex]);
        let settings = TrayIconSettings { mode: TrayIconMode::Tightest, ..Default::default() };
        assert_eq!(content(&settings, Some(&data), enabled), Content::Value { percent: 80.0, style: TrayIconStyle::Ring });
        let settings = TrayIconSettings { mode: TrayIconMode::Provider, provider: Some("claude".into()), metric: TrayIconMetric::Session, style: TrayIconStyle::Number, ..Default::default() };
        assert_eq!(content(&settings, Some(&data), enabled), Content::Value { percent: 30.0, style: TrayIconStyle::Number });
        let settings = TrayIconSettings { mode: TrayIconMode::Rundown, ..Default::default() };
        assert_eq!(content(&settings, Some(&data), enabled), Content::Rundown(vec![Some(55.0), None, Some(80.0)]));
        assert_eq!(content(&settings, None, enabled), Content::Logo);
        let settings = TrayIconSettings { mode: TrayIconMode::Provider, provider: Some("devin".into()), ..Default::default() };
        assert_eq!(content(&settings, Some(&data), enabled), Content::Logo);
    }

    #[test]
    fn every_layout_paints_inside_the_square_at_every_size() {
        for size in [16usize, 20, 24, 32, 64] {
            for content in [
                Content::Logo,
                Content::Value { percent: 100.0, style: TrayIconStyle::Number },
                Content::Value { percent: 50.0, style: TrayIconStyle::Bar },
                Content::Value { percent: 50.0, style: TrayIconStyle::Ring },
                Content::Rundown(vec![Some(10.0), None, Some(90.0), Some(50.0), Some(5.0), Some(70.0), Some(30.0), Some(99.0)]),
            ] {
                let render = super::render(&content, size, true);
                assert_eq!(render.rgba.len(), size * size * 4);
                assert!(lit(&render) > 0, "{content:?} at {size} painted nothing");
                let (min, max) = lit_columns(&render);
                assert!(min < size && max < size, "{content:?} at {size} spilled");
            }
        }
    }

    #[test]
    fn bars_and_rings_fill_with_the_percentage() {
        for style in [TrayIconStyle::Bar, TrayIconStyle::Ring] {
            let low = solid(&super::render(&Content::Value { percent: 20.0, style }, 32, true));
            let high = solid(&super::render(&Content::Value { percent: 90.0, style }, 32, true));
            assert!(high > low, "{style:?}: {high} vs {low}");
        }
        let empty = super::render(&Content::Rundown(vec![Some(0.0), Some(0.0)]), 32, true);
        let full = super::render(&Content::Rundown(vec![Some(100.0), Some(100.0)]), 32, true);
        assert!(solid(&full) > solid(&empty));
    }

    #[test]
    fn three_digits_fit_at_sixteen_pixels() {
        let render = super::render(&Content::Value { percent: 100.0, style: TrayIconStyle::Number }, 16, true);
        let (min, max) = lit_columns(&render);
        assert!(max - min >= 10, "three digits should span most of the width: {min}..{max}");
    }

    #[test]
    fn tone_picks_the_foreground_colour() {
        let light = super::render(&Content::Logo, 16, true);
        let dark = super::render(&Content::Logo, 16, false);
        let sample = |r: &Render| r.rgba.chunks_exact(4).find(|px| px[3] > 200).map(|px| px[0]);
        assert_eq!(sample(&light), Some(255));
        assert_eq!(sample(&dark), Some(16));
        let word = light.bgra_premultiplied()[light.rgba.chunks_exact(4).position(|px| px[3] == 255).unwrap()];
        assert_eq!(word, 0xFFFF_FFFF);
    }
}
