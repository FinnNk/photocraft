use super::damage::layer_bounds;
use super::moves::{move_layers, with_links};
use super::selection::{selected, top_level};
use super::*;

/// The bounds Align and Distribute move by, in a document of bounds `canvas`: [`layer_bounds`],
/// except that a shape layer is measured over everything it draws, beyond the canvas too (its
/// cache is clipped to the canvas, #8660), by rendering it again. Only those commands pay for that
/// render, once per click; `canvas` is the frame a shape's gradients are laid out in, so an opacity
/// stop measures as it renders.
pub(crate) fn align_bounds(l: &Layer, canvas: Rect) -> Option<Rect> {
    let r = match &l.content {
        LayerContent::Group(g) => g.children.iter().filter_map(|c| align_bounds(c, canvas)).fold(Rect::EMPTY, |a, b| a.union(&b)),
        LayerContent::Shape(sh) => shape_pixel_bounds(sh, canvas).unwrap_or_else(|| {
            let cached = l.surface().map(|s| s.content_bounds()).unwrap_or(Rect::EMPTY);
            // A fill that shows everywhere covers the canvas, rendered yet or not.
            if cached.is_empty() && unbounded_fill(sh) { canvas } else { cached }
        }),
        _ => return layer_bounds(l),
    };
    (!r.is_empty()).then_some(r)
}

/// Whether the shape's fill shows everywhere (an inverted path, or below 100 % density), so it has
/// no extent of its own.
fn unbounded_fill(sh: &photocraft_doc::ShapeLayer) -> bool {
    sh.fill.is_some() && (sh.path.inverted || (sh.density.is_finite() && sh.density < 1.0))
}

/// Largest area [`shape_pixel_bounds`] renders to find a shape's extent; beyond it the path and
/// stroke bounds stand in.
const SHAPE_BOUNDS_MAX_PX: u64 = 64 << 20;

/// The pixels a shape layer draws, wherever they fall. Its cache is rendered clipped to the
/// canvas, so a shape partly off the canvas measured from it looked smaller and Align moved it
/// by the wrong amount, a little more on each click (#8660). The shape is rendered again over
/// everything it can draw (its path, stroke and feather), unclipped, and measured from that. A fill
/// that shows everywhere (an inverted path, or below 100 % density) has no extent of its own and
/// keeps the canvas-clipped measure (`None`).
pub(super) fn shape_pixel_bounds(sh: &photocraft_doc::ShapeLayer, canvas: Rect) -> Option<Rect> {
    if unbounded_fill(sh) {
        return None;
    }
    let reach = photocraft_compose::shape_bounds(sh)?.inflate(1);
    if u64::from(reach.width()) * u64::from(reach.height()) > SHAPE_BOUNDS_MAX_PX {
        return photocraft_compose::hard_shape_bounds(sh);
    }
    // Gradients lay out in the document's frame (as the cached render does); the feather and the
    // clip cover the shape's whole reach, beyond the canvas too.
    let shape = photocraft_vector::CompiledShape::with_frames(sh, photocraft_vector::DEFAULT_TOLERANCE, reach, canvas);
    let r = shape.render(PixelFormat::GRAYA8, reach).content_bounds();
    (!r.is_empty()).then_some(r)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Edge {
    Top,
    VCenter,
    Bottom,
    Left,
    HCenter,
    Right,
}

impl Edge {
    fn parse(k: &str) -> Option<Edge> {
        Some(match k {
            "topEdges" => Edge::Top,
            "verticalCenters" => Edge::VCenter,
            "bottomEdges" => Edge::Bottom,
            "leftEdges" => Edge::Left,
            "horizontalCenters" => Edge::HCenter,
            "rightEdges" => Edge::Right,
            _ => return None,
        })
    }
    fn vertical(self) -> bool {
        matches!(self, Edge::Top | Edge::VCenter | Edge::Bottom)
    }
    /// The edge's coordinate, doubled so centres stay integral.
    fn value2(self, r: &Rect) -> i64 {
        let (a, b) = if self.vertical() { (r.y0, r.y1) } else { (r.x0, r.x1) };
        match self {
            Edge::Top | Edge::Left => 2 * a as i64,
            Edge::Bottom | Edge::Right => 2 * b as i64,
            Edge::VCenter | Edge::HCenter => a as i64 + b as i64,
        }
    }
    fn delta(self, d: i32) -> (i32, i32) {
        if self.vertical() { (0, d) } else { (d, 0) }
    }
}

/// Selected top-level layers that have content bounds, bottom-to-top.
fn bounded_selection(doc: &Document, sel: &[LayerId]) -> Vec<(LayerId, Rect)> {
    top_level(doc, sel).into_iter().filter_map(|id| Some((id, align_bounds(doc.layer(id)?, doc.bounds())?))).collect()
}

pub(super) fn align(s: &mut Session, p: &Value, kind: &str) -> Result<Value> {
    let cmd = format!("layer.align.{kind}");
    let edge = Edge::parse(kind).ok_or_else(|| bad(&cmd, "unknown edge"))?;
    let sel = selected(s);
    let to = p.get("to").and_then(Value::as_str).unwrap_or("auto").to_string();
    let before = s.active().ok_or(EngineError::NoDocument)?.doc.clone();
    let moved = s.edit("Align", |doc, _| {
        let items = bounded_selection(doc, &sel);
        if items.is_empty() {
            return Err(EngineError::Other("the selected layers have no content to align".into()));
        }
        let target = match to.as_str() {
            "canvas" => doc.bounds(),
            "selection" => doc.selection.as_ref().map(|m| m.content_bounds()).ok_or_else(|| EngineError::Other("there is no selection to align to".into()))?,
            "auto" | "layers" => match (&doc.selection, sel.len()) {
                // Photoshop: with an active selection, the layers align to its bounds (Layer › Align
                // Layers to Selection), however many are selected.
                (Some(m), _) if to == "auto" && !m.content_bounds().is_empty() => m.content_bounds(),
                // One layer with no selection aligns to the canvas (otherwise it would align to
                // itself), or to the artboard it sits in, as in Photoshop (#7945).
                (None, 1) if to == "auto" => items.first().map_or_else(|| doc.bounds(), |(id, _)| board_or_canvas(doc, *id)),
                _ => items.iter().fold(Rect::EMPTY, |a, (_, b)| a.union(b)),
            },
            other => return Err(bad(&cmd, format!("`to` must be auto|layers|selection|canvas, not `{other}`"))),
        };
        if target.is_empty() {
            return Err(EngineError::Other("nothing to align to".into()));
        }
        let t2 = edge.value2(&target);
        let moves: Vec<(LayerId, i32, i32)> = items
            .iter()
            .flat_map(|(id, r)| {
                let d = (t2 - edge.value2(r)).div_euclid(2) as i32;
                let (dx, dy) = edge.delta(d);
                // Linked layers travel with the layer they are linked to.
                with_links(doc, &[*id]).into_iter().map(move |l| (l, dx, dy))
            })
            .collect();
        let moves = dedup_moves(doc, moves);
        move_layers(doc, &before, &moves)?;
        Ok(moves.iter().filter(|m| m.1 != 0 || m.2 != 0).count())
    })?;
    Ok(json!({"moved": moved}))
}

/// The bounds a lone layer aligns to: the artboard holding it, else the canvas. An artboard
/// itself aligns to the canvas.
fn board_or_canvas(doc: &Document, id: LayerId) -> Rect {
    doc.artboard_of(id).filter(|board| *board != id).and_then(|board| doc.layer(board)?.artboard().map(|a| a.rect)).unwrap_or_else(|| doc.bounds())
}

/// One move per layer (first wins), skipping layers nested in another moved layer.
fn dedup_moves(doc: &Document, moves: Vec<(LayerId, i32, i32)>) -> Vec<(LayerId, i32, i32)> {
    let mut out: Vec<(LayerId, i32, i32)> = Vec::new();
    for m in moves {
        if !out.iter().any(|o| o.0 == m.0) {
            out.push(m);
        }
    }
    let ids: Vec<LayerId> = out.iter().map(|m| m.0).collect();
    let keep = top_level(doc, &ids);
    out.retain(|m| keep.contains(&m.0));
    out
}

pub(super) fn distribute(s: &mut Session, kind: &str) -> Result<Value> {
    let sel = selected(s);
    let kind = kind.to_string();
    let before = s.active().ok_or(EngineError::NoDocument)?.doc.clone();
    let moved = s.edit("Distribute", |doc, _| {
        let mut items = bounded_selection(doc, &sel);
        if items.len() < 3 {
            return Err(EngineError::Other("distributing needs three or more layers with content".into()));
        }
        let n = items.len();
        let mut moves = Vec::new();
        match kind.as_str() {
            "horizontally" | "vertically" => {
                // Equal gaps between neighbours; the outermost layers stay put.
                let v = kind == "vertically";
                let lo = |r: &Rect| if v { r.y0 } else { r.x0 } as i64;
                let hi = |r: &Rect| if v { r.y1 } else { r.x1 } as i64;
                items.sort_by_key(|(_, r)| (lo(r) + hi(r), lo(r)));
                let span = hi(&items[n - 1].1) - lo(&items[0].1);
                let total: i64 = items.iter().map(|(_, r)| hi(r) - lo(r)).sum();
                let gap = (span - total) as f64 / (n - 1) as f64;
                let mut cursor = lo(&items[0].1) as f64;
                for (id, r) in &items {
                    let want = cursor.round() as i64;
                    let d = (want - lo(r)) as i32;
                    moves.push(if v { (*id, 0, d) } else { (*id, d, 0) });
                    cursor += (hi(r) - lo(r)) as f64 + gap;
                }
            }
            k => {
                let edge = Edge::parse(k).ok_or_else(|| bad("layer.distribute", format!("unknown kind `{k}`")))?;
                items.sort_by_key(|(_, r)| edge.value2(r));
                let (first, last) = (edge.value2(&items[0].1) as f64, edge.value2(&items[n - 1].1) as f64);
                for (i, (id, r)) in items.iter().enumerate() {
                    let want2 = first + (last - first) * i as f64 / (n - 1) as f64;
                    let d = ((want2 - edge.value2(r) as f64) / 2.0).round() as i32;
                    let (dx, dy) = edge.delta(d);
                    moves.push((*id, dx, dy));
                }
            }
        }
        let moves: Vec<_> = moves.into_iter().flat_map(|(id, dx, dy)| with_links(doc, &[id]).into_iter().map(move |l| (l, dx, dy))).collect();
        let moves = dedup_moves(doc, moves);
        move_layers(doc, &before, &moves)?;
        Ok(moves.iter().filter(|m| m.1 != 0 || m.2 != 0).count())
    })?;
    Ok(json!({"moved": moved}))
}
