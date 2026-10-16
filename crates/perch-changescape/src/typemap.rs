//! Type maps: the layout and drawing of a `types` picture as one SVG
//! document, which Perch draws with usvg.
//!
//! The page gives no positions. This module lays a map out in layers,
//! from left to right, and draws it with the colors and sizes of the
//! Changescape viewer. A map that cannot be drawn can be read as the
//! plain list that `typemap_fallback` gives.

use std::collections::BTreeMap;
use std::collections::{HashMap, VecDeque};

use crate::page::Svg;
use crate::v4::{FieldRowStatus, FieldsItem, NodesItem, Relation, Status, TypeEdgeStatus, TypeMap};

/// The longest type a row shows, in characters. A longer one is cut.
const TYPE_MAX: usize = 36;
/// Text is set in a monospace face, so a character is 0.6 em wide.
const CHAR_EM: f64 = 0.6;
const MONO: &str = "Menlo, monospace";
const TITLE_SIZE: f64 = 12.5;
const FACTS_SIZE: f64 = 11.0;
const KICKER_SIZE: f64 = 10.5;
/// The padding inside a box, and the height of a row.
const PAD: f64 = 10.0;
const LINE: f64 = 18.0;
/// The height of the small kicker or facts line.
const SMALL_LINE: f64 = 15.0;
const MIN_W: f64 = 120.0;
/// The gap between two columns, where the edges run.
const COL_GAP: f64 = 60.0;
/// The gap between two boxes in one column.
const NODE_GAP: f64 = 22.0;
/// The space around the whole drawing.
const MARGIN: f64 = 16.0;
/// How many times the barycenter order is swept.
const SWEEPS: usize = 4;
/// The space a horizontal run keeps from a box it passes above or below.
const CHANNEL_CLEAR: f64 = 6.0;
/// The steps in which a run looks for a clear height, and how many it tries.
const CHANNEL_STEP: f64 = 4.0;
const CHANNEL_TRIES: usize = 200;
/// The distance under the boxes of the first lane, and the step between lanes.
const LANE_GAP: f64 = 16.0;
const LANE_STEP: f64 = 6.0;
/// The length of an arrowhead, and the room a tag keeps around its arrow.
const ARROW_LEN: f64 = 9.0;
const TAG_ROOM: f64 = 16.0;

// The colors of the viewer's light theme, since usvg takes no CSS tokens.
const CARD: &str = "#ffffff";
const INK: &str = "#24272c";
const MUTED: &str = "#5d636b";
const FAINT: &str = "#8f949b";
const RULE: &str = "#d8d5cc";
const ADDED: &str = "#2f7a3a";
const REMOVED: &str = "#b0392c";
const CHANGED: &str = "#9a6400";

/// What a drawn line or a fallback line says, by its color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Added,
    Removed,
    Changed,
    Context,
}

/// One line of the plain list that replaces a map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FallbackLine {
    pub text: String,
    pub tone: Tone,
}

/// A type map as one SVG document. A node with an element has its box
/// group's id in `hot`, and a row cut short has its full text in `tips`.
pub fn typemap_svg(map: &TypeMap) -> Svg {
    let lay = layout(map);
    let mut out = String::new();
    let mut hot = BTreeMap::new();
    let mut tips = BTreeMap::new();
    out.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">",
        w = num(lay.width),
        h = num(lay.height),
    ));
    out.push_str(&format!(
        "<rect x=\"0\" y=\"0\" width=\"{}\" height=\"{}\" fill=\"{CARD}\"/>",
        num(lay.width),
        num(lay.height),
    ));

    for (edge, route) in map.edges.iter().zip(&lay.routes) {
        let Some(route) = route else { continue };
        let tone = edge_tone(edge.status);
        let d = route
            .points
            .iter()
            .enumerate()
            .map(|(i, (x, y))| format!("{}{} {}", if i == 0 { "M" } else { "L" }, num(*x), num(*y)))
            .collect::<Vec<_>>()
            .join(" ");
        out.push_str(&format!(
            "<path d=\"{d}\" fill=\"none\" stroke=\"{}\" stroke-width=\"1.4\"{}/>",
            color(tone),
            dash(tone),
        ));
        if let Some(points) = arrow(&route.points) {
            let hollow = matches!(edge.relation, Relation::Extends | Relation::Implements);
            let (fill, stroke) = if hollow {
                (CARD, color(tone))
            } else {
                (color(tone), color(tone))
            };
            let [tip, a, b] = points;
            out.push_str(&format!(
                "<path d=\"M{} {} L{} {} L{} {} Z\" fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"1.2\"/>",
                num(tip.0), num(tip.1), num(a.0), num(a.1), num(b.0), num(b.1),
            ));
        }
        if let (Relation::OneOf, Some(tag), Some(label)) = (edge.relation, &edge.tag, route.label) {
            out.push_str(&text(
                label.x,
                label.y,
                MONO,
                FACTS_SIZE,
                " text-anchor=\"end\"",
                MUTED,
                tag,
            ));
        }
    }

    for (i, node) in lay.nodes.iter().enumerate() {
        let group_id = node.element.as_ref().map(|element| {
            let id = format!("tm-node-{i}");
            hot.insert(id.clone(), element.clone());
            format!(" id=\"{id}\"")
        });
        out.push_str(&format!("<g{}>", group_id.unwrap_or_default()));
        let tone = status_tone(node.status);
        let rx = if node.kind == Kind::Union { 12.0 } else { 3.0 };
        out.push_str(&format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{rx}\" fill=\"{CARD}\" stroke=\"{}\" stroke-width=\"1.3\"{}/>",
            num(node.x),
            num(node.y),
            num(node.width),
            num(node.height),
            color(tone),
            dash(tone),
        ));
        if let Some(kicker) = node.kicker {
            out.push_str(&text(
                node.x + PAD,
                node.y + PAD + 11.0,
                MONO,
                KICKER_SIZE,
                "",
                MUTED,
                kicker,
            ));
        }
        out.push_str(&text(
            node.x + PAD,
            node.y + node.top + 13.0,
            MONO,
            TITLE_SIZE,
            " font-weight=\"bold\"",
            INK,
            &node.title,
        ));
        if let Some(facts) = &node.facts {
            out.push_str(&text(
                node.x + PAD,
                node.y + node.top + LINE + 11.0,
                MONO,
                FACTS_SIZE,
                "",
                MUTED,
                facts,
            ));
        }
        if !node.rows.is_empty() {
            out.push_str(&format!(
                "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{RULE}\" stroke-width=\"1\"/>",
                num(node.x),
                num(node.y + node.hdr),
                num(node.x + node.width),
                num(node.y + node.hdr),
            ));
        }
        for (j, row) in node.rows.iter().enumerate() {
            let row_id = if row.cut() {
                let id = format!("tm-row-{i}-{j}");
                tips.insert(id.clone(), row.full.clone());
                format!(" id=\"{id}\"")
            } else {
                String::new()
            };
            let x = node.x + PAD;
            let y = node.row_y(j);
            out.push_str(&text(
                x,
                y + 4.0,
                MONO,
                TITLE_SIZE,
                &row_id,
                text_color(row.tone),
                &row.text,
            ));
            if row.tone == Tone::Removed {
                out.push_str(&format!(
                    "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{REMOVED}\" stroke-width=\"1\"/>",
                    num(x),
                    num(y),
                    num(x + text_width(&row.text, TITLE_SIZE)),
                    num(y),
                ));
            }
        }
        out.push_str("</g>");
    }
    out.push_str("</svg>");
    Svg {
        svg: out,
        hot,
        tips,
    }
}

/// The plain list of a map: each node with its rows, then each edge as
/// `source.field → target`, colored by its status.
pub fn typemap_fallback(map: &TypeMap) -> Vec<FallbackLine> {
    let nodes: Vec<Boxed> = map.nodes.iter().map(Boxed::new).collect();
    let names: HashMap<&str, &str> = nodes
        .iter()
        .map(|node| (node.id.as_str(), node.name.as_str()))
        .collect();
    let mut out = Vec::new();
    for node in &nodes {
        let head = match node.kicker {
            Some(kicker) => format!("{kicker} {}", node.title),
            None => node.title.clone(),
        };
        let mut text = format!("{head} ({})", status_word(node.status));
        if let Some(facts) = &node.facts {
            text.push_str(" · ");
            text.push_str(facts);
        }
        out.push(FallbackLine {
            text,
            tone: status_tone(node.status),
        });
        for row in &node.rows {
            out.push(FallbackLine {
                text: format!("  {}", row.full),
                tone: row.tone,
            });
        }
    }
    for edge in &map.edges {
        let source = names
            .get(edge.source.as_str())
            .copied()
            .unwrap_or(&edge.source);
        let target = names
            .get(edge.target.as_str())
            .copied()
            .unwrap_or(&edge.target);
        let from = match &edge.field {
            Some(field) => format!("{source}.{field}"),
            None => source.to_string(),
        };
        let detail = match &edge.tag {
            Some(tag) => format!("{} {tag}", relation_word(edge.relation)),
            None => relation_word(edge.relation).to_string(),
        };
        out.push(FallbackLine {
            text: format!("{from} → {target} ({detail})"),
            tone: edge_tone(edge.status),
        });
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Record,
    Union,
    Interface,
}

/// One row of a box: its drawn text, its full text, and its color. A
/// field row has a name, which an edge's `field` names.
#[derive(Debug, Clone)]
struct Row {
    name: Option<String>,
    text: String,
    full: String,
    tone: Tone,
}

impl Row {
    fn cut(&self) -> bool {
        self.text != self.full
    }
}

fn field_row(item: &FieldsItem) -> Row {
    match item {
        FieldsItem::Field {
            name,
            status,
            type_,
        } => Row {
            name: Some(name.clone()),
            text: format!("{name}: {}", short_type(type_)),
            full: format!("{name}: {type_}"),
            tone: match status {
                FieldRowStatus::Added => Tone::Added,
                FieldRowStatus::Removed => Tone::Removed,
                FieldRowStatus::Context => Tone::Context,
            },
        },
        FieldsItem::Retyped {
            name,
            old_type,
            type_,
        } => Row {
            name: Some(name.clone()),
            text: format!("{name}: {} → {}", short_type(old_type), short_type(type_)),
            full: format!("{name}: {old_type} → {type_}"),
            tone: Tone::Changed,
        },
        FieldsItem::Folded {
            added,
            removed,
            retyped,
            same,
        } => {
            let text = folded_text(*added, *removed, *retyped, *same);
            let changed = added + removed + retyped > 0;
            Row {
                name: None,
                full: text.clone(),
                text,
                tone: if changed {
                    Tone::Changed
                } else {
                    Tone::Context
                },
            }
        }
    }
}

/// A type cut in the middle to `TYPE_MAX` characters. Its start and its
/// end stay, with "…" at the cut.
fn short_type(t: &str) -> String {
    let chars: Vec<char> = t.chars().collect();
    if chars.len() <= TYPE_MAX {
        return t.to_string();
    }
    let head = TYPE_MAX / 2;
    let tail = TYPE_MAX - 1 - head;
    let start: String = chars[..head].iter().collect();
    let end: String = chars[chars.len() - tail..].iter().collect();
    format!("{start}…{end}")
}

/// The text of a folded row: the nonzero counts of the changed fields,
/// then the count of the same ones.
fn folded_text(added: i64, removed: i64, retyped: i64, same: i64) -> String {
    let mut marks = Vec::new();
    if added > 0 {
        marks.push(format!("+{added}"));
    }
    if removed > 0 {
        marks.push(format!("−{removed}"));
    }
    if retyped > 0 {
        marks.push(format!("~{retyped}"));
    }
    let changed = if marks.is_empty() {
        String::new()
    } else {
        format!("{} fields", marks.join(" "))
    };
    let same = if same > 0 {
        format!("{same} more")
    } else {
        String::new()
    };
    let text: Vec<String> = [changed, same]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect();
    if text.is_empty() {
        "no fields".to_string()
    } else {
        text.join(", ")
    }
}

/// A node of the map, read and measured. Its position is set by the layout.
#[derive(Debug, Clone)]
struct Boxed {
    id: String,
    name: String,
    element: Option<String>,
    status: Status,
    kind: Kind,
    kicker: Option<&'static str>,
    /// The name, or "one of" and the name for a union.
    title: String,
    facts: Option<String>,
    rows: Vec<Row>,
    width: f64,
    height: f64,
    /// The y of the title line, from the top of the box.
    top: f64,
    /// The y of the first row, from the top of the box.
    hdr: f64,
    x: f64,
    y: f64,
}

impl Boxed {
    fn new(item: &NodesItem) -> Boxed {
        let (id, name, element, status, kind, was, kind_change, hidden, rows) = match item {
            NodesItem::Record {
                id,
                name,
                element,
                status,
                was,
                kind_change,
                hidden,
                fields,
            } => (
                id,
                name,
                element,
                *status,
                Kind::Record,
                was.as_deref(),
                kind_change.as_deref(),
                *hidden,
                fields.iter().map(field_row).collect::<Vec<Row>>(),
            ),
            NodesItem::Union {
                id,
                name,
                element,
                status,
                was,
                hidden,
            } => (
                id,
                name,
                element,
                *status,
                Kind::Union,
                was.as_deref(),
                None,
                *hidden,
                Vec::new(),
            ),
            NodesItem::Interface {
                id,
                name,
                element,
                status,
                was,
                hidden,
            } => (
                id,
                name,
                element,
                *status,
                Kind::Interface,
                was.as_deref(),
                None,
                *hidden,
                Vec::new(),
            ),
        };
        let (kicker, title) = match kind {
            Kind::Union => (None, format!("one of {name}")),
            Kind::Interface => (Some("interface"), name.clone()),
            Kind::Record => (None, name.clone()),
        };
        let mut facts = Vec::new();
        if let Some(was) = was {
            facts.push(format!("was {was}"));
        }
        if let Some(change) = kind_change {
            facts.push(change.to_string());
        }
        if hidden > 0 {
            facts.push(format!("+{hidden} types"));
        }
        let mut node = Boxed {
            id: id.clone(),
            name: name.clone(),
            element: element.clone(),
            status,
            kind,
            kicker,
            title,
            facts: (!facts.is_empty()).then(|| facts.join(" · ")),
            rows,
            width: 0.0,
            height: 0.0,
            top: 0.0,
            hdr: 0.0,
            x: 0.0,
            y: 0.0,
        };
        node.measure();
        node
    }

    /// The size of the box, from the widest of its lines.
    fn measure(&mut self) {
        self.top = PAD
            + if self.kicker.is_some() {
                SMALL_LINE
            } else {
                0.0
            };
        let facts_h = if self.facts.is_some() {
            SMALL_LINE
        } else {
            0.0
        };
        self.hdr = self.top + LINE + facts_h + 4.0;
        let mut lines: Vec<(&str, f64)> = vec![(self.title.as_str(), TITLE_SIZE)];
        if let Some(kicker) = self.kicker {
            lines.push((kicker, KICKER_SIZE));
        }
        if let Some(facts) = &self.facts {
            lines.push((facts.as_str(), FACTS_SIZE));
        }
        for row in &self.rows {
            lines.push((row.text.as_str(), TITLE_SIZE));
        }
        self.width = lines
            .iter()
            .map(|(text, size)| text_width(text, *size) + 2.0 * PAD)
            .fold(MIN_W, f64::max);
        self.height = self.hdr + self.rows.len() as f64 * LINE + PAD;
    }

    /// The y of row `i`, at its middle.
    fn row_y(&self, i: usize) -> f64 {
        self.y + self.hdr + i as f64 * LINE + LINE / 2.0
    }
}

/// The y where an edge leaves a node: its named row, or the middle.
fn port_y(node: &Boxed, field: Option<&str>) -> f64 {
    field
        .and_then(|f| node.rows.iter().position(|r| r.name.as_deref() == Some(f)))
        .map_or(node.y + node.height / 2.0, |i| node.row_y(i))
}

/// Where each node and each edge is, in the drawing's own units.
#[derive(Debug, Clone)]
struct Layout {
    nodes: Vec<Boxed>,
    /// Per edge of the map, its route. None when an end is not a node.
    routes: Vec<Option<Route>>,
    width: f64,
    height: f64,
}

#[derive(Debug, Clone)]
struct Route {
    points: Vec<(f64, f64)>,
    /// The tag of a one_of edge, where its text goes.
    label: Option<Label>,
}

/// Where the text of a tag goes. The text ends at `x` when `end` is set, and
/// starts there otherwise. `width` is the width of the text.
#[derive(Debug, Clone, Copy)]
struct Label {
    x: f64,
    y: f64,
    width: f64,
}

fn layout(map: &TypeMap) -> Layout {
    let mut nodes: Vec<Boxed> = map.nodes.iter().map(Boxed::new).collect();
    let n = nodes.len();
    let mut index: HashMap<&str, usize> = HashMap::new();
    for (i, node) in nodes.iter().enumerate() {
        index.entry(node.id.as_str()).or_insert(i);
    }
    let ends: Vec<Option<(usize, usize)>> = map
        .edges
        .iter()
        .map(|e| {
            Some((
                *index.get(e.source.as_str())?,
                *index.get(e.target.as_str())?,
            ))
        })
        .collect();

    // Drop the back edges found by a depth-first search in node order, so
    // the layers come from a graph with no cycle.
    let mut out: Vec<Vec<(usize, usize)>> = vec![Vec::new(); n];
    for (k, end) in ends.iter().enumerate() {
        if let Some((u, v)) = *end {
            out[u].push((k, v));
        }
    }
    let mut back = vec![false; ends.len()];
    let mut state = vec![0u8; n];
    for start in 0..n {
        if state[start] == 0 {
            mark_back(start, &out, &mut state, &mut back);
        }
    }

    // Layer by longest path from the sources: the column of a node.
    let mut fwd: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut indeg = vec![0usize; n];
    for (k, end) in ends.iter().enumerate() {
        if let (Some((u, v)), false) = (*end, back[k]) {
            fwd[u].push(v);
            indeg[v] += 1;
        }
    }
    let mut layer = vec![0usize; n];
    let mut queue: VecDeque<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
    while let Some(u) = queue.pop_front() {
        for &v in &fwd[u] {
            layer[v] = layer[v].max(layer[u] + 1);
            indeg[v] -= 1;
            if indeg[v] == 0 {
                queue.push_back(v);
            }
        }
    }
    let ncols = layer.iter().map(|l| l + 1).max().unwrap_or(0);

    // Order each column by barycenters of its neighbours, starting from
    // the given order. Ties keep their current order.
    let mut cols: Vec<Vec<usize>> = vec![Vec::new(); ncols];
    for (i, &l) in layer.iter().enumerate() {
        cols[l].push(i);
    }
    let mut pos = vec![0usize; n];
    for col in &cols {
        for (p, &v) in col.iter().enumerate() {
            pos[v] = p;
        }
    }
    let mut nbr: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (u, v) in ends.iter().flatten() {
        if u != v {
            nbr[*u].push(*v);
            nbr[*v].push(*u);
        }
    }
    for _ in 0..SWEEPS {
        for c in 1..ncols {
            sweep(&mut cols, &mut pos, &nbr, &layer, c, c - 1);
        }
        for c in (0..ncols.saturating_sub(1)).rev() {
            sweep(&mut cols, &mut pos, &nbr, &layer, c, c + 1);
        }
    }

    // Columns: the widest box sets the width, and each stack is centred.
    let col_w: Vec<f64> = cols
        .iter()
        .map(|col| col.iter().map(|&v| nodes[v].width).fold(0.0, f64::max))
        .collect();
    let col_h: Vec<f64> = cols
        .iter()
        .map(|col| {
            col.iter().map(|&v| nodes[v].height).sum::<f64>()
                + NODE_GAP * col.len().saturating_sub(1) as f64
        })
        .collect();
    let tallest = col_h.iter().copied().fold(0.0, f64::max);
    // The gap to the right of a column is wide enough for the tags of the
    // edges that leave it, and for the tags of the edges that enter the next
    // column, since each tag sits in the gap before its target.
    let gap_w = tag_gaps(map, &ends, &layer, ncols);
    let mut col_x = vec![0.0; ncols];
    let mut x = MARGIN;
    for c in 0..ncols {
        col_x[c] = x;
        x += col_w[c] + gap_w[c];
    }
    for (c, col) in cols.iter().enumerate() {
        let mut y = MARGIN + (tallest - col_h[c]) / 2.0;
        for &v in col {
            nodes[v].x = col_x[c];
            nodes[v].y = y;
            y += nodes[v].height + NODE_GAP;
        }
    }

    // Edges. A forward edge leaves its source by the gap after the source
    // column and enters its target by the gap before the target column. A
    // back edge, or one in the same column, goes round under all the boxes.
    // Each vertical run lies in one of these gaps, and each edge takes its
    // own x in every gap it uses, so no two runs share a line.
    let normal: Vec<bool> = ends
        .iter()
        .map(|end| match end {
            Some((u, v)) => layer[*v] > layer[*u],
            None => false,
        })
        .collect();
    let bottom = nodes.iter().map(|x| x.y + x.height).fold(0.0, f64::max);

    // The gaps where each edge runs its verticals: the gap after its source
    // column, and the gap before its target column. Gap -1 is the margin.
    // A run that leaves a column is set in the middle of its gap. A run that
    // enters a column is set at the left of its gap, so that the tag, which
    // sits just left of the arrowhead, does not cross its own run.
    let slot = |g: isize| (g + 1) as usize;
    let mut vgaps: Vec<Option<(isize, isize)>> = Vec::with_capacity(ends.len());
    for end in &ends {
        vgaps.push(end.map(|(u, v)| (layer[u] as isize, layer[v] as isize - 1)));
    }
    let mut leaving = vec![0usize; ncols + 1];
    let mut entering = vec![0usize; ncols + 1];
    for &(a, b) in vgaps.iter().flatten() {
        if b == a {
            entering[slot(b)] += 1;
        } else {
            leaving[slot(a)] += 1;
            entering[slot(b)] += 1;
        }
    }
    let gap_mid = |g: isize| {
        if g < 0 {
            MARGIN / 2.0
        } else {
            let c = g as usize;
            col_x[c] + col_w[c] + gap_w[c] / 2.0
        }
    };
    let mut used_leaving = vec![0usize; ncols + 1];
    let mut used_entering = vec![0usize; ncols + 1];
    let mut place_leaving = |g: isize| {
        let i = slot(g);
        let n = leaving[i] as f64;
        let s = used_leaving[i] as f64;
        used_leaving[i] += 1;
        gap_mid(g) + (s - (n - 1.0) / 2.0) * (20.0 / n).min(6.0)
    };
    let mut place_entering = |g: isize| {
        let i = slot(g);
        if g < 0 {
            let n = entering[i] as f64;
            let s = used_entering[i] as f64;
            used_entering[i] += 1;
            return MARGIN / 2.0 + (s - (n - 1.0) / 2.0) * (6.0_f64).min(10.0 / n);
        }
        let n = entering[i] as f64;
        let s = used_entering[i] as f64;
        used_entering[i] += 1;
        let c = g as usize;
        col_x[c] + col_w[c] + 6.0 + s * (10.0 / n).min(6.0)
    };
    let mut xs: Vec<(f64, f64)> = vec![(0.0, 0.0); ends.len()];
    for (k, g) in vgaps.iter().enumerate() {
        if let Some((a, b)) = *g {
            let (xa, xb) = if b == a {
                let x = place_entering(b);
                (x, x)
            } else {
                let xa = place_leaving(a);
                (xa, place_entering(b))
            };
            xs[k] = (xa, xb);
        }
    }

    let back_total = (0..ends.len())
        .filter(|&k| ends[k].is_some() && !normal[k])
        .count();
    let mut around = 0usize;
    let mut lanes = 0usize;
    let mut routes = Vec::with_capacity(ends.len());
    for (k, end) in ends.iter().enumerate() {
        let Some((u, v)) = *end else {
            routes.push(None);
            continue;
        };
        let (src, tgt) = (&nodes[u], &nodes[v]);
        let (sc, tc) = (layer[u], layer[v]);
        let exit = (
            src.x + src.width,
            port_y(src, map.edges[k].field.as_deref()),
        );
        let entry = (tgt.x, tgt.y + tgt.height / 2.0);
        let (xa, xb) = xs[k];
        let points = if normal[k] && tc == sc + 1 {
            vec![exit, (xa, exit.1), (xa, entry.1), entry]
        } else if normal[k] {
            // Across more columns: run at the first clear height, else in a
            // lane under all the boxes.
            let y = channel_y(&nodes, &layer, sc, tc, [entry.1, exit.1]).unwrap_or_else(|| {
                lanes += 1;
                bottom + LANE_GAP + LANE_STEP * (back_total + lanes - 1) as f64
            });
            vec![exit, (xa, exit.1), (xa, y), (xb, y), (xb, entry.1), entry]
        } else {
            let lane = bottom + LANE_GAP + LANE_STEP * around as f64;
            around += 1;
            vec![
                exit,
                (xa, exit.1),
                (xa, lane),
                (xb, lane),
                (xb, entry.1),
                entry,
            ]
        };
        let edge = &map.edges[k];
        let label = match (&edge.relation, edge.tag.as_deref()) {
            (Relation::OneOf, Some(tag)) => Some(tag_label(&points, text_width(tag, FACTS_SIZE))),
            _ => None,
        };
        routes.push(Some(Route { points, label }));
    }

    // Shift the drawing so its top left is at the margin.
    let mut bounds: Option<[f64; 4]> = None;
    for node in &nodes {
        grow(&mut bounds, node.x, node.y);
        grow(&mut bounds, node.x + node.width, node.y + node.height);
    }
    for route in routes.iter().flatten() {
        for &(x, y) in &route.points {
            grow(&mut bounds, x, y);
        }
        if let Some(label) = route.label {
            grow(&mut bounds, label.x - label.width, label.y - SMALL_LINE);
            grow(&mut bounds, label.x + label.width, label.y + 4.0);
        }
    }
    let (dx, dy, width, height) = match bounds {
        Some([x0, y0, x1, y1]) => (
            MARGIN - x0,
            MARGIN - y0,
            x1 - x0 + 2.0 * MARGIN,
            y1 - y0 + 2.0 * MARGIN,
        ),
        None => (0.0, 0.0, 2.0 * MARGIN, 2.0 * MARGIN),
    };
    for node in &mut nodes {
        node.x += dx;
        node.y += dy;
    }
    for route in routes.iter_mut().flatten() {
        for point in &mut route.points {
            point.0 += dx;
            point.1 += dy;
        }
        if let Some(label) = &mut route.label {
            label.x += dx;
            label.y += dy;
        }
    }
    Layout {
        nodes,
        routes,
        width,
        height,
    }
}

/// Mark the back edges below `u` in a depth-first search.
fn mark_back(u: usize, out: &[Vec<(usize, usize)>], state: &mut [u8], back: &mut [bool]) {
    state[u] = 1;
    for &(k, v) in &out[u] {
        match state[v] {
            1 => back[k] = true,
            0 => mark_back(v, out, state, back),
            _ => {}
        }
    }
    state[u] = 2;
}

/// Reorder column `c` by the mean position of its neighbours in column
/// `other`. A node with no such neighbour keeps its place.
fn sweep(
    cols: &mut [Vec<usize>],
    pos: &mut [usize],
    nbr: &[Vec<usize>],
    layer: &[usize],
    c: usize,
    other: usize,
) {
    let mut keyed: Vec<(f64, usize, usize)> = cols[c]
        .iter()
        .map(|&v| {
            let ps: Vec<usize> = nbr[v]
                .iter()
                .filter(|&&w| layer[w] == other)
                .map(|&w| pos[w])
                .collect();
            let key = if ps.is_empty() {
                pos[v] as f64
            } else {
                ps.iter().sum::<usize>() as f64 / ps.len() as f64
            };
            (key, pos[v], v)
        })
        .collect();
    keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    cols[c] = keyed.into_iter().map(|(_, _, v)| v).collect();
    for (p, &v) in cols[c].iter().enumerate() {
        pos[v] = p;
    }
}

/// Widen the bounds to hold a point.
fn grow(bounds: &mut Option<[f64; 4]>, x: f64, y: f64) {
    *bounds = Some(match *bounds {
        None => [x, y, x, y],
        Some([x0, y0, x1, y1]) => [x0.min(x), y0.min(y), x1.max(x), y1.max(y)],
    });
}

/// The three corners of an arrowhead at the end of a route: the tip, and
/// the two corners at its base.
fn arrow(points: &[(f64, f64)]) -> Option<[(f64, f64); 3]> {
    let [.., prev, tip] = points else { return None };
    let (dx, dy) = (tip.0 - prev.0, tip.1 - prev.1);
    let len = (dx * dx + dy * dy).sqrt();
    let (ux, uy) = if len > 0.0 {
        (dx / len, dy / len)
    } else {
        (1.0, 0.0)
    };
    let base = (tip.0 - ux * ARROW_LEN, tip.1 - uy * ARROW_LEN);
    let (px, py) = (-uy * 4.5, ux * 4.5);
    Some([*tip, (base.0 + px, base.1 + py), (base.0 - px, base.1 - py)])
}

/// The y of the horizontal run of a forward edge across more than one
/// column. It is the first of the preferred heights that is clear of every
/// box in the columns between the ends. Failing that, it is the nearest clear
/// height, searched outward from the first preference. None if there is none.
fn channel_y(
    nodes: &[Boxed],
    layer: &[usize],
    sc: usize,
    tc: usize,
    prefer: [f64; 2],
) -> Option<f64> {
    let clear = |y: f64| {
        nodes
            .iter()
            .zip(layer)
            .filter(|(_, &l)| l > sc && l < tc)
            .all(|(node, _)| y < node.y - CHANNEL_CLEAR || y > node.y + node.height + CHANNEL_CLEAR)
    };
    if let Some(y) = prefer.into_iter().find(|&y| clear(y)) {
        return Some(y);
    }
    (1..=CHANNEL_TRIES)
        .flat_map(|i| {
            let d = i as f64 * CHANNEL_STEP;
            [prefer[0] - d, prefer[0] + d]
        })
        .find(|&y| clear(y))
}

/// The width of each gap between columns: the room that the widest tag
/// needs, or `COL_GAP` if that is more. A tag needs its text, plus the
/// arrowhead and `TAG_ROOM`. It counts for the gap it sits in, and for the
/// gap left of the column its edge leaves.
fn tag_gaps(
    map: &TypeMap,
    ends: &[Option<(usize, usize)>],
    layer: &[usize],
    ncols: usize,
) -> Vec<f64> {
    let mut need = vec![0.0f64; ncols];
    for (edge, end) in map.edges.iter().zip(ends) {
        let (Some((u, v)), Relation::OneOf, Some(tag)) = (end, edge.relation, &edge.tag) else {
            continue;
        };
        let room = text_width(tag, FACTS_SIZE) + 2.0 * ARROW_LEN + TAG_ROOM;
        need[layer[*u]] = need[layer[*u]].max(room);
        if layer[*v] > 0 {
            need[layer[*v] - 1] = need[layer[*v] - 1].max(room);
        }
    }
    need.into_iter().map(|w| w.max(COL_GAP)).collect()
}

/// Where the tag of a one_of edge goes: just left of the arrowhead, on the
/// last run, right-aligned and a few px above the line.
fn tag_label(points: &[(f64, f64)], width: f64) -> Label {
    let entry = points[points.len() - 1];
    Label {
        x: entry.0 - ARROW_LEN - 3.0,
        y: entry.1 - 4.0,
        width,
    }
}

fn status_tone(status: Status) -> Tone {
    match status {
        Status::Added => Tone::Added,
        Status::Removed => Tone::Removed,
        Status::Changed | Status::Moved => Tone::Changed,
        Status::Context => Tone::Context,
    }
}

fn edge_tone(status: TypeEdgeStatus) -> Tone {
    match status {
        TypeEdgeStatus::Added => Tone::Added,
        TypeEdgeStatus::Removed => Tone::Removed,
        TypeEdgeStatus::Context => Tone::Context,
    }
}

fn status_word(status: Status) -> &'static str {
    match status {
        Status::Added => "added",
        Status::Removed => "removed",
        Status::Changed => "changed",
        Status::Moved => "moved",
        Status::Context => "context",
    }
}

fn relation_word(relation: Relation) -> &'static str {
    match relation {
        Relation::Holds => "holds",
        Relation::OneOf => "one_of",
        Relation::Extends => "extends",
        Relation::Implements => "implements",
    }
}

/// The stroke or fill of a tone.
fn color(tone: Tone) -> &'static str {
    match tone {
        Tone::Added => ADDED,
        Tone::Removed => REMOVED,
        Tone::Changed => CHANGED,
        Tone::Context => FAINT,
    }
}

/// The color of a row's text. A context row reads in the muted ink.
fn text_color(tone: Tone) -> &'static str {
    match tone {
        Tone::Context => MUTED,
        other => color(other),
    }
}

/// The dash of a removed line, as an attribute.
fn dash(tone: Tone) -> &'static str {
    if tone == Tone::Removed {
        " stroke-dasharray=\"5 3\""
    } else {
        ""
    }
}

fn text_width(text: &str, size: f64) -> f64 {
    text.chars().count() as f64 * size * CHAR_EM
}

/// A text element at a point. `extra` holds more attributes, such as an id.
fn text(x: f64, y: f64, font: &str, size: f64, extra: &str, fill: &str, body: &str) -> String {
    format!(
        "<text x=\"{}\" y=\"{}\" font-family=\"{font}\" font-size=\"{}\"{extra} fill=\"{fill}\">{}</text>",
        num(x),
        num(y),
        num(size),
        esc(body),
    )
}

fn num(v: f64) -> String {
    format!("{v:.1}")
}

/// Escape text for XML.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v4::{self, BlocksItem, PicturesItem};

    fn load(text: &str) -> v4::Page {
        serde_json::from_str(text).unwrap()
    }

    /// Every type map of the two conformance cases: the block of case 02,
    /// the spine of case 07, and the pictures of its stops.
    fn fixture_maps() -> Vec<TypeMap> {
        let mut out = Vec::new();
        let page = load(include_str!(
            "../tests/fixtures/conformance/02-table-groups-fold/page.json"
        ));
        for view in &page.views {
            for block in &view.blocks {
                if let BlocksItem::Typemap { map, .. } = block {
                    out.push(map.clone());
                }
            }
        }
        let page = load(include_str!(
            "../tests/fixtures/conformance/07-tour/page.json"
        ));
        for chapter in page.tour.iter().flat_map(|tour| &tour.chapters) {
            if let v4::Spine::TypeMap(map) = &chapter.spine {
                out.push(map.clone());
            }
            for stop in &chapter.stops {
                for picture in &stop.pictures {
                    if let PicturesItem::Types {
                        caption,
                        edges,
                        nodes,
                        note,
                        title,
                    } = picture
                    {
                        out.push(TypeMap {
                            caption: caption.clone(),
                            edges: edges.clone(),
                            kind: "types".to_string(),
                            nodes: nodes.clone(),
                            note: note.clone(),
                            title: title.clone(),
                        });
                    }
                }
            }
        }
        out
    }

    fn node_count(map: &TypeMap) -> usize {
        map.nodes.len()
    }

    /// A check that the markup is well formed: tags nest, and every `&`
    /// starts an entity.
    fn well_formed(xml: &str) -> bool {
        let mut stack: Vec<String> = Vec::new();
        let mut rest = xml;
        while let Some(start) = rest.find('<') {
            let Some(end) = rest[start..].find('>') else {
                return false;
            };
            let tag = &rest[start + 1..start + end];
            rest = &rest[start + end + 1..];
            if tag.starts_with('?') || tag.starts_with('!') {
                continue;
            }
            if let Some(name) = tag.strip_prefix('/') {
                if stack.pop().as_deref() != Some(name.trim()) {
                    return false;
                }
            } else if !tag.ends_with('/') {
                let name = tag.split_whitespace().next().unwrap_or("").to_string();
                stack.push(name);
            }
        }
        let entities_ok = xml.match_indices('&').all(|(i, _)| {
            let after = &xml[i + 1..];
            ["amp;", "lt;", "gt;", "quot;", "apos;", "#"]
                .iter()
                .any(|e| after.starts_with(e))
        });
        stack.is_empty() && entities_ok
    }

    #[test]
    fn fixture_maps_are_drawn_as_well_formed_svg() {
        let maps = fixture_maps();
        assert!(maps.len() >= 2);
        for map in &maps {
            let svg = typemap_svg(map);
            assert!(svg.svg.starts_with("<svg "), "{}", svg.svg);
            assert!(svg.svg.ends_with("</svg>"));
            assert!(well_formed(&svg.svg), "not well formed: {}", svg.svg);
        }
    }

    #[test]
    fn every_node_with_an_element_is_hot_and_no_other() {
        for map in fixture_maps() {
            let svg = typemap_svg(&map);
            let mut expected: Vec<&str> = map
                .nodes
                .iter()
                .filter_map(|n| match n {
                    NodesItem::Record { element, .. }
                    | NodesItem::Union { element, .. }
                    | NodesItem::Interface { element, .. } => element.as_deref(),
                })
                .collect();
            let mut got: Vec<&str> = svg.hot.values().map(String::as_str).collect();
            expected.sort();
            got.sort();
            assert_eq!(got, expected);
            for id in svg.hot.keys() {
                assert!(svg.svg.contains(&format!("id=\"{id}\"")), "{id} not drawn");
            }
        }
    }

    #[test]
    fn no_two_boxes_overlap() {
        for map in fixture_maps() {
            let lay = layout(&map);
            for (i, a) in lay.nodes.iter().enumerate() {
                for b in &lay.nodes[i + 1..] {
                    let apart = a.x + a.width <= b.x
                        || b.x + b.width <= a.x
                        || a.y + a.height <= b.y
                        || b.y + b.height <= a.y;
                    assert!(apart, "{} overlaps {}", a.id, b.id);
                }
            }
            assert_eq!(segment_through_a_box(&map, &lay), None);
        }
    }

    /// A segment of a route that is not level, or that runs through the
    /// inside of a box other than the edge's own source and target. Returns
    /// a description of the first such segment.
    fn segment_through_a_box(map: &TypeMap, lay: &Layout) -> Option<String> {
        let index = |id: &str| lay.nodes.iter().position(|n| n.id == id);
        for (edge, route) in map.edges.iter().zip(&lay.routes) {
            let Some(route) = route else { continue };
            let ends = (index(&edge.source), index(&edge.target));
            for pair in route.points.windows(2) {
                let ((ax, ay), (bx, by)) = (pair[0], pair[1]);
                if ax != bx && ay != by {
                    return Some(format!("diagonal segment {pair:?}"));
                }
                for (i, node) in lay.nodes.iter().enumerate() {
                    if Some(i) == ends.0 || Some(i) == ends.1 {
                        continue;
                    }
                    if segment_hits_box(pair[0], pair[1], node) {
                        return Some(format!("{pair:?} crosses {}", node.id));
                    }
                }
            }
        }
        None
    }

    /// Whether an axis-aligned segment runs through the inside of a box. The
    /// edge of the box does not count.
    fn segment_hits_box(a: (f64, f64), b: (f64, f64), node: &Boxed) -> bool {
        const INSET: f64 = 0.5;
        let (x0, y0) = (node.x + INSET, node.y + INSET);
        let (x1, y1) = (node.x + node.width - INSET, node.y + node.height - INSET);
        a.0.min(b.0) < x1 && a.0.max(b.0) > x0 && a.1.min(b.1) < y1 && a.1.max(b.1) > y0
    }

    /// The distance from a point to a route.
    fn distance_to_route(points: &[(f64, f64)], p: (f64, f64)) -> f64 {
        points
            .windows(2)
            .map(|w| {
                let ((ax, ay), (bx, by)) = (w[0], w[1]);
                let (dx, dy) = (bx - ax, by - ay);
                let len2 = dx * dx + dy * dy;
                let t = if len2 > 0.0 {
                    (((p.0 - ax) * dx + (p.1 - ay) * dy) / len2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let (qx, qy) = (ax + t * dx, ay + t * dy);
                ((p.0 - qx).powi(2) + (p.1 - qy).powi(2)).sqrt()
            })
            .fold(f64::INFINITY, f64::min)
    }

    /// Each one_of tag sits within 20 px of its own edge's route, inside the
    /// gap before its target, and clear of every box. Returns how many tags
    /// there are.
    fn check_tags_on_their_routes(map: &TypeMap) -> usize {
        let lay = layout(map);
        let mut tags = 0;
        for (edge, route) in map.edges.iter().zip(&lay.routes) {
            if edge.relation != Relation::OneOf {
                continue;
            }
            let (Some(tag), Some(route)) = (&edge.tag, route) else {
                continue;
            };
            let label = route.label.expect("a one_of tag has a label");
            let d = distance_to_route(&route.points, (label.x, label.y));
            assert!(d <= 20.0, "tag {tag} is {d} px from its route");
            // The text box: it ends left of the arrowhead and stays in the
            // gap before the target, clear of every box.
            let target = lay.nodes.iter().find(|n| n.id == edge.target).unwrap();
            let (x0, x1) = (label.x - label.width, label.x);
            let (y0, y1) = (label.y - 9.0, label.y + 3.0);
            let gap_left = lay
                .nodes
                .iter()
                .map(|n| n.x + n.width)
                .filter(|&r| r <= target.x + 1e-9)
                .fold(f64::NEG_INFINITY, f64::max);
            assert!(
                x0 >= gap_left - 1e-6 && x1 <= target.x,
                "tag {tag} spans {x0}..{x1}, outside its gap {gap_left}..{}",
                target.x
            );
            for node in &lay.nodes {
                let hit = x0 < node.x + node.width
                    && x1 > node.x
                    && y0 < node.y + node.height
                    && y1 > node.y;
                assert!(!hit, "tag {tag} overlaps the box {}", node.id);
            }
            tags += 1;
        }
        tags
    }

    #[test]
    fn each_one_of_tag_sits_on_its_own_route() {
        let mut tags = 0;
        for map in fixture_maps() {
            tags += check_tags_on_their_routes(&map);
        }
        assert!(tags >= 2, "the fixtures have one_of tags: {tags}");
    }

    fn field(name: &str) -> serde_json::Value {
        serde_json::json!({ "row": "field", "name": name, "status": "context", "type": "int" })
    }

    fn record(id: &str, fields: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "node": "record", "id": id, "name": id, "element": null, "status": "context",
            "was": null, "kind_change": null, "hidden": 0,
            "fields": fields.iter().map(|f| field(f)).collect::<Vec<_>>(),
        })
    }

    fn union(id: &str) -> serde_json::Value {
        serde_json::json!({
            "node": "union", "id": id, "name": id, "element": null,
            "status": "context", "was": null, "hidden": 0,
        })
    }

    fn edge(
        source: &str,
        target: &str,
        field: Option<&str>,
        relation: &str,
        tag: Option<&str>,
    ) -> serde_json::Value {
        serde_json::json!({
            "source": source, "target": target, "field": field, "relation": relation,
            "status": "context", "tag": tag,
        })
    }

    fn map_of(nodes: Vec<serde_json::Value>, edges: Vec<serde_json::Value>) -> TypeMap {
        serde_json::from_value(serde_json::json!({
            "caption": null, "kind": "types", "note": null, "title": null,
            "nodes": nodes, "edges": edges,
        }))
        .unwrap()
    }

    /// A chain A -> B -> C, with A -> C through the field `f` of A. A tall
    /// box D sits in the middle column, at the height of C's middle.
    fn skipping_map() -> TypeMap {
        map_of(
            vec![
                record("A", &["f", "g"]),
                record("B", &["h"]),
                record("C", &["x", "y", "z"]),
                record("D", &["d1", "d2", "d3", "d4", "d5", "d6", "d7", "d8", "d9"]),
            ],
            vec![
                edge("A", "B", Some("g"), "holds", None),
                edge("B", "C", Some("h"), "holds", None),
                edge("A", "C", Some("f"), "holds", None),
                edge("A", "D", None, "holds", None),
            ],
        )
    }

    #[test]
    fn no_edge_runs_through_a_box_it_does_not_join() {
        let map = skipping_map();
        let lay = layout(&map);
        let (c, d) = (&lay.nodes[2], &lay.nodes[3]);
        let c_mid = c.y + c.height / 2.0;
        assert!(
            d.x > lay.nodes[0].x && d.x < c.x,
            "D is in the middle column"
        );
        assert!(
            d.y < c_mid && c_mid < d.y + d.height,
            "D sits at the height of C's middle, so the first choice is blocked"
        );
        assert_eq!(segment_through_a_box(&map, &lay), None);
    }

    #[test]
    fn each_one_of_tag_of_a_synthetic_map_sits_on_its_route() {
        let map = map_of(
            vec![union("U"), record("X", &["p"]), record("Z", &["q"])],
            vec![
                edge("U", "X", None, "one_of", Some("kind=entry")),
                edge("X", "Z", Some("p"), "holds", None),
                edge("U", "Z", None, "one_of", Some("kind=legacy-long-tag")),
            ],
        );
        assert_eq!(check_tags_on_their_routes(&map), 2);
        let lay = layout(&map);
        assert_eq!(segment_through_a_box(&map, &lay), None);
    }

    #[test]
    fn every_edge_has_a_path() {
        for map in fixture_maps() {
            let lay = layout(&map);
            assert_eq!(lay.routes.len(), map.edges.len());
            assert!(lay.routes.iter().all(Option::is_some));
        }
    }

    #[test]
    fn the_same_map_gives_the_same_svg_twice() {
        for map in fixture_maps() {
            let a = typemap_svg(&map);
            let b = typemap_svg(&map);
            assert_eq!(a.svg, b.svg);
            assert_eq!(a.hot, b.hot);
            assert_eq!(a.tips, b.tips);
        }
    }

    #[test]
    fn the_fallback_has_one_line_per_edge() {
        for map in fixture_maps() {
            let lines = typemap_fallback(&map);
            let rows: usize = map
                .nodes
                .iter()
                .map(|n| match n {
                    NodesItem::Record { fields, .. } => fields.len(),
                    _ => 0,
                })
                .sum();
            assert_eq!(lines.len(), node_count(&map) + rows + map.edges.len());
            let edges = &lines[lines.len() - map.edges.len()..];
            for (line, edge) in edges.iter().zip(&map.edges) {
                assert!(line.text.contains(" → "), "{}", line.text);
                assert_eq!(line.tone, edge_tone(edge.status));
            }
        }
    }

    #[test]
    fn a_long_type_is_cut_in_the_middle() {
        assert_eq!(short_type("int"), "int");
        let exact = "a".repeat(TYPE_MAX);
        assert_eq!(short_type(&exact), exact);
        let long = format!("{}x{}", "a".repeat(18), "b".repeat(18));
        assert_eq!(
            short_type(&long),
            format!("{}…{}", "a".repeat(18), "b".repeat(17))
        );
        assert_eq!(short_type(&"é".repeat(40)).chars().count(), TYPE_MAX);
    }

    #[test]
    fn folded_row_text_counts_each_change() {
        assert_eq!(folded_text(2, 1, 3, 4), "+2 −1 ~3 fields, 4 more");
        assert_eq!(folded_text(0, 0, 0, 4), "4 more");
        assert_eq!(folded_text(1, 0, 0, 0), "+1 fields");
        assert_eq!(folded_text(0, 0, 0, 0), "no fields");
    }

    #[test]
    fn a_cut_row_keeps_its_full_text_as_a_tip() {
        let map: TypeMap = serde_json::from_value(serde_json::json!({
            "caption": null, "kind": "types", "note": null, "title": null,
            "edges": [],
            "nodes": [{ "node": "record", "id": "m.R", "name": "R", "element": "fn:m.R",
                "status": "changed", "was": null, "kind_change": null, "hidden": 0,
                "fields": [
                    { "row": "retyped", "name": "f", "old_type": "Box<dyn Epoch>",
                      "type": "Box<dyn Epoch<Manifest>>Policy<Manifest> and more" },
                    { "row": "field", "name": "g", "type": "int", "status": "context" }
                ] }],
        }))
        .unwrap();
        let svg = typemap_svg(&map);
        assert_eq!(svg.tips.len(), 1);
        let (id, full) = svg.tips.iter().next().unwrap();
        assert_eq!(id, "tm-row-0-0");
        assert_eq!(
            full,
            "f: Box<dyn Epoch> → Box<dyn Epoch<Manifest>>Policy<Manifest> and more"
        );
        assert!(svg.svg.contains("id=\"tm-row-0-0\""));
        assert!(!svg.tips.contains_key("tm-row-0-1"));
    }

    /// Writes the drawings of the two fixtures to the folder named by
    /// `TYPEMAP_SVG_OUT`, for a look by eye. Does nothing when it is unset.
    #[test]
    fn write_fixture_drawings_when_asked() {
        let Some(dir) = std::env::var_os("TYPEMAP_SVG_OUT") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let maps = fixture_maps();
        let by_block = maps.first().unwrap();
        let by_spine = maps
            .iter()
            .find(|m| m.title.as_deref() == Some("Records of the store"))
            .unwrap();
        std::fs::write(dir.join("02-overview.svg"), typemap_svg(by_block).svg).unwrap();
        std::fs::write(dir.join("07-spine.svg"), typemap_svg(by_spine).svg).unwrap();
    }
}
