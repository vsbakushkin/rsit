//! Paints one row of the commit graph. Port of IntelliJ `SimpleGraphCellPainter`.

use gpui_kit::*;
use rsit_graph::{EdgeDir, PrintElement, PrintKind};

pub const ROW_HEIGHT: f32 = 22.0;
/// Horizontal space per lane (`PaintParameters.WIDTH_NODE`).
pub const LANE_WIDTH: f32 = 16.0;
const LINE: f32 = 1.5;
const CIRCLE_RADIUS: f32 = 4.0;
const HEAD_RADIUS_DELTA: f32 = 2.0;
const ARROW_ANGLE_COS2: f32 = 0.7;
const ARROW_LENGTH: f32 = 0.3;

pub fn lane_color(color_id: i32) -> Hsla {
    rgb(rsit_graph::color::rgb_for_color_id(color_id)).into()
}

/// Width in pixels taken by the graph in a row.
pub fn graph_width(elements: &[PrintElement]) -> f32 {
    let lanes = elements.iter().map(|e| e.pos.max(e.other_pos()) + 1).max().unwrap_or(0);
    lanes as f32 * LANE_WIDTH
}

pub struct RowGraph {
    pub elements: Vec<PrintElement>,
    /// The row's commit is HEAD: drawn with IntelliJ's ringed head node.
    pub is_head: bool,
    pub background: Hsla,
}

pub fn paint_row(bounds: Bounds<Pixels>, row: RowGraph, window: &mut Window) {
    let origin = bounds.origin;
    let row_center = (ROW_HEIGHT / 2.0).floor();
    let element_center = (LANE_WIDTH / 2.0).floor();
    let point = |x: f32, y: f32| origin + gpui_kit::point(px(x), px(y));

    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        for element in &row.elements {
            let color = lane_color(element.color_id);
            let x1 = LANE_WIDTH * element.pos as f32 + element_center;
            let y1 = row_center;
            match element.kind {
                PrintKind::Node => {}
                PrintKind::Edge { other_pos, dir, arrow } => {
                    let down = dir == EdgeDir::Down;
                    if other_pos == element.pos {
                        let y2 = if down { ROW_HEIGHT } else { 0.0 };
                        line(window, point(x1, y1), point(x1, y2), color, element.dashed());
                        if arrow {
                            arrow_head(window, (x1, y1), (x1, y2), point, color);
                        }
                    } else {
                        // twice as long as the half-row so neighbouring rows dock
                        let x2 = LANE_WIDTH * other_pos as f32 + element_center;
                        let y2 = if down { ROW_HEIGHT + row_center } else { row_center - ROW_HEIGHT };
                        line(window, point(x1, y1), point(x2, y2), color, element.dashed());
                        if arrow {
                            arrow_head(window, (x1, y1), ((x1 + x2) / 2.0, (y1 + y2) / 2.0), point, color);
                        }
                    }
                }
                PrintKind::Terminal { dir } => {
                    let gap = CIRCLE_RADIUS / 2.0 + 1.0;
                    let y2 = if dir == EdgeDir::Down { ROW_HEIGHT - gap } else { gap };
                    line(window, point(x1, y1), point(x1, y2), color, element.dashed());
                    arrow_head(window, (x1, y1), (x1, y2), point, color);
                }
            }
        }
        for element in row.elements.iter().filter(|e| e.kind == PrintKind::Node) {
            let color = lane_color(element.color_id);
            let center = point(LANE_WIDTH * element.pos as f32 + element_center, row_center);
            if row.is_head {
                // outer ring in the node color, a gap in the background, then the node
                let outer = CIRCLE_RADIUS + HEAD_RADIUS_DELTA;
                circle(window, center, outer, color);
                circle(window, center, outer - HEAD_RADIUS_DELTA / 2.0 - 0.5, row.background);
                circle(window, center, CIRCLE_RADIUS - 1.0, color);
            } else {
                circle(window, center, CIRCLE_RADIUS, color);
            }
        }
    });
}

fn line(window: &mut Window, from: Point<Pixels>, to: Point<Pixels>, color: Hsla, dashed: bool) {
    if dashed {
        dashed_line(window, from, to, color);
        return;
    }
    let mut path = PathBuilder::stroke(px(LINE));
    path.move_to(from);
    path.line_to(to);
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

fn dashed_line(window: &mut Window, from: Point<Pixels>, to: Point<Pixels>, color: Hsla) {
    let (dx, dy) = (f32::from(to.x - from.x), f32::from(to.y - from.y));
    let len = dx.hypot(dy);
    if len == 0.0 {
        return;
    }
    let (dash, gap) = (3.0, 3.0);
    let mut t = 0.0;
    let mut path = PathBuilder::stroke(px(LINE));
    while t < len {
        let end = (t + dash).min(len);
        path.move_to(from + gpui_kit::point(px(dx * t / len), px(dy * t / len)));
        path.line_to(from + gpui_kit::point(px(dx * end / len), px(dy * end / len)));
        t += dash + gap;
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

/// Two short strokes at `tip`, pointing away from `from` (IntelliJ `rotate`).
fn arrow_head(
    window: &mut Window,
    from: (f32, f32),
    tip: (f32, f32),
    point: impl Fn(f32, f32) -> Point<Pixels>,
    color: Hsla,
) {
    let (tx, ty) = (from.0 - tip.0, from.1 - tip.1);
    let d = tx.hypot(ty);
    if d == 0.0 {
        return;
    }
    let length = ARROW_LENGTH * ROW_HEIGHT;
    let (sx, sy) = (length * tx / d, length * ty / d);
    let cos = ARROW_ANGLE_COS2.sqrt();
    let sin = (1.0 - ARROW_ANGLE_COS2).sqrt();
    for sin in [sin, -sin] {
        let (rx, ry) = (sx * cos - sy * sin, sx * sin + sy * cos);
        let mut path = PathBuilder::stroke(px(LINE));
        path.move_to(point(tip.0, tip.1));
        path.line_to(point(tip.0 + rx, tip.1 + ry));
        if let Ok(path) = path.build() {
            window.paint_path(path, color);
        }
    }
}

fn circle(window: &mut Window, center: Point<Pixels>, radius: f32, color: Hsla) {
    let r = px(radius);
    let bounds = Bounds::new(center - gpui_kit::point(r, r), size(r * 2.0, r * 2.0));
    window.paint_quad(fill(bounds, color).corner_radii(r));
}
