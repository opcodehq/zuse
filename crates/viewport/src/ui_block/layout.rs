//! Pure geometry and formatting for UI blocks: everything a renderer needs
//! that does not touch GPUI, kept here so it can be unit tested.

/// Gap between grid tracks, in logical pixels.
pub const GRID_GAP: f32 = 12.0;
/// Minimum grid track for compact children (Stat, Badge, Progress).
pub const GRID_MIN_COMPACT: f32 = 144.0;
/// Minimum grid track for everything else.
pub const GRID_MIN_WIDE: f32 = 256.0;
/// Below this container width a Table reflows each row into a card.
pub const TABLE_NARROW: f32 = 384.0;
/// At or above this container width a Flow lays out left to right.
pub const FLOW_ROW: f32 = 512.0;

/// Line chart plot margins: top, right, bottom, left.
pub const PLOT_MARGINS: [f32; 4] = [10.0, 12.0, 22.0, 36.0];
/// Height of the whole line chart box, margins included.
pub const LINE_CHART_HEIGHT: f32 = 176.0;

/// Format a number the way JavaScript's `String(number)` does.
pub fn js_number(value: f64) -> String {
    if value == 0.0 {
        return "0".into();
    }
    if !value.is_finite() {
        return if value.is_nan() {
            "NaN".into()
        } else if value > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        };
    }
    let magnitude = value.abs();
    if (1e-6..1e21).contains(&magnitude) {
        return format!("{value}");
    }
    let formatted = format!("{value:e}");
    match formatted.split_once('e') {
        Some((mantissa, exponent)) if !exponent.starts_with('-') => {
            format!("{mantissa}e+{exponent}")
        }
        _ => formatted,
    }
}

/// `Math.round`: halves round toward positive infinity.
pub fn js_round(value: f64) -> f64 {
    (value + 0.5).floor()
}

/// A chart value with its optional unit, e.g. `"820 ms"`.
pub fn chart_value(value: f64, unit: Option<&str>) -> String {
    match unit.filter(|unit| !unit.is_empty()) {
        Some(unit) => format!("{} {unit}", js_number(value)),
        None => js_number(value),
    }
}

/// Horizontal bar placement as fractions of the track width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BarSpan {
    pub start: f64,
    pub fraction: f64,
}

/// Bar chart scaling: the zero line position and each bar's span. Values are
/// normalised by the largest magnitude; negative values extend left of zero.
pub fn bar_spans(values: &[f64]) -> (f64, Vec<BarSpan>) {
    let largest = values.iter().fold(0.0f64, |acc, v| acc.max(v.abs()));
    let scale = if largest > 0.0 && largest.is_finite() {
        largest
    } else {
        1.0
    };
    let low = values.iter().fold(0.0f64, |acc, v| acc.min(v / scale));
    let high = values.iter().fold(0.0f64, |acc, v| acc.max(v / scale));
    let span = match high - low {
        span if span > 0.0 => span,
        _ => 1.0,
    };
    let zero = -low / span;
    let spans = values
        .iter()
        .map(|value| BarSpan {
            start: zero.min((value / scale - low) / span),
            fraction: (value / scale).abs() / span,
        })
        .collect();
    (zero, spans)
}

/// Length in UTF-16 code units, the unit the reference layout measures in.
fn js_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// Table column weights: the longest of the header and every cell, clamped
/// to 12..=48 characters. Widths are each weight over the total.
pub fn column_weights(columns: &[String], rows: &[Vec<String>]) -> Vec<f32> {
    columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let longest = rows
                .iter()
                .map(|row| row.get(index).map_or(0, |cell| js_len(cell)))
                .max()
                .unwrap_or(0);
            js_len(column).max(longest).clamp(12, 48) as f32
        })
        .collect()
}

/// One line of a reflowed (narrow) table row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NarrowLine {
    /// A cell spanning both columns.
    Full(usize),
    /// One or two half-width cells.
    Pair(usize, Option<usize>),
}

/// Cells longer than this span both columns of a reflowed row.
const NARROW_SPAN_CHARS: usize = 40;

/// Lay a row's cells into a two-column grid the way CSS auto-placement does:
/// the first cell and long cells span both columns; a spanning cell that
/// meets a half-filled line moves to the next one, leaving the gap.
pub fn narrow_lines(cells: &[String]) -> Vec<NarrowLine> {
    let mut lines = Vec::new();
    let mut pending: Option<usize> = None;
    for (index, cell) in cells.iter().enumerate() {
        if index == 0 || js_len(cell) > NARROW_SPAN_CHARS {
            if let Some(left) = pending.take() {
                lines.push(NarrowLine::Pair(left, None));
            }
            lines.push(NarrowLine::Full(index));
        } else if let Some(left) = pending.take() {
            lines.push(NarrowLine::Pair(left, Some(index)));
        } else {
            pending = Some(index);
        }
    }
    if let Some(left) = pending {
        lines.push(NarrowLine::Pair(left, None));
    }
    lines
}

/// Columns of an auto-fit grid with tracks of at least
/// `max(minimum, (width - gaps) / columns)`, never wider than the container,
/// collapsing empty tracks so `children` stretch to fill.
pub fn grid_columns(width: f32, columns: u8, minimum: f32, children: usize) -> usize {
    let requested = columns.max(1) as usize;
    if children == 0 {
        return 1;
    }
    if width.is_nan() || width <= 0.0 {
        return requested.min(children);
    }
    let count = requested as f32;
    let track = width.min(minimum.max((width - (count - 1.0) * GRID_GAP) / count));
    let fit = ((width + GRID_GAP + 0.01) / (track + GRID_GAP))
        .floor()
        .max(1.0) as usize;
    fit.min(children).max(1)
}

/// Compact notation like `Intl.NumberFormat("en", {notation: "compact",
/// maximumFractionDigits: 1})`: 1.2K, 3.4M, 12.3B.
pub fn compact_number(value: f64) -> String {
    if !value.is_finite() {
        return js_number(value);
    }
    const UNITS: [(f64, &str); 4] = [(1e12, "T"), (1e9, "B"), (1e6, "M"), (1e3, "K")];
    let sign = if value < 0.0 { "-" } else { "" };
    let magnitude = value.abs();
    let round1 = |v: f64| (v * 10.0).round() / 10.0;
    let mut unit = UNITS
        .iter()
        .position(|(base, _)| magnitude >= *base)
        .unwrap_or(UNITS.len());
    let mut scaled = match UNITS.get(unit) {
        Some((base, _)) => round1(magnitude / base),
        None => round1(magnitude),
    };
    // Rounding can carry into the next unit: 999.96 → 1K, 999,960 → 1M.
    if scaled >= 1000.0 && unit > 0 {
        unit -= 1;
        scaled = round1(magnitude / UNITS[unit].0);
    } else if scaled >= 1000.0 && unit == UNITS.len() {
        unit = UNITS.len() - 1;
        scaled = round1(magnitude / UNITS[unit].0);
    }
    let digits = if scaled.fract() == 0.0 {
        format!("{scaled:.0}")
    } else {
        format!("{scaled:.1}")
    };
    if digits == "0" {
        return "0".into();
    }
    let suffix = UNITS.get(unit).map_or("", |(_, suffix)| suffix);
    format!("{sign}{digits}{suffix}")
}

/// Whole percentage of `value` in `total` (0 when the total is 0).
pub fn percent_of(value: f64, total: f64) -> f64 {
    if total == 0.0 {
        0.0
    } else {
        js_round(value / total * 100.0)
    }
}

/// Progress percent shown next to a bar: clamped to 0–100 and rounded.
pub fn progress_percent(percent: f64) -> f64 {
    js_round(percent.clamp(0.0, 100.0))
}

const E10: f64 = 7.0710678118654755; // sqrt(50)
const E5: f64 = 3.1622776601683795; // sqrt(10)
const E2: f64 = std::f64::consts::SQRT_2;

fn tick_spec(start: f64, stop: f64, count: f64) -> (f64, f64, f64) {
    let step = (stop - start) / count.max(0.0);
    let power = step.log10().floor();
    let error = step / 10f64.powf(power);
    let factor = if error >= E10 {
        10.0
    } else if error >= E5 {
        5.0
    } else if error >= E2 {
        2.0
    } else {
        1.0
    };
    let (mut i1, mut i2, inc);
    if power < 0.0 {
        let increment = 10f64.powf(-power) / factor;
        i1 = (start * increment).round();
        i2 = (stop * increment).round();
        if i1 / increment < start {
            i1 += 1.0;
        }
        if i2 / increment > stop {
            i2 -= 1.0;
        }
        inc = -increment;
    } else {
        let increment = 10f64.powf(power) * factor;
        i1 = (start / increment).round();
        i2 = (stop / increment).round();
        if i1 * increment < start {
            i1 += 1.0;
        }
        if i2 * increment > stop {
            i2 -= 1.0;
        }
        inc = increment;
    }
    if i2 < i1 && (0.5..2.0).contains(&count) {
        return tick_spec(start, stop, count * 2.0);
    }
    (i1, i2, inc)
}

fn tick_increment(start: f64, stop: f64, count: f64) -> f64 {
    tick_spec(start, stop, count).2
}

/// Round tick values over `[start, stop]`, about `count` of them (d3's
/// `ticks`).
pub fn ticks(start: f64, stop: f64, count: usize) -> Vec<f64> {
    let count = count as f64;
    if count <= 0.0 || !start.is_finite() || !stop.is_finite() {
        return Vec::new();
    }
    if start == stop {
        return vec![start];
    }
    let reverse = stop < start;
    let (lo, hi) = if reverse {
        (stop, start)
    } else {
        (start, stop)
    };
    let (i1, i2, inc) = tick_spec(lo, hi, count);
    if i2 < i1 {
        return Vec::new();
    }
    let n = (i2 - i1 + 1.0) as usize;
    let mut out: Vec<f64> = (0..n)
        .map(|i| {
            let k = i1 + i as f64;
            if inc < 0.0 { k / -inc } else { k * inc }
        })
        .collect();
    if reverse {
        out.reverse();
    }
    out
}

/// Extend `[start, stop]` to round tick boundaries (d3's `linear.nice`).
pub fn nice(start: f64, stop: f64, count: usize) -> (f64, f64) {
    let (mut start, mut stop) = (start, stop);
    let reverse = stop < start;
    if reverse {
        std::mem::swap(&mut start, &mut stop);
    }
    let mut previous: Option<f64> = None;
    for _ in 0..10 {
        let step = tick_increment(start, stop, count as f64);
        if previous == Some(step) {
            break;
        }
        if step > 0.0 {
            start = (start / step).floor() * step;
            stop = (stop / step).ceil() * step;
        } else if step < 0.0 {
            start = (start * step).ceil() / step;
            stop = (stop * step).floor() / step;
        } else {
            break;
        }
        previous = Some(step);
    }
    if reverse {
        (stop, start)
    } else {
        (start, stop)
    }
}

/// Line chart value domain: always includes zero, an all-zero series spans
/// 0–1, and the result is niced to round ticks.
pub fn line_domain(values: &[f64]) -> (f64, f64) {
    let mut min = 0.0f64;
    let mut max = 0.0f64;
    for value in values.iter().filter(|v| v.is_finite()) {
        min = min.min(*value);
        max = max.max(*value);
    }
    if min == 0.0 && max == 0.0 {
        max = 1.0;
    }
    let lo = min.min(0.0);
    let hi = max.max(0.0);
    nice(lo, if hi == lo { lo + 1.0 } else { hi }, 10)
}

/// Vertical pixel of `value` in a plot `height` tall over `domain`.
pub fn y_pixel(value: f64, domain: (f64, f64), height: f64) -> f64 {
    let (lo, hi) = domain;
    let span = hi - lo;
    if span == 0.0 {
        return height;
    }
    height - (value - lo) / span * height
}

/// Horizontal pixel of point `index` of `len` on a point scale `width` wide
/// (first and last on the edges; a single point is centred).
pub fn x_pixel(index: usize, len: usize, width: f64) -> f64 {
    if len <= 1 {
        width / 2.0
    } else {
        index as f64 * width / (len - 1) as f64
    }
}

/// Index of the point nearest horizontal offset `x` within the plot.
pub fn nearest_index(x: f64, len: usize, width: f64) -> usize {
    if len <= 1 || width <= 0.0 {
        return 0;
    }
    js_round((x / width).clamp(0.0, 1.0) * (len - 1) as f64) as usize
}

/// Every how many points an x-axis label is drawn so at most `max` show.
pub fn x_label_step(len: usize, max: usize) -> usize {
    len.div_ceil(max.max(1)).max(1)
}

/// Tooltip anchor x: the cursor, kept 48px inside the chart's edges.
pub fn tooltip_left(cursor_x: f32, plot_width: f32, margin_left: f32) -> f32 {
    cursor_x.min(plot_width + margin_left - 48.0).max(48.0)
}

/// Snap a slider value to its step from `min`, within the range, without
/// accumulating floating-point noise from the step.
pub fn snap_slider(value: f64, min: f64, max: f64, step: f64) -> f64 {
    let step = if step > 0.0 && step.is_finite() {
        step
    } else {
        1.0
    };
    let steps = ((value - min) / step).round();
    let snapped = (min + steps * step).clamp(min, max);
    let decimals = decimal_places(step).max(decimal_places(min));
    let factor = 10f64.powi(decimals.min(12) as i32);
    (snapped * factor).round() / factor
}

fn decimal_places(value: f64) -> usize {
    let text = js_number(value);
    if let Some((_, exponent)) = text.split_once("e-") {
        return exponent.parse().unwrap_or(0);
    }
    text.split_once('.')
        .map_or(0, |(_, fraction)| fraction.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_scale_by_the_largest_magnitude() {
        let (zero, spans) = bar_spans(&[820.0, 540.0]);
        assert_eq!(zero, 0.0);
        assert_eq!(
            spans[0],
            BarSpan {
                start: 0.0,
                fraction: 1.0
            }
        );
        assert!((spans[1].fraction - 540.0 / 820.0).abs() < 1e-12);
    }

    #[test]
    fn negative_bars_extend_left_of_zero() {
        let (zero, spans) = bar_spans(&[-50.0, 100.0, 25.0]);
        // low = -0.5, high = 1, span = 1.5.
        assert!((zero - 1.0 / 3.0).abs() < 1e-12);
        assert!((spans[0].start - 0.0).abs() < 1e-12);
        assert!((spans[0].fraction - 1.0 / 3.0).abs() < 1e-12);
        assert!((spans[1].start - 1.0 / 3.0).abs() < 1e-12);
        assert!((spans[1].fraction - 2.0 / 3.0).abs() < 1e-12);
        assert!((spans[2].start - zero).abs() < 1e-12);
        let (zero, spans) = bar_spans(&[-4.0, -2.0]);
        assert_eq!(zero, 1.0);
        assert_eq!(
            spans[0],
            BarSpan {
                start: 0.0,
                fraction: 1.0
            }
        );
        assert_eq!(
            spans[1],
            BarSpan {
                start: 0.5,
                fraction: 0.5
            }
        );
    }

    #[test]
    fn all_zero_bars_are_empty() {
        let (zero, spans) = bar_spans(&[0.0, 0.0]);
        assert_eq!(zero, 0.0);
        assert!(spans.iter().all(|span| span.fraction == 0.0));
    }

    #[test]
    fn column_weights_clamp_to_cell_lengths() {
        let columns = vec!["Id".to_string(), "Description".into(), "x".repeat(60)];
        let rows = vec![
            vec!["1".to_string(), "a".repeat(30), "b".into()],
            vec!["2".to_string(), "short".into(), "c".into()],
        ];
        assert_eq!(column_weights(&columns, &rows), vec![12.0, 30.0, 48.0]);
        // UTF-16 length, like the reference.
        let emoji = vec!["😀".repeat(10)];
        assert_eq!(column_weights(&emoji, &[]), vec![20.0]);
    }

    #[test]
    fn narrow_rows_follow_grid_auto_placement() {
        let cells: Vec<String> = ["Name", "a", "b", &"x".repeat(41), "c"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            narrow_lines(&cells),
            vec![
                NarrowLine::Full(0),
                NarrowLine::Pair(1, Some(2)),
                NarrowLine::Full(3),
                NarrowLine::Pair(4, None),
            ]
        );
        let cells: Vec<String> = ["Name", "a", &"x".repeat(41)]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            narrow_lines(&cells),
            vec![
                NarrowLine::Full(0),
                NarrowLine::Pair(1, None),
                NarrowLine::Full(2)
            ]
        );
    }

    #[test]
    fn grid_wraps_below_the_minimum_track() {
        assert_eq!(grid_columns(600.0, 2, GRID_MIN_WIDE, 4), 2);
        assert_eq!(grid_columns(500.0, 2, GRID_MIN_WIDE, 4), 1);
        assert_eq!(grid_columns(700.0, 4, GRID_MIN_COMPACT, 4), 4);
        assert_eq!(grid_columns(500.0, 4, GRID_MIN_COMPACT, 4), 3);
        assert_eq!(grid_columns(300.0, 4, GRID_MIN_COMPACT, 4), 2);
        assert_eq!(grid_columns(100.0, 4, GRID_MIN_COMPACT, 4), 1);
        // Empty tracks collapse: two children in a three-column grid.
        assert_eq!(grid_columns(900.0, 3, GRID_MIN_WIDE, 2), 2);
        // Unmeasured: the requested columns.
        assert_eq!(grid_columns(0.0, 3, GRID_MIN_WIDE, 5), 3);
    }

    #[test]
    fn compact_numbers_match_intl() {
        for (value, expected) in [
            (0.0, "0"),
            (5.0, "5"),
            (12.34, "12.3"),
            (999.0, "999"),
            (999.96, "1K"),
            (1000.0, "1K"),
            (1234.0, "1.2K"),
            (12_345.0, "12.3K"),
            (123_456.0, "123.5K"),
            (999_999.0, "1M"),
            (3_400_000.0, "3.4M"),
            (2.5e9, "2.5B"),
            (7e12, "7T"),
            (-1500.0, "-1.5K"),
            (0.04, "0"),
        ] {
            assert_eq!(compact_number(value), expected, "{value}");
        }
    }

    #[test]
    fn nice_ticks_match_d3() {
        assert_eq!(ticks(0.0, 1.0, 4), vec![0.0, 0.2, 0.4, 0.6, 0.8, 1.0]);
        assert_eq!(ticks(0.0, 70.0, 4), vec![0.0, 20.0, 40.0, 60.0]);
        assert_eq!(
            ticks(-20.0, 100.0, 4),
            vec![-20.0, 0.0, 20.0, 40.0, 60.0, 80.0, 100.0]
        );
        assert_eq!(ticks(0.0, 0.3, 4), vec![0.0, 0.1, 0.2, 0.3]);
        assert_eq!(nice(0.0, 73.0, 10), (0.0, 80.0));
        assert_eq!(nice(0.0, 70.0, 10), (0.0, 70.0));
        assert_eq!(nice(-13.0, 97.0, 10), (-20.0, 100.0));
        assert_eq!(line_domain(&[50.0, 70.0]), (0.0, 70.0));
        assert_eq!(line_domain(&[0.0, 0.0]), (0.0, 1.0));
        assert_eq!(line_domain(&[-3.0, 8.5]), (-3.0, 9.0));
    }

    #[test]
    fn percentages_round_like_math_round() {
        assert_eq!(percent_of(1.0, 3.0), 33.0);
        assert_eq!(percent_of(2.0, 3.0), 67.0);
        assert_eq!(percent_of(1.0, 8.0), 13.0);
        assert_eq!(percent_of(5.0, 0.0), 0.0);
        assert_eq!(progress_percent(42.5), 43.0);
        assert_eq!(progress_percent(140.0), 100.0);
        assert_eq!(progress_percent(-3.0), 0.0);
    }

    #[test]
    fn point_scale_and_hover() {
        assert_eq!(x_pixel(0, 5, 100.0), 0.0);
        assert_eq!(x_pixel(4, 5, 100.0), 100.0);
        assert_eq!(x_pixel(0, 1, 100.0), 50.0);
        assert_eq!(nearest_index(60.0, 5, 100.0), 2);
        assert_eq!(nearest_index(63.0, 5, 100.0), 3);
        assert_eq!(nearest_index(-10.0, 5, 100.0), 0);
        assert_eq!(x_label_step(10, 4), 3);
        assert_eq!(x_label_step(2, 4), 1);
        assert_eq!(tooltip_left(10.0, 300.0, 36.0), 48.0);
        assert_eq!(tooltip_left(400.0, 300.0, 36.0), 288.0);
        assert_eq!(y_pixel(0.0, (0.0, 100.0), 144.0), 144.0);
        assert_eq!(y_pixel(100.0, (0.0, 100.0), 144.0), 0.0);
    }

    #[test]
    fn sliders_snap_without_float_noise() {
        assert_eq!(snap_slider(0.31, 0.0, 1.0, 0.1), 0.3);
        assert_eq!(snap_slider(7.4, 0.0, 10.0, 1.0), 7.0);
        assert_eq!(snap_slider(12.0, 0.0, 10.0, 1.0), 10.0);
        assert_eq!(snap_slider(3.0, 1.0, 9.0, 2.5), 3.5);
    }

    #[test]
    fn chart_values_print_like_javascript() {
        assert_eq!(chart_value(820.0, Some("ms")), "820 ms");
        assert_eq!(chart_value(-2.5, None), "-2.5");
        assert_eq!(chart_value(1e21, Some("")), "1e+21");
    }
}
