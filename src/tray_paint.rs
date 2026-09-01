//! The tray icon, painted.
//!
//! One square, monotone, drawn from the current readings at whatever size
//! the taskbar wants. The renderer is pure -- a coverage canvas with a
//! handful of shapes and a five-row digit font -- so the same pixels serve
//! the shell (as a 32-bit icon), the settings preview (as a texture) and
//! the review dump (as PNGs), and the layouts can be tested by counting
//! pixels.
//!
//! What an icon shows is a setting: the static logo; the tightest limit
//! across the fleet; one provider's chosen window; or the whole fleet as a
//! row of bars. A value is used or left, and drawn as a number, a bar, a
//! column or a ring; a ring can carry digits or the provider's mark inside.
//! There can be several icons, each with its own settings.

use crate::app_settings::{
    TrayIconMark, TrayIconMeasure, TrayIconMetric, TrayIconMode, TrayIconSettings, TrayIconStyle,
};
use crate::models::{AppUsageData, UsageData};
use crate::providers::{ProviderId, ProviderSet};

/// What to draw this time.
#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    Logo,
    /// One percentage, 0 to 100 (more is clamped when drawn, shown as
    /// digits). `label` is the icon's short name, drawn by the Letters
    /// style and by a `Mark::Label`.
    Value { percent: f64, style: TrayIconStyle, mark: Mark, label: String },
    /// One entry per enabled provider, in provider order; `None` for one
    /// with nothing current. `rows` lays them out as horizontal rows
    /// instead of columns.
    Rundown { bars: Vec<Option<f64>>, rows: bool },
}

/// The text a value icon carries beside its shape: inside a ring, in a
/// band above a bar, column or number, above the letters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// The whole percent.
    Digits,
    /// The icon's label.
    Label,
    None,
}

impl Content {
    /// A value with the default digits mark, for tests and previews.
    pub fn value(percent: f64, style: TrayIconStyle) -> Self {
        Content::Value { percent, style, mark: Mark::Digits, label: String::new() }
    }
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
/// window, or one particular limit. A limit the provider does not report
/// (no monthly window, a per-model cap that has gone) falls back to the
/// tightest, so the icon keeps saying something rather than going blank.
pub fn provider_percent(usage: &UsageData, metric: &TrayIconMetric) -> f64 {
    reported_percent(usage, metric).unwrap_or_else(|| tightest_percent(usage))
}

/// A provider's tightest window: the highest reading across everything it
/// reports (credits aside -- a balance is not a cap).
pub fn tightest_percent(usage: &UsageData) -> f64 {
    [usage.session.percentage, usage.weekly.percentage]
        .into_iter()
        .chain(usage.monthly.as_ref().map(|monthly| monthly.percentage))
        .chain(usage.scoped.iter().map(|scoped| scoped.section.percentage))
        .fold(0.0, f64::max)
}

/// The percentage under a metric only if the provider reports that limit;
/// `None` otherwise, so a fleet view can leave the provider out rather than
/// compare a window it does not have. A window with neither a figure nor
/// a reset time is one the provider does not bill, not one at zero.
pub fn reported_percent(usage: &UsageData, metric: &TrayIconMetric) -> Option<f64> {
    let real = |section: &crate::models::UsageSection| section.percentage > 0.0 || section.resets_at.is_some();
    match metric {
        TrayIconMetric::Tightest => Some(tightest_percent(usage)),
        TrayIconMetric::Session => real(&usage.session).then_some(usage.session.percentage),
        TrayIconMetric::Weekly => real(&usage.weekly).then_some(usage.weekly.percentage),
        TrayIconMetric::Monthly => usage.monthly.as_ref().map(|monthly| monthly.percentage),
        TrayIconMetric::Credits => usage.credits.as_ref().map(|credits| credits.percentage),
        TrayIconMetric::Scoped(label) => usage.scoped.iter().find(|scoped| &scoped.label == label).map(|scoped| scoped.section.percentage),
    }
}

/// The limits a provider offers an icon, as the icon names them: the
/// tightest first, then each window it reports, in the dashboard's order.
/// Only windows with a figure or a reset time are real; a flat zero is a
/// window the provider does not bill.
pub fn provider_windows(usage: &UsageData) -> Vec<(TrayIconMetric, String)> {
    let mut out = vec![(TrayIconMetric::Tightest, "Tightest window".to_string())];
    if usage.session.percentage > 0.0 || usage.session.resets_at.is_some() {
        out.push((TrayIconMetric::Session, "Session".to_string()));
    }
    if usage.weekly.percentage > 0.0 || usage.weekly.resets_at.is_some() {
        let title = match &usage.weekly_label {
            Some(label) => format!("Weekly ({label})"),
            None => "Weekly".to_string(),
        };
        out.push((TrayIconMetric::Weekly, title));
    }
    for scoped in &usage.scoped {
        let window = match scoped.window {
            crate::models::LimitWindow::Session => "session",
            crate::models::LimitWindow::Weekly => "weekly",
            crate::models::LimitWindow::Monthly => "monthly",
        };
        out.push((TrayIconMetric::Scoped(scoped.label.clone()), format!("{} · {window}", scoped.label)));
    }
    if usage.monthly.is_some() {
        out.push((TrayIconMetric::Monthly, "Monthly".to_string()));
    }
    if usage.credits.is_some() {
        out.push((TrayIconMetric::Credits, "Credits".to_string()));
    }
    out
}

/// What a metric is called: the provider's own title when it reports that
/// limit, otherwise the generic name.
pub fn metric_name(metric: &TrayIconMetric, usage: Option<&UsageData>) -> String {
    if let Some(usage) = usage {
        if let Some((_, title)) = provider_windows(usage).into_iter().find(|(candidate, _)| candidate == metric) {
            return title;
        }
    }
    match metric {
        TrayIconMetric::Tightest => "Tightest window".to_string(),
        TrayIconMetric::Session => "Session".to_string(),
        TrayIconMetric::Weekly => "Weekly".to_string(),
        TrayIconMetric::Monthly => "Monthly".to_string(),
        TrayIconMetric::Credits => "Credits".to_string(),
        TrayIconMetric::Scoped(label) => label.clone(),
    }
}

/// The provider an icon reads, and its used percentage under the icon's
/// window: for `Tightest` the provider with the highest reading, for
/// `Provider` the chosen one (or the first enabled when none is chosen).
/// `None` when nothing it needs is reporting, or for the other modes.
pub fn shown_provider(settings: &TrayIconSettings, data: Option<&AppUsageData>, enabled: ProviderSet) -> Option<(ProviderId, f64)> {
    let data = data?;
    match settings.mode {
        TrayIconMode::Tightest => enabled
            .iter()
            .filter_map(|provider| Some((provider, reported_percent(data.get(provider)?, &settings.metric)?)))
            .fold(None, |best: Option<(ProviderId, f64)>, candidate| match best {
                Some(best) if best.1 >= candidate.1 => Some(best),
                _ => Some(candidate),
            }),
        TrayIconMode::Provider => {
            let chosen = settings
                .provider
                .as_deref()
                .and_then(ProviderId::from_key)
                .or_else(|| enabled.iter().next())?;
            Some((chosen, provider_percent(data.get(chosen)?, &settings.metric)))
        }
        TrayIconMode::Logo | TrayIconMode::Rundown => None,
    }
}

/// The highest used percentage among what the icon shows, for the alert
/// tint: the value itself, or the worst bar of a rundown. `None` for the
/// logo or when nothing is reporting.
pub fn shown_used_percent(settings: &TrayIconSettings, data: Option<&AppUsageData>, enabled: ProviderSet) -> Option<f64> {
    match settings.mode {
        TrayIconMode::Logo => None,
        TrayIconMode::Tightest | TrayIconMode::Provider => shown_provider(settings, data, enabled).map(|(_, used)| used),
        TrayIconMode::Rundown => rundown_bars(settings, data, enabled).into_iter().flatten().fold(None, |worst: Option<f64>, value| {
            Some(worst.map_or(value, |worst| worst.max(value)))
        }),
    }
}

fn rundown_bars(settings: &TrayIconSettings, data: Option<&AppUsageData>, enabled: ProviderSet) -> Vec<Option<f64>> {
    enabled
        .iter()
        .map(|provider| {
            data.and_then(|data| data.get(provider))
                .and_then(|usage| reported_percent(usage, &settings.metric))
        })
        .collect()
}

/// Decide what the icon shows from the settings and the latest readings.
/// Anything that needs a reading and has none falls back to the logo, so the
/// icon is never a blank shape.
pub fn content(settings: &TrayIconSettings, data: Option<&AppUsageData>, enabled: ProviderSet) -> Content {
    match settings.mode {
        TrayIconMode::Logo => Content::Logo,
        TrayIconMode::Tightest | TrayIconMode::Provider => match shown_provider(settings, data, enabled) {
            Some((provider, used)) => {
                let percent = match settings.measure {
                    TrayIconMeasure::Used => used,
                    TrayIconMeasure::Remaining => (100.0 - used).max(0.0),
                };
                // The label rides along for the Letters style and for a
                // label mark. A number never carries a second percent, and
                // the letters never carry themselves again.
                let label = settings.label_for(Some(provider));
                let mark = match settings.effective_mark() {
                    TrayIconMark::Digits => Mark::Digits,
                    TrayIconMark::Initials => Mark::Label,
                    TrayIconMark::None => Mark::None,
                };
                Content::Value { percent, style: settings.style, mark, label }
            }
            None => Content::Logo,
        },
        TrayIconMode::Rundown => {
            let bars = rundown_bars(settings, data, enabled);
            if bars.iter().all(Option::is_none) {
                Content::Logo
            } else {
                let bars = match settings.measure {
                    TrayIconMeasure::Used => bars,
                    TrayIconMeasure::Remaining => bars.into_iter().map(|bar| bar.map(|used| (100.0 - used).max(0.0))).collect(),
                };
                Content::Rundown { bars, rows: settings.style == TrayIconStyle::Bar }
            }
        }
    }
}

/// Paint `content` at `size` pixels. `light` draws in white (for a dark
/// taskbar); otherwise in near-black.
pub fn render(content: &Content, size: usize, light: bool) -> Render {
    let tone: u8 = if light { 255 } else { 16 };
    render_tinted(content, size, [tone, tone, tone])
}

/// Paint `content` at `size` pixels in one colour: the alert tint, or the
/// tone `render` picks.
pub fn render_tinted(content: &Content, size: usize, rgb: [u8; 3]) -> Render {
    let mut canvas = Canvas::new(size.max(8));
    match content {
        Content::Logo => paint_logo(&mut canvas),
        Content::Value { percent, style, mark, label } => {
            // A reading that is not a number is drawn as nothing used, not
            // as an empty shape with "0" beside it.
            let percent = if percent.is_finite() { *percent } else { 0.0 };
            let text = mark_text(*mark, percent, label);
            match style {
                TrayIconStyle::Ring => paint_ring(&mut canvas, percent, &text),
                TrayIconStyle::Letters => {
                    // Only a digits mark rides above the letters; the band
                    // choice follows the mark, never a lookalike label.
                    let band = if *mark == Mark::Digits { paint_caption(&mut canvas, &text) } else { (0.0, canvas.size as f32) };
                    paint_letters(&mut canvas, percent, label, band)
                }
                TrayIconStyle::Number | TrayIconStyle::Bar | TrayIconStyle::Column => {
                    // A caption above, when it can be read; the shape takes
                    // the rest of the square.
                    let band = paint_caption(&mut canvas, &text);
                    match style {
                        TrayIconStyle::Number => paint_number(&mut canvas, percent, band),
                        TrayIconStyle::Bar => paint_bar(&mut canvas, percent, band),
                        _ => paint_column(&mut canvas, percent, band),
                    }
                }
            }
        }
        Content::Rundown { bars, rows } => paint_rundown(&mut canvas, bars, *rows),
    }
    let rgba = canvas
        .coverage
        .iter()
        .flat_map(|&cov| [rgb[0], rgb[1], rgb[2], (cov.clamp(0.0, 1.0) * 255.0).round() as u8])
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

/// What a mark says, ready to draw; empty for none.
fn mark_text(mark: Mark, percent: f64, label: &str) -> String {
    match mark {
        Mark::Digits => digits_for(percent),
        Mark::Label => label.to_string(),
        Mark::None => String::new(),
    }
}

fn paint_ring(canvas: &mut Canvas, percent: f64, text: &str) {
    let n = canvas.size as f32;
    let (cx, cy) = (n / 2.0, n / 2.0);
    // Thin enough that two letters read inside it at sixteen pixels.
    let (r_out, r_in) = (n * 0.47, n * 0.35);
    let fraction = (percent / 100.0).clamp(0.0, 1.0) as f32;
    canvas.ring_arc(cx, cy, r_in, r_out, 225.0, 270.0, 0.30);
    if fraction > 0.0 {
        canvas.ring_arc(cx, cy, r_in, r_out, 225.0, 270.0 * fraction, 1.0);
    }
    // Inside, when a font cell stays above a pixel: two characters read
    // from sixteen pixels, three digits need more ring than that. The
    // text block's corners must stay inside the ring's inner edge, so the
    // scale is capped by the block's own diagonal.
    if !text.is_empty() {
        let count = text.chars().count();
        let cells = (4 * count.max(2) - 1) as f32;
        let corner = ((cells / 2.0).powi(2) + 2.5_f32.powi(2)).sqrt();
        let inner = 2.0 * r_in * 0.85;
        let scale = fit_scale(inner, inner * 0.72, count).min(r_in / corner);
        if scale >= MARK_MIN_SCALE {
            canvas.text(text, cx, cy, scale, 1.0);
        }
    }
}

/// Pixels per font cell below which text is left out rather than drawn as
/// mush. At 1.15 a two-character mark reads at sixteen pixels.
const MARK_MIN_SCALE: f32 = 1.15;

/// The caption a bar, column or number carries: the mark's text in a band
/// across the top, when a font cell would stay above a pixel -- which two
/// characters manage at sixteen pixels. Returns the vertical band left
/// for the shape.
fn paint_caption(canvas: &mut Canvas, text: &str) -> (f32, f32) {
    let n = canvas.size as f32;
    let whole = (0.0, n);
    if text.is_empty() {
        return whole;
    }
    let scale = fit_scale(n * 0.92, n * 0.38, text.chars().count());
    if scale < MARK_MIN_SCALE {
        return whole;
    }
    let height = 5.0 * scale;
    let band_top = height + (n * 0.06).max(1.0);
    canvas.text(text, n / 2.0, band_top / 2.0, scale, 1.0);
    (band_top, n)
}

/// A horizontal bar across the middle of `band`, filling from the left.
fn paint_bar(canvas: &mut Canvas, percent: f64, band: (f32, f32)) {
    let n = canvas.size as f32;
    let (x0, x1) = (n * 0.06, n * 0.94);
    let mid = (band.0 + band.1) / 2.0;
    let half = ((band.1 - band.0) * 0.17).clamp(1.5, n * 0.16);
    let (y0, y1) = (mid - half, mid + half);
    let fraction = (percent / 100.0).clamp(0.0, 1.0) as f32;
    // Track, then the fill from the left.
    canvas.rect(x0, y0, x1, y1, 0.28);
    let edge = (n * 0.06).max(1.0).min(half);
    canvas.rect(x0, y0, x1, y0 + edge, 0.9);
    canvas.rect(x0, y1 - edge, x1, y1, 0.9);
    canvas.rect(x0, y0, x0 + edge, y1, 0.9);
    canvas.rect(x1 - edge, y0, x1, y1, 0.9);
    if fraction > 0.0 {
        canvas.rect(x0, y0, x0 + (x1 - x0) * fraction, y1, 1.0);
    }
}

/// The bar stood on end within `band`: fills from the bottom.
fn paint_column(canvas: &mut Canvas, percent: f64, band: (f32, f32)) {
    let n = canvas.size as f32;
    let (x0, x1) = (n * 0.32, n * 0.68);
    let (y0, y1) = (band.0 + n * 0.06, band.1 - n * 0.06);
    let fraction = (percent / 100.0).clamp(0.0, 1.0) as f32;
    canvas.rect(x0, y0, x1, y1, 0.28);
    let edge = (n * 0.06).max(1.0);
    canvas.rect(x0, y0, x1, y0 + edge, 0.9);
    canvas.rect(x0, y1 - edge, x1, y1, 0.9);
    canvas.rect(x0, y0, x0 + edge, y1, 0.9);
    canvas.rect(x1 - edge, y0, x1, y1, 0.9);
    if fraction > 0.0 {
        canvas.rect(x0, y1 - (y1 - y0) * fraction, x1, y1, 1.0);
    }
}

/// The whole percent, as large as `band` allows.
fn paint_number(canvas: &mut Canvas, percent: f64, band: (f32, f32)) {
    let n = canvas.size as f32;
    let text = digits_for(percent);
    let height = band.1 - band.0;
    let scale = fit_scale(n * 0.92, height * 0.80, text.len());
    canvas.text(&text, n / 2.0, (band.0 + band.1) / 2.0, scale, 1.0);
}

/// The label, as large as its band allows, filling from the bottom with
/// the percentage: dim letters, solid up to the line. A digits mark rides
/// above as a caption (painted by the caller), so the letters can also
/// say how much.
fn paint_letters(canvas: &mut Canvas, percent: f64, label: &str, band: (f32, f32)) {
    let n = canvas.size as f32;
    let letters = if label.is_empty() { digits_for(percent) } else { label.to_string() };
    let height = band.1 - band.0;
    let scale = fit_scale(n * 0.92, height * 0.80, letters.chars().count());
    let (cx, cy) = (n / 2.0, (band.0 + band.1) / 2.0);
    canvas.text(&letters, cx, cy, scale, 0.30);
    let fraction = (percent / 100.0).clamp(0.0, 1.0) as f32;
    if fraction > 0.0 {
        let top = cy + 2.5 * scale;
        let line = top - 5.0 * scale * fraction;
        canvas.text_below(&letters, cx, cy, scale, 1.0, line);
    }
}

/// One thin bar per provider -- columns filling from the bottom, or rows
/// filling from the left; a provider with nothing current is an outline.
fn paint_rundown(canvas: &mut Canvas, bars: &[Option<f64>], rows: bool) {
    if bars.is_empty() {
        paint_logo(canvas);
        return;
    }
    let n = canvas.size as f32;
    let count = bars.len() as f32;
    let (near, far) = if rows { (n * 0.08, n * 0.92) } else { (n * 0.10, n * 0.92) };
    let span = n * 0.90;
    let first = (n - span) / 2.0;
    let slot = span / count;
    let width = (slot * 0.62).max(1.0);
    for (index, bar) in bars.iter().enumerate() {
        let a0 = first + slot * index as f32 + (slot - width) / 2.0;
        let a1 = a0 + width;
        // (a0, a1) is the bar's extent across; (near, far) along.
        let fill = |canvas: &mut Canvas, from: f32, to: f32, alpha: f32| {
            if rows {
                canvas.rect(from, a0, to, a1, alpha);
            } else {
                canvas.rect(a0, from, a1, to, alpha);
            }
        };
        match bar {
            Some(percent) => {
                fill(canvas, near, far, 0.28);
                let fraction = (percent / 100.0).clamp(0.0, 1.0) as f32;
                if fraction > 0.0 {
                    let length = (far - near) * fraction;
                    if rows {
                        fill(canvas, near, (near + length).min(far), 1.0);
                    } else {
                        fill(canvas, (far - length).max(near), far, 1.0);
                    }
                }
            }
            None => fill(canvas, near, far, 0.16),
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
        self.text_below(text, cx, cy, scale, alpha, f32::NEG_INFINITY);
    }

    /// `text`, but only the part at or below `y_from`: how letters fill.
    fn text_below(&mut self, text: &str, cx: f32, cy: f32, scale: f32, alpha: f32, y_from: f32) {
        let glyphs: Vec<&'static [u8; 5]> = text.chars().filter_map(glyph).collect();
        if glyphs.is_empty() {
            return;
        }
        let width = (4 * glyphs.len() - 1) as f32 * scale;
        let height = 5.0 * scale;
        let left = cx - width / 2.0;
        let top = cy - height / 2.0;
        self.paint(alpha, move |x, y| {
            if y < y_from {
                return false;
            }
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

/// A three-by-five font, digits and capitals: each row is three bits, top
/// row first.
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
    const LETTERS: [[u8; 5]; 26] = [
        [0b010, 0b101, 0b111, 0b101, 0b101], // A
        [0b110, 0b101, 0b110, 0b101, 0b110], // B
        [0b011, 0b100, 0b100, 0b100, 0b011], // C
        [0b110, 0b101, 0b101, 0b101, 0b110], // D
        [0b111, 0b100, 0b110, 0b100, 0b111], // E
        [0b111, 0b100, 0b110, 0b100, 0b100], // F
        [0b011, 0b100, 0b101, 0b101, 0b011], // G
        [0b101, 0b101, 0b111, 0b101, 0b101], // H
        [0b111, 0b010, 0b010, 0b010, 0b111], // I
        [0b001, 0b001, 0b001, 0b101, 0b010], // J
        [0b101, 0b101, 0b110, 0b101, 0b101], // K
        [0b100, 0b100, 0b100, 0b100, 0b111], // L
        [0b101, 0b111, 0b111, 0b101, 0b101], // M
        [0b110, 0b101, 0b101, 0b101, 0b101], // N
        [0b010, 0b101, 0b101, 0b101, 0b010], // O
        [0b110, 0b101, 0b110, 0b100, 0b100], // P
        [0b010, 0b101, 0b101, 0b110, 0b011], // Q
        [0b110, 0b101, 0b110, 0b101, 0b101], // R
        [0b011, 0b100, 0b010, 0b001, 0b110], // S
        [0b111, 0b010, 0b010, 0b010, 0b010], // T
        [0b101, 0b101, 0b101, 0b101, 0b011], // U
        [0b101, 0b101, 0b101, 0b101, 0b010], // V
        [0b101, 0b101, 0b111, 0b111, 0b101], // W
        [0b101, 0b101, 0b010, 0b101, 0b101], // X
        [0b101, 0b101, 0b010, 0b010, 0b010], // Y
        [0b111, 0b001, 0b010, 0b100, 0b111], // Z
    ];
    if let Some(d) = c.to_digit(10) {
        return Some(&DIGITS[d as usize]);
    }
    c.is_ascii_uppercase().then(|| &LETTERS[(c as u8 - b'A') as usize])
}

/// The application icon: the gauge in white on a black rounded plate, so
/// it reads on any background the desktop, taskbar or Store puts it on.
pub fn render_app_icon(size: usize) -> Render {
    render_app_icon_with(size, 0.30, 0.195, 0.06)
}

/// The set of gauge weights, at the sizes worth judging: pick one, then
/// its numbers become `render_app_icon`'s.
pub fn write_app_icon_weights(dir: &std::path::Path) -> Result<usize, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut written = 0;
    for (name, r_out, r_in, hub) in [
        ("thin", 0.30, 0.215, 0.048),
        ("regular", 0.30, 0.195, 0.060),
        ("bold", 0.315, 0.175, 0.075),
        ("heavy", 0.33, 0.155, 0.092),
    ] {
        for size in [24usize, 48, 256] {
            let render = render_app_icon_with(size, r_out, r_in, hub);
            let image = image::RgbaImage::from_raw(size as u32, size as u32, render.rgba.clone()).ok_or("bad buffer")?;
            image.save(dir.join(format!("{name}-{size}.png"))).map_err(|e| e.to_string())?;
            written += 1;
        }
    }
    Ok(written)
}

fn render_app_icon_with(size: usize, r_out_factor: f32, r_in_factor: f32, hub_factor: f32) -> Render {
    let mut canvas = Canvas::new(size.max(8));
    let n = canvas.size as f32;
    let radius = n * 0.22;
    // Plate: a rounded square.
    canvas.paint(1.0, move |x, y| {
        let (cx, cy) = ((x - n / 2.0).abs(), (y - n / 2.0).abs());
        let half = n / 2.0 - 0.5;
        let (qx, qy) = ((cx - (half - radius)).max(0.0), (cy - (half - radius)).max(0.0));
        cx <= half && cy <= half && qx * qx + qy * qy <= radius * radius
    });
    let plate: Vec<f32> = canvas.coverage.clone();
    // Glyph: the logo at 64% of the plate, painted as its own coverage so
    // the plate stays black underneath.
    let mut glyph = Canvas::new(canvas.size);
    let (cx, cy) = (n / 2.0, n / 2.0);
    let (r_out, r_in) = (n * r_out_factor, n * r_in_factor);
    glyph.ring_arc(cx, cy, r_in, r_out, 225.0, 270.0, 0.38);
    glyph.ring_arc(cx, cy, r_in, r_out, 225.0, 170.0, 1.0);
    glyph.disc(cx, cy, n * hub_factor, 1.0);
    let mut rgba = Vec::with_capacity(canvas.size * canvas.size * 4);
    for (plate_cov, glyph_cov) in plate.iter().zip(glyph.coverage.iter()) {
        // White glyph over black plate; alpha is the plate's.
        let white = glyph_cov.clamp(0.0, 1.0);
        let level = (white * 255.0).round() as u8;
        rgba.extend_from_slice(&[level, level, level, (plate_cov.clamp(0.0, 1.0) * 255.0).round() as u8]);
    }
    Render { size: canvas.size, rgba }
}

/// The app icon as PNGs at the sizes Windows and the Store use, plus an
/// `icon.ico` holding them (PNG-compressed entries) and an SVG of the same
/// geometry. `--render-app-icon <dir>`.
pub fn write_app_icon(dir: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let sizes = [16usize, 20, 24, 32, 48, 64, 128, 256];
    // The Store listing's 1:1 logos, PNG only -- an ICO tops out at 256.
    for size in [300usize, 1080] {
        let render = render_app_icon(size);
        let image = image::RgbaImage::from_raw(size as u32, size as u32, render.rgba.clone()).ok_or("bad buffer")?;
        image.save(dir.join(format!("{size}x{size}.png"))).map_err(|e| e.to_string())?;
    }
    let mut pngs: Vec<(usize, Vec<u8>)> = Vec::new();
    for size in sizes {
        let render = render_app_icon(size);
        let image = image::RgbaImage::from_raw(size as u32, size as u32, render.rgba.clone()).ok_or("bad buffer")?;
        image.save(dir.join(format!("{size}x{size}.png"))).map_err(|e| e.to_string())?;
        let mut png = std::io::Cursor::new(Vec::new());
        image.write_to(&mut png, image::ImageFormat::Png).map_err(|e| e.to_string())?;
        pngs.push((size, png.into_inner()));
    }
    // ICO: header, one directory entry per image, then the payloads. Sizes
    // up to 64 are classic 32-bit DIBs (every loader reads those); 128 and
    // 256 are PNG-compressed, the convention since Vista.
    let payloads: Vec<(usize, Vec<u8>)> = pngs
        .iter()
        .map(|(size, png)| (*size, if *size >= 128 { png.clone() } else { ico_dib(&render_app_icon(*size)) }))
        .collect();
    let mut ico: Vec<u8> = Vec::new();
    ico.extend_from_slice(&[0, 0, 1, 0]);
    ico.extend_from_slice(&(payloads.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * payloads.len();
    for (size, payload) in &payloads {
        let dimension = if *size >= 256 { 0u8 } else { *size as u8 };
        ico.extend_from_slice(&[dimension, dimension, 0, 0, 1, 0, 32, 0]);
        ico.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        ico.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += payload.len();
    }
    for (_, payload) in &payloads {
        ico.extend_from_slice(payload);
    }
    std::fs::write(dir.join("icon.ico"), ico).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("icon.svg"), app_icon_svg()).map_err(|e| e.to_string())?;
    Ok(())
}

/// An ICO image entry as a 32-bit DIB: a BITMAPINFOHEADER whose height
/// counts both the colour rows and the mask rows, bottom-up BGRA pixels
/// (straight alpha), then an all-zero 1-bit AND mask padded to 32-bit rows.
fn ico_dib(render: &Render) -> Vec<u8> {
    let size = render.size;
    let mut out = Vec::with_capacity(40 + size * size * 4 + size * 4);
    let header: [u32; 3] = [40, size as u32, (size * 2) as u32];
    for word in header {
        out.extend_from_slice(&word.to_le_bytes());
    }
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&((size * size * 4) as u32).to_le_bytes());
    out.extend_from_slice(&[0u8; 16]); // resolution, colours used/important
    for y in (0..size).rev() {
        for x in 0..size {
            let i = (y * size + x) * 4;
            let px = &render.rgba[i..i + 4];
            out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
    let mask_row = size.div_ceil(32) * 4;
    out.extend(std::iter::repeat_n(0u8, mask_row * size));
    out
}

/// The same geometry as `render_app_icon`, as an SVG on a 256 grid.
fn app_icon_svg() -> String {
    let n = 256.0f32;
    let (cx, cy) = (n / 2.0, n / 2.0);
    let (r_out, r_in) = (n * 0.30, n * 0.195);
    // A ring arc from `start` degrees (clockwise from twelve) over `span`.
    let arc = |start: f32, span: f32, opacity: f32| {
        let point = |r: f32, deg: f32| {
            let rad = deg.to_radians();
            (cx + r * rad.sin(), cy - r * rad.cos())
        };
        let (ax, ay) = point(r_out, start);
        let (bx, by) = point(r_out, start + span);
        let (cx2, cy2) = point(r_in, start + span);
        let (dx, dy) = point(r_in, start);
        let large = if span > 180.0 { 1 } else { 0 };
        format!(
            "  <path fill=\"#fff\" fill-opacity=\"{opacity}\" d=\"M{ax:.2} {ay:.2} A{r_out:.2} {r_out:.2} 0 {large} 1 {bx:.2} {by:.2} L{cx2:.2} {cy2:.2} A{r_in:.2} {r_in:.2} 0 {large} 0 {dx:.2} {dy:.2} Z\"/>\n"
        )
    };
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 256 256\" width=\"256\" height=\"256\">\n  <rect width=\"256\" height=\"256\" rx=\"{:.1}\" fill=\"#000\"/>\n{}{}  <circle cx=\"{cx}\" cy=\"{cy}\" r=\"{:.2}\" fill=\"#fff\"/>\n</svg>\n",
        n * 0.22,
        arc(225.0, 270.0, 0.38),
        arc(225.0, 170.0, 1.0),
        n * 0.06
    )
}

/// Every mode and style at the sizes the taskbar uses, written as PNGs for
/// a look before shipping. `--render-tray-previews <dir>`.
pub fn write_previews(dir: &std::path::Path) -> Result<usize, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut written = 0;
    let contents: Vec<(&str, Content)> = vec![
        ("logo", Content::Logo),
        ("number-7", Content::Value { percent: 7.0, style: TrayIconStyle::Number, mark: Mark::None, label: String::new() }),
        ("number-42", Content::Value { percent: 42.0, style: TrayIconStyle::Number, mark: Mark::None, label: String::new() }),
        ("number-100", Content::Value { percent: 100.0, style: TrayIconStyle::Number, mark: Mark::None, label: String::new() }),
        ("bar-25", Content::Value { percent: 25.0, style: TrayIconStyle::Bar, mark: Mark::None, label: String::new() }),
        ("bar-80", Content::Value { percent: 80.0, style: TrayIconStyle::Bar, mark: Mark::None, label: String::new() }),
        ("column-25", Content::Value { percent: 25.0, style: TrayIconStyle::Column, mark: Mark::None, label: String::new() }),
        ("column-80", Content::Value { percent: 80.0, style: TrayIconStyle::Column, mark: Mark::None, label: String::new() }),
        ("ring-33", Content::value(33.0, TrayIconStyle::Ring)),
        ("ring-91", Content::value(91.0, TrayIconStyle::Ring)),
        ("ring-cl-64", Content::Value { percent: 64.0, style: TrayIconStyle::Ring, mark: Mark::Label, label: "CL".into() }),
        ("ring-cx-12", Content::Value { percent: 12.0, style: TrayIconStyle::Ring, mark: Mark::Label, label: "CX".into() }),
        ("ring-plain-50", Content::Value { percent: 50.0, style: TrayIconStyle::Ring, mark: Mark::None, label: String::new() }),
        ("letters-cl-64", Content::Value { percent: 64.0, style: TrayIconStyle::Letters, mark: Mark::Label, label: "CL".into() }),
        ("letters-opu-30", Content::Value { percent: 30.0, style: TrayIconStyle::Letters, mark: Mark::Label, label: "OPU".into() }),
        ("letters-gk-95", Content::Value { percent: 95.0, style: TrayIconStyle::Letters, mark: Mark::Label, label: "GK".into() }),
        ("bar-caption-cx-80", Content::Value { percent: 80.0, style: TrayIconStyle::Bar, mark: Mark::Label, label: "CX".into() }),
        ("bar-caption-digits-80", Content::Value { percent: 80.0, style: TrayIconStyle::Bar, mark: Mark::Digits, label: String::new() }),
        ("column-caption-ag-40", Content::Value { percent: 40.0, style: TrayIconStyle::Column, mark: Mark::Label, label: "AG".into() }),
        ("number-caption-cl-42", Content::Value { percent: 42.0, style: TrayIconStyle::Number, mark: Mark::Label, label: "CL".into() }),
        ("rundown-5", Content::Rundown { bars: vec![Some(21.0), Some(64.0), Some(4.0), None, Some(88.0)], rows: false }),
        ("rundown-8", Content::Rundown { bars: vec![Some(21.0), Some(64.0), Some(4.0), None, Some(88.0), Some(50.0), None, Some(97.0)], rows: false }),
        ("rundown-rows-5", Content::Rundown { bars: vec![Some(21.0), Some(64.0), Some(4.0), None, Some(88.0)], rows: true }),
    ];
    for (name, content) in &contents {
        for size in [16usize, 20, 24, 32, 64] {
            for (tone, light) in [("dark-taskbar", true), ("light-taskbar", false)] {
                let render = render(content, size, light);
                write_composited(dir, &format!("{name}-{size}-{tone}.png"), &render, light)?;
                written += 1;
            }
        }
    }
    // The alert tints, on both taskbars.
    for (name, rgb) in [("warning", ALERT_WARNING), ("critical", ALERT_CRITICAL)] {
        for size in [16usize, 24, 32] {
            for (tone, light) in [("dark-taskbar", true), ("light-taskbar", false)] {
                let render = render_tinted(&Content::value(88.0, TrayIconStyle::Ring), size, rgb[usize::from(!light)]);
                write_composited(dir, &format!("ring-{name}-{size}-{tone}.png"), &render, light)?;
                written += 1;
            }
        }
    }
    Ok(written)
}

/// The tint for a value at the warning line, on a dark taskbar then a light one.
pub const ALERT_WARNING: [[u8; 3]; 2] = [[245, 165, 36], [183, 121, 31]];
/// The tint for a value at the critical line, on a dark taskbar then a light one.
pub const ALERT_CRITICAL: [[u8; 3]; 2] = [[239, 68, 68], [217, 45, 32]];

/// Composite onto the taskbar colour so the PNG shows what a person sees,
/// not alpha on a checkerboard.
fn write_composited(dir: &std::path::Path, name: &str, render: &Render, light: bool) -> Result<(), String> {
    let size = render.size;
    let bg: [u8; 3] = if light { [32, 32, 32] } else { [243, 243, 243] };
    let mut rgb = Vec::with_capacity(size * size * 3);
    for px in render.rgba.chunks_exact(4) {
        let a = f32::from(px[3]) / 255.0;
        for channel in 0..3 {
            rgb.push((f32::from(px[channel]) * a + f32::from(bg[channel]) * (1.0 - a)).round() as u8);
        }
    }
    let image = image::RgbImage::from_raw(size as u32, size as u32, rgb).ok_or("bad buffer")?;
    image.save(dir.join(name)).map_err(|e| e.to_string())
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
        assert_eq!(provider_percent(&data, &TrayIconMetric::Tightest), 40.0);
        assert_eq!(provider_percent(&data, &TrayIconMetric::Session), 12.0);
        data.monthly = Some(UsageSection { percentage: 77.0, resets_at: None });
        assert_eq!(provider_percent(&data, &TrayIconMetric::Tightest), 77.0);
    }

    #[test]
    fn content_follows_the_mode_and_falls_back_to_the_logo() {
        let mut data = AppUsageData::default();
        data.insert(ProviderId::Claude, usage(30.0, 55.0));
        data.insert(ProviderId::Grok, usage(0.0, 80.0));
        let enabled = ProviderSet::from_enabled([ProviderId::Claude, ProviderId::Grok, ProviderId::Codex]);
        let settings = TrayIconSettings { mode: TrayIconMode::Tightest, ..Default::default() };
        assert_eq!(
            content(&settings, Some(&data), enabled),
            Content::Value { percent: 80.0, style: TrayIconStyle::Ring, mark: Mark::Digits, label: "GK".into() }
        );
        let settings = TrayIconSettings { mode: TrayIconMode::Provider, provider: Some("claude".into()), metric: TrayIconMetric::Session, style: TrayIconStyle::Number, ..Default::default() };
        assert_eq!(
            content(&settings, Some(&data), enabled),
            Content::Value { percent: 30.0, style: TrayIconStyle::Number, mark: Mark::None, label: "CL".into() },
            "a number never carries a second percent"
        );
        let settings = TrayIconSettings { mode: TrayIconMode::Rundown, ..Default::default() };
        assert_eq!(content(&settings, Some(&data), enabled), Content::Rundown { bars: vec![Some(55.0), None, Some(80.0)], rows: false });
        assert_eq!(content(&settings, None, enabled), Content::Logo);
        let settings = TrayIconSettings { mode: TrayIconMode::Provider, provider: Some("devin".into()), ..Default::default() };
        assert_eq!(content(&settings, Some(&data), enabled), Content::Logo);
    }

    #[test]
    fn the_options_shape_the_value() {
        let mut data = AppUsageData::default();
        data.insert(ProviderId::Claude, usage(30.0, 55.0));
        data.insert(ProviderId::Grok, usage(0.0, 80.0));
        let enabled = ProviderSet::from_enabled([ProviderId::Claude, ProviderId::Grok]);
        // What is left, not what is used; the alert still judges what is used.
        let settings = TrayIconSettings { mode: TrayIconMode::Tightest, measure: TrayIconMeasure::Remaining, ..Default::default() };
        assert_eq!(
            content(&settings, Some(&data), enabled),
            Content::Value { percent: 20.0, style: TrayIconStyle::Ring, mark: Mark::Digits, label: "GK".into() }
        );
        assert_eq!(shown_used_percent(&settings, Some(&data), enabled), Some(80.0));
        // The tightest provider's mark rides inside the ring.
        let settings = TrayIconSettings { mode: TrayIconMode::Tightest, mark: TrayIconMark::Initials, ..Default::default() };
        assert_eq!(content(&settings, Some(&data), enabled), Content::Value { percent: 80.0, style: TrayIconStyle::Ring, mark: Mark::Label, label: "GK".into() });
        // The window applies to the fleet too: by session Claude is tightest.
        let settings = TrayIconSettings { mode: TrayIconMode::Tightest, metric: TrayIconMetric::Session, mark: TrayIconMark::Initials, ..Default::default() };
        assert_eq!(content(&settings, Some(&data), enabled), Content::Value { percent: 30.0, style: TrayIconStyle::Ring, mark: Mark::Label, label: "CL".into() });
        // Monthly falls back to weekly where there is no monthly window.
        assert_eq!(provider_percent(&usage(30.0, 55.0), &TrayIconMetric::Monthly), 55.0);
        let mut monthly = usage(30.0, 55.0);
        monthly.monthly = Some(UsageSection { percentage: 9.0, resets_at: None });
        assert_eq!(provider_percent(&monthly, &TrayIconMetric::Monthly), 9.0);
        // A rundown in the bar style is rows; its alert is its worst bar.
        let settings = TrayIconSettings { mode: TrayIconMode::Rundown, style: TrayIconStyle::Bar, ..Default::default() };
        assert_eq!(content(&settings, Some(&data), enabled), Content::Rundown { bars: vec![Some(55.0), Some(80.0)], rows: true });
        assert_eq!(shown_used_percent(&settings, Some(&data), enabled), Some(80.0));
        assert_eq!(shown_used_percent(&TrayIconSettings { mode: TrayIconMode::Logo, ..Default::default() }, Some(&data), enabled), None);
    }

    #[test]
    fn a_provider_offers_the_limits_it_reports() {
        let mut data = usage(30.0, 55.0);
        assert_eq!(provider_windows(&data).len(), 3, "tightest, session, weekly");
        data.weekly_label = Some("7d".into());
        data.scoped.push(crate::models::ScopedLimit { label: "Fable".into(), window: crate::models::LimitWindow::Weekly, section: UsageSection { percentage: 71.0, resets_at: None } });
        data.credits = Some(crate::models::CreditsSection { percentage: 12.0, remaining: 88.0, total: 100.0 });
        let windows = provider_windows(&data);
        let titles: Vec<&str> = windows.iter().map(|(_, title)| title.as_str()).collect();
        assert_eq!(titles, vec!["Tightest window", "Session", "Weekly (7d)", "Fable · weekly", "Credits"]);
        assert_eq!(provider_percent(&data, &TrayIconMetric::Scoped("Fable".into())), 71.0);
        assert_eq!(provider_percent(&data, &TrayIconMetric::Credits), 12.0);
        assert_eq!(provider_percent(&data, &TrayIconMetric::Scoped("gone".into())), 71.0, "a cap no longer reported falls back to the tightest");
        assert_eq!(metric_name(&TrayIconMetric::Scoped("Fable".into()), Some(&data)), "Fable · weekly");
        assert_eq!(metric_name(&TrayIconMetric::Scoped("Fable".into()), None), "Fable");
        assert_eq!(metric_name(&TrayIconMetric::Weekly, Some(&data)), "Weekly (7d)");
        // A session the provider does not bill is not offered, and an icon
        // still set to it shows the tightest rather than a flat zero.
        let weekly_only = usage(0.0, 40.0);
        assert!(!provider_windows(&weekly_only).iter().any(|(metric, _)| *metric == TrayIconMetric::Session));
        assert_eq!(provider_percent(&weekly_only, &TrayIconMetric::Session), 40.0);
        assert_eq!(reported_percent(&weekly_only, &TrayIconMetric::Session), None, "a fleet view leaves it out instead");
    }

    #[test]
    fn every_layout_paints_inside_the_square_at_every_size() {
        for size in [16usize, 20, 24, 32, 64] {
            for content in [
                Content::Logo,
                Content::value(100.0, TrayIconStyle::Number),
                Content::value(50.0, TrayIconStyle::Bar),
                Content::value(50.0, TrayIconStyle::Column),
                Content::value(50.0, TrayIconStyle::Ring),
                Content::Value { percent: 50.0, style: TrayIconStyle::Ring, mark: Mark::Label, label: "CL".into() },
                Content::Value { percent: 50.0, style: TrayIconStyle::Letters, mark: Mark::Label, label: "OPU".into() },
                Content::Value { percent: 50.0, style: TrayIconStyle::Bar, mark: Mark::Label, label: "CL".into() },
                Content::Value { percent: 50.0, style: TrayIconStyle::Number, mark: Mark::Digits, label: String::new() },
                Content::Rundown { bars: vec![Some(10.0), None, Some(90.0), Some(50.0), Some(5.0), Some(70.0), Some(30.0), Some(99.0)], rows: false },
                Content::Rundown { bars: vec![Some(10.0), None, Some(90.0), Some(50.0), Some(5.0), Some(70.0), Some(30.0), Some(99.0)], rows: true },
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
    fn bars_columns_and_rings_fill_with_the_percentage() {
        for style in [TrayIconStyle::Bar, TrayIconStyle::Column, TrayIconStyle::Ring, TrayIconStyle::Letters] {
            let low = solid(&super::render(&Content::Value { percent: 20.0, style, mark: Mark::None, label: "CL".into() }, 32, true));
            let high = solid(&super::render(&Content::Value { percent: 90.0, style, mark: Mark::None, label: "CL".into() }, 32, true));
            assert!(high > low, "{style:?}: {high} vs {low}");
        }
        for rows in [false, true] {
            let empty = super::render(&Content::Rundown { bars: vec![Some(0.0), Some(0.0)], rows }, 32, true);
            let full = super::render(&Content::Rundown { bars: vec![Some(100.0), Some(100.0)], rows }, 32, true);
            assert!(solid(&full) > solid(&empty));
        }
        // A column fills upward: the bottom half is solid before the top.
        let half = super::render(&Content::Value { percent: 50.0, style: TrayIconStyle::Column, mark: Mark::None, label: String::new() }, 32, true);
        let solid_rows: Vec<usize> = (0..32).filter(|y| (0..32).any(|x| half.rgba[(y * 32 + x) * 4 + 3] > 200)).collect();
        assert!(solid_rows.iter().filter(|y| **y >= 16).count() > solid_rows.iter().filter(|y| **y < 8).count());
    }

    #[test]
    fn letters_fill_and_captions_appear_only_where_they_can_be_read() {
        let letters = |percent, label: &str, mark| super::render(&Content::Value { percent, style: TrayIconStyle::Letters, mark, label: label.into() }, 16, true);
        // The letters are there dim at 0 %, solid as the value rises.
        assert!(lit(&letters(0.0, "CL", Mark::None)) > 0);
        assert!(solid(&letters(0.0, "CL", Mark::None)) == 0, "nothing solid at zero");
        assert!(solid(&letters(50.0, "CL", Mark::None)) > 0);
        assert!(solid(&letters(100.0, "CL", Mark::None)) > solid(&letters(50.0, "CL", Mark::None)));
        // Three letters still span the square at sixteen pixels.
        let (min, max) = lit_columns(&letters(100.0, "OPU", Mark::None));
        assert!(max - min >= 10, "{min}..{max}");
        // A digits mark rides above the letters and costs them height, and
        // the fill still rises from the bottom of the letters' own band.
        assert!(lit(&letters(60.0, "CL", Mark::Digits)) > lit(&letters(60.0, "CL", Mark::None)));
        let captioned = super::render(&Content::Value { percent: 50.0, style: TrayIconStyle::Letters, mark: Mark::Digits, label: "CL".into() }, 32, true);
        let full = super::render(&Content::Value { percent: 100.0, style: TrayIconStyle::Letters, mark: Mark::Digits, label: "CL".into() }, 32, true);
        assert!(solid(&full) > solid(&captioned));
        // A label that happens to spell the current percent still gets its
        // caption: the layout differs from the caption-less render.
        let coincidence = super::render(&Content::Value { percent: 50.0, style: TrayIconStyle::Letters, mark: Mark::Digits, label: "50".into() }, 32, true);
        assert_ne!(coincidence.rgba, super::render(&Content::Value { percent: 50.0, style: TrayIconStyle::Letters, mark: Mark::None, label: "50".into() }, 32, true).rgba);
        // The fill rises from the bottom: at 50 % nothing solid in the top half.
        let half = super::render(&Content::Value { percent: 50.0, style: TrayIconStyle::Letters, mark: Mark::None, label: "CL".into() }, 32, true);
        let solid_rows: Vec<usize> = (0..32).filter(|y| (0..32).any(|x| half.rgba[(y * 32 + x) * 4 + 3] > 200)).collect();
        assert!(solid_rows.iter().all(|y| *y >= 16), "{solid_rows:?}");

        // Captions: two characters read from sixteen pixels; the shape gets
        // what is left. No text, no band.
        let band = |size, text: &str| paint_caption(&mut Canvas::new(size), text);
        assert_eq!(band(16, ""), (0.0, 16.0));
        let (top, bottom) = band(16, "CL");
        assert!(top > 5.0 && bottom == 16.0, "{top}..{bottom}");
        assert!(band(24, "CL").0 > 7.0);
        assert!(band(32, "OPU").0 > 8.0);
        assert!(band(32, "80").0 > 8.0);
        // The bar really moves down under its caption, at sixteen too.
        let top_quarter_lit = |render: &Render| (0..render.size / 4).any(|y| (0..render.size).any(|x| render.rgba[(y * render.size + x) * 4 + 3] > 64));
        assert!(!top_quarter_lit(&super::render(&Content::Value { percent: 60.0, style: TrayIconStyle::Bar, mark: Mark::None, label: String::new() }, 16, true)));
        assert!(top_quarter_lit(&super::render(&Content::Value { percent: 60.0, style: TrayIconStyle::Bar, mark: Mark::Label, label: "CL".into() }, 16, true)));
        // A reading that is not a number draws as nothing used, caption intact.
        let nan = super::render(&Content::Value { percent: f64::NAN, style: TrayIconStyle::Bar, mark: Mark::Label, label: "CL".into() }, 24, true);
        let zero = super::render(&Content::Value { percent: 0.0, style: TrayIconStyle::Bar, mark: Mark::Label, label: "CL".into() }, 24, true);
        assert_eq!(nan.rgba, zero.rgba);
        let negative = super::render(&Content::value(-5.0, TrayIconStyle::Ring), 24, true);
        assert_eq!(negative.rgba, super::render(&Content::value(0.0, TrayIconStyle::Ring), 24, true).rgba);
        // A number style never carries a second percent; letters never
        // carry themselves again.
        let mut data = AppUsageData::default();
        data.insert(ProviderId::Claude, usage(30.0, 55.0));
        let enabled = ProviderSet::from_enabled([ProviderId::Claude]);
        let settings = TrayIconSettings { mode: TrayIconMode::Provider, provider: Some("claude".into()), style: TrayIconStyle::Number, mark: TrayIconMark::Digits, ..Default::default() };
        assert!(matches!(content(&settings, Some(&data), enabled), Content::Value { mark: Mark::None, .. }));
        let settings = TrayIconSettings { mode: TrayIconMode::Provider, provider: Some("claude".into()), style: TrayIconStyle::Letters, label: Some("opus".into()), mark: TrayIconMark::Initials, ..Default::default() };
        assert_eq!(content(&settings, Some(&data), enabled), Content::Value { percent: 55.0, style: TrayIconStyle::Letters, mark: Mark::None, label: "OPU".into() });
        let settings = TrayIconSettings { mode: TrayIconMode::Provider, provider: Some("claude".into()), style: TrayIconStyle::Bar, mark: TrayIconMark::Initials, ..Default::default() };
        assert_eq!(content(&settings, Some(&data), enabled), Content::Value { percent: 55.0, style: TrayIconStyle::Bar, mark: Mark::Label, label: "CL".into() });
    }

    #[test]
    fn a_ring_carries_its_mark_only_where_it_can_be_read() {
        let ring = |size, mark, label: &str| solid(&super::render(&Content::Value { percent: 50.0, style: TrayIconStyle::Ring, mark, label: label.into() }, size, true));
        for size in [16usize, 20, 24, 32] {
            assert!(ring(size, Mark::Label, "CL") > ring(size, Mark::None, ""), "two letters read inside a {size} px ring");
        }
        assert!(ring(16, Mark::Digits, "") > ring(16, Mark::None, ""), "so do two digits");
        // The text block stays inside the ring: no solid pixel on the
        // ring's inner-edge diagonal corner.
        for size in [16usize, 24, 32] {
            let n = size as f32;
            let r_in = n * 0.35;
            let render = super::render(&Content::Value { percent: 0.0, style: TrayIconStyle::Ring, mark: Mark::Label, label: "CL".into() }, size, true);
            for (dx, dy) in [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
                let x = (n / 2.0 + dx * r_in * 0.72) as usize;
                let y = (n / 2.0 + dy * r_in * 0.72) as usize;
                // At 0 % the ring track is faint; anything solid here would
                // be text spilling into the ring.
                let alpha = render.rgba[(y * size + x) * 4 + 3];
                assert!(alpha < 200, "{size}: text touches the ring at {x},{y} ({alpha})");
            }
        }
        assert_eq!(
            solid(&super::render(&Content::Value { percent: 100.0, style: TrayIconStyle::Ring, mark: Mark::Digits, label: String::new() }, 16, true)),
            solid(&super::render(&Content::Value { percent: 100.0, style: TrayIconStyle::Ring, mark: Mark::None, label: String::new() }, 16, true)),
            "three digits do not fit a 16 px ring"
        );
        assert!(
            solid(&super::render(&Content::Value { percent: 100.0, style: TrayIconStyle::Ring, mark: Mark::Digits, label: String::new() }, 24, true))
                > solid(&super::render(&Content::Value { percent: 100.0, style: TrayIconStyle::Ring, mark: Mark::None, label: String::new() }, 24, true)),
            "they do at 24 px"
        );
        // Every provider's mark is made of letters the font has.        // Every provider's mark is made of letters the font has.
        for descriptor in crate::providers::PROVIDER_DESCRIPTORS {
            assert!(descriptor.tray_mark.chars().all(|c| glyph(c).is_some()), "{}", descriptor.tray_mark);
            assert!((1..=2).contains(&descriptor.tray_mark.len()));
        }
        let tinted = render_tinted(&Content::Logo, 16, [200, 10, 10]);
        let px = tinted.rgba.chunks_exact(4).find(|px| px[3] > 200).unwrap();
        assert_eq!(&px[..3], &[200, 10, 10]);
    }

    #[test]
    fn three_digits_fit_at_sixteen_pixels() {
        let render = super::render(&Content::value(100.0, TrayIconStyle::Number), 16, true);
        let (min, max) = lit_columns(&render);
        assert!(max - min >= 10, "three digits should span most of the width: {min}..{max}");
    }

    #[test]
    fn the_app_icon_is_a_black_plate_with_a_white_glyph() {
        let render = render_app_icon(64);
        let px = |x: usize, y: usize| {
            let i = (y * 64 + x) * 4;
            (&render.rgba[i..i + 4]).to_vec()
        };
        assert_eq!(px(0, 0)[3], 0, "the corner is outside the rounded plate");
        assert_eq!(px(32, 8)[3], 255, "inside the plate is opaque");
        assert_eq!(px(32, 8)[0], 0, "and black where the glyph is not");
        assert_eq!(px(32, 32)[0], 255, "the hub is white");
        assert!(app_icon_svg().contains("<rect") && app_icon_svg().contains("<circle"));
        let dib = ico_dib(&render_app_icon(16));
        assert_eq!(dib.len(), 40 + 16 * 16 * 4 + 4 * 16);
        assert_eq!(u32::from_le_bytes([dib[8], dib[9], dib[10], dib[11]]), 32, "height counts colour and mask rows");
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
