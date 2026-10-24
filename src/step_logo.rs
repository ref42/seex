//! Removes the LCEDA / EasyEDA logo that EasyEDA embeds in its STEP models.
//!
//! The logo is an icon plus the words "LCEDA" and "EasyEDA", usually on one side
//! of the package, and on many models a second, mirrored "LCEDA" underneath. It
//! comes in two forms:
//!
//! * thin solids (about 1 µm) lying on the package (SOT-23 family);
//! * a relief or engraving (0.001–0.1 mm) merged into a body face. Each letter
//!   is attached to that face through an inner loop of its boundary (passives,
//!   diodes, LEDs). The face is flat, or the round side of a cylindrical body
//!   (buzzers, round capacitors).
//!
//! Only the logo is touched: part markings, pin-1 marks and cathode bands look
//! similar but are kept. A piece belongs to the logo when it is the icon (always
//! the same shape), or one of the logo letters right next to the icon, or part
//! of a five-letter "LCEDA" row. Letters are recognised by their number of side
//! faces and how many of those are curved, which does not depend on the scale.
//!
//! Thin solids are dropped. For merged letters, their faces are deleted along
//! with the inner loop of the face they sit on, which leaves that face whole.
//! The result is checked: every shell must still be closed and nothing may
//! point at a deleted entity. If anything is off, the model is kept as it was and
//! [`strip_logo`] says so, so the export can tell the user the logo is still there.

use std::collections::{HashMap, HashSet};

/// Deepest relief / engraving that can belong to the logo (mm). Round parts
/// such as buzzers have it cut 0.1 mm deep.
const MAX_RELIEF_MM: f64 = 0.12;
/// Thickest solid that can be a logo piece (mm).
const MAX_THIN_SOLID_MM: f64 = 0.005;
/// Two pieces lie in the same plane when their offsets differ by less (mm).
const PLANE_TOLERANCE_MM: f64 = 0.02;
/// Largest flood fill accepted as a relief feature (faces).
const MAX_FEATURE_FACES: usize = 400;

/// (side faces, curved side faces) of the icon.
const ICON: (usize, usize) = (35, 28);
/// Letters of "LCEDA" and "EasyEDA" next to the icon.
const LOGO_LETTERS: [(usize, usize); 8] = [
    (6, 0),
    (12, 0),
    (14, 2),
    (14, 8),
    (14, 12),
    (15, 5),
    (21, 19),
    (22, 14),
];
/// Letters of the separate "LCEDA" row (another font).
const ROW_LETTERS: [(usize, usize); 6] = [(6, 0), (10, 4), (10, 8), (11, 1), (12, 0), (14, 2)];
/// Shapes every "LCEDA" row contains (L, C, E, D).
const ROW_REQUIRED: [(usize, usize); 4] = [(6, 0), (10, 8), (12, 0), (10, 4)];

/// What [`strip_logo`] took out.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LogoRemoval {
    /// Logo pieces (icon, letters).
    pub pieces: usize,
    /// Faces deleted, thin solids included.
    pub faces: usize,
}

/// What logo removal did to one model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogoOutcome {
    /// No logo was found; the model is used as it is.
    NotFound,
    /// The logo was removed and the model rebuilt without it.
    Removed(LogoRemoval),
    /// A logo was found but could not be cut out without breaking the model, so
    /// the original model (logo included) is used.
    Kept,
    /// The model could not be read, so it could not be checked for a logo; the
    /// original model is used.
    Unreadable,
}

impl LogoOutcome {
    /// The exported model may still carry the logo.
    pub fn logo_remains(self) -> bool {
        matches!(self, Self::Kept | Self::Unreadable)
    }
}

/// The model without the EasyEDA logo, and what was done. The new model is only
/// returned for [`LogoOutcome::Removed`]; in every other case the caller keeps
/// the original bytes.
pub fn strip_logo(data: &[u8]) -> (Option<Vec<u8>>, LogoOutcome) {
    match remove_logo(data) {
        Ok((model, removal)) => (Some(model), LogoOutcome::Removed(removal)),
        Err(outcome) => (None, outcome),
    }
}

/// The model without the EasyEDA logo; otherwise why it was left as it was.
fn remove_logo(data: &[u8]) -> Result<(Vec<u8>, LogoRemoval), LogoOutcome> {
    let text: String = data.iter().map(|&byte| byte as char).collect();
    let mut model = Model::parse(&text).ok_or(LogoOutcome::Unreadable)?;
    let geometry = Geometry::new(&model);
    let pieces = geometry.pieces();
    let mut selected = select_logo(&pieces);
    // Some models draw the icon across the edge of the face the words sit on, so
    // it is not a separate relief: take the words, and colour the icon like the
    // face under it instead of cutting it out.
    let mut icon_faces: HashSet<u64> = HashSet::new();
    if selected.is_empty() {
        selected = select_words(&pieces);
        if selected.is_empty() {
            return Err(LogoOutcome::NotFound);
        }
        icon_faces = geometry.faces_beside(&pieces, &selected);
    }
    let base_face = selected
        .iter()
        .find_map(|&i| pieces[i].loops.first().map(|&(face, _)| face));
    let mut dead: HashSet<u64> = HashSet::new();
    let mut loops: Vec<(u64, u64)> = Vec::new();
    let mut removal = LogoRemoval::default();
    for &index in &selected {
        let piece = &pieces[index];
        removal.pieces += 1;
        removal.faces += piece.faces.len();
        dead.extend(piece.faces.iter().copied());
        if let Some(solid) = piece.solid {
            dead.insert(solid);
            dead.extend(model.refs(solid));
        }
        loops.extend(piece.loops.iter().copied());
    }
    // Letters that straddle the seam of a split face (round bodies) are part of
    // its outline rather than holes in it, so they cannot be cut out: give what
    // is left inside each logo's area the colour of the face under it.
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for &index in &selected {
        match groups
            .iter_mut()
            .find(|g| coplanar(&pieces[g[0]], &pieces[index]))
        {
            Some(group) => group.push(index),
            None => groups.push(vec![index]),
        }
    }
    let mut leftovers: Vec<(HashSet<u64>, u64)> = Vec::new();
    for group in groups {
        let first = &pieces[group[0]];
        let Some(&(base, _)) = first.loops.first() else {
            continue;
        };
        let (mut lo, mut hi) = (first.lo, first.hi);
        for &i in &group {
            for a in 0..3 {
                lo[a] = lo[a].min(pieces[i].lo[a]);
                hi[a] = hi[a].max(pieces[i].hi[a]);
            }
        }
        let margin = 0.02;
        let bases: HashSet<u64> = loops.iter().map(|&(face, _)| face).collect();
        let mut leftover: HashSet<u64> = HashSet::new();
        for entity in &model.entities {
            let face = entity.id;
            if entity.kind != "ADVANCED_FACE"
                || dead.contains(&face)
                || bases.contains(&face)
                || icon_faces.contains(&face)
            {
                continue;
            }
            let one: HashSet<u64> = [face].into_iter().collect();
            let points = geometry.face_points(&one);
            let within = !points.is_empty()
                && points.iter().all(|p| {
                    (0..3).all(|a| p[a] >= lo[a] - margin && p[a] <= hi[a] + margin)
                        && match first.cylinder {
                            Some(c) => c.depth(*p) <= MAX_RELIEF_MM,
                            None => (dot(*p, first.normal) - first.offset).abs() <= MAX_RELIEF_MM,
                        }
                });
            if within {
                leftover.insert(face);
            }
        }
        leftovers.push((leftover, base));
    }
    if let Some(base) = base_face {
        let recoloured = recolour(&mut model, &icon_faces, base);
        if recoloured > 0 {
            removal.pieces += 1;
            removal.faces += recoloured;
        }
    }
    for (leftover, base) in leftovers {
        removal.faces += recolour(&mut model, &leftover, base);
    }

    for (face, bound) in loops {
        if dead.contains(&face) {
            continue;
        }
        let body = model.body(face).ok_or(LogoOutcome::Kept)?.to_string();
        let edited = remove_ref(&body, bound).ok_or(LogoOutcome::Kept)?;
        model.set_body(face, edited);
    }
    model.delete_cascading(dead);
    model.collect_garbage();
    if !model.is_consistent() {
        return Err(LogoOutcome::Kept);
    }
    let out: Vec<u8> = model.write().chars().map(|c| c as u8).collect();
    Ok((out, removal))
}

// ---------------------------------------------------------------------------
// STEP text

struct Entity {
    id: u64,
    /// Statement as read, without the closing `;`.
    raw: String,
    /// Everything after `=`.
    body: String,
    kind: String,
    refs: Vec<u64>,
    edited: bool,
}

struct Model {
    head: String,
    tail: String,
    entities: Vec<Entity>,
    index: HashMap<u64, usize>,
    roots: Vec<u64>,
    live: Vec<bool>,
}

impl Model {
    fn parse(text: &str) -> Option<Self> {
        let data_at = text.find("DATA;")? + "DATA;".len();
        let head = text[..data_at].to_string();
        let mut entities = Vec::new();
        let mut tail = None;
        let mut start = data_at;
        let mut in_string = false;
        let bytes = text.as_bytes();
        let mut i = data_at;
        while i < bytes.len() {
            match bytes[i] {
                b'\'' => in_string = !in_string,
                b';' if !in_string => {
                    let statement = text[start..i].trim();
                    if let Some(rest) = statement.strip_prefix('#') {
                        let (id, body) = rest.split_once('=')?;
                        let id: u64 = id.trim().parse().ok()?;
                        let body = body.trim().to_string();
                        entities.push(Entity {
                            id,
                            raw: statement.to_string(),
                            kind: entity_kind(&body),
                            refs: refs_of(&body),
                            body,
                            edited: false,
                        });
                    } else if statement == "ENDSEC" {
                        tail = Some(text[start..].trim_start().to_string());
                        break;
                    } else if !statement.is_empty() {
                        return None;
                    }
                    start = i + 1;
                }
                _ => {}
            }
            i += 1;
        }
        let tail = tail?;
        let index: HashMap<u64, usize> = entities
            .iter()
            .enumerate()
            .map(|(i, e)| (e.id, i))
            .collect();
        let mut used: HashSet<u64> = HashSet::new();
        for entity in &entities {
            used.extend(entity.refs.iter().copied());
        }
        let roots = entities
            .iter()
            .map(|e| e.id)
            .filter(|id| !used.contains(id))
            .collect();
        let live = vec![true; entities.len()];
        Some(Self {
            head,
            tail,
            entities,
            index,
            roots,
            live,
        })
    }

    fn get(&self, id: u64) -> Option<&Entity> {
        let &i = self.index.get(&id)?;
        self.live[i].then(|| &self.entities[i])
    }

    fn kind(&self, id: u64) -> &str {
        self.get(id).map(|e| e.kind.as_str()).unwrap_or("")
    }

    fn refs(&self, id: u64) -> Vec<u64> {
        self.get(id).map(|e| e.refs.clone()).unwrap_or_default()
    }

    fn body(&self, id: u64) -> Option<&str> {
        self.get(id).map(|e| e.body.as_str())
    }

    fn set_body(&mut self, id: u64, body: String) {
        if let Some(&i) = self.index.get(&id) {
            let entity = &mut self.entities[i];
            entity.refs = refs_of(&body);
            entity.body = body;
            entity.edited = true;
        }
    }

    /// Deletes `dead`; entities that point at a deleted one either lose it from a
    /// list of references or are deleted too.
    fn delete_cascading(&mut self, dead: HashSet<u64>) {
        let mut users: HashMap<u64, Vec<u64>> = HashMap::new();
        for entity in &self.entities {
            for &r in &entity.refs {
                users.entry(r).or_default().push(entity.id);
            }
        }
        let mut gone: HashSet<u64> = HashSet::new();
        let mut queue: Vec<u64> = dead.into_iter().collect();
        while let Some(id) = queue.pop() {
            if !gone.insert(id) {
                continue;
            }
            for user in users.get(&id).cloned().unwrap_or_default() {
                if gone.contains(&user) || !self.refs(user).contains(&id) {
                    continue;
                }
                let body = self.body(user).unwrap_or_default().to_string();
                match remove_ref(&body, id) {
                    Some(edited) => self.set_body(user, edited),
                    None => queue.push(user),
                }
            }
        }
        for id in gone {
            if let Some(&i) = self.index.get(&id) {
                self.live[i] = false;
            }
        }
    }

    /// Keeps only what the file's top-level entities still reach.
    fn collect_garbage(&mut self) {
        let mut reached = vec![false; self.entities.len()];
        let mut stack: Vec<u64> = self.roots.clone();
        while let Some(id) = stack.pop() {
            let Some(&i) = self.index.get(&id) else {
                continue;
            };
            if !self.live[i] || reached[i] {
                continue;
            }
            reached[i] = true;
            stack.extend(self.entities[i].refs.iter().copied());
        }
        self.live = reached;
    }

    /// Every closed shell still closed (each edge used by exactly two of its
    /// faces), no reference to a missing entity, at least one solid left.
    fn is_consistent(&self) -> bool {
        let mut solids = 0;
        for (i, entity) in self.entities.iter().enumerate() {
            if !self.live[i] {
                continue;
            }
            if entity.refs.iter().any(|&r| self.get(r).is_none()) {
                return false;
            }
            match entity.kind.as_str() {
                "MANIFOLD_SOLID_BREP" | "BREP_WITH_VOIDS" => solids += 1,
                "CLOSED_SHELL" => {
                    let mut uses: HashMap<u64, usize> = HashMap::new();
                    for &face in &entity.refs {
                        for edge in face_edges(self, face) {
                            *uses.entry(edge).or_default() += 1;
                        }
                    }
                    if uses.values().any(|&n| n != 2) {
                        return false;
                    }
                }
                _ => {}
            }
        }
        solids > 0
    }

    fn write(&self) -> String {
        let mut out = String::with_capacity(self.head.len() + self.entities.len() * 64);
        out.push_str(&self.head);
        out.push('\n');
        for (i, entity) in self.entities.iter().enumerate() {
            if !self.live[i] {
                continue;
            }
            if entity.edited {
                out.push_str(&format!("#{} = {}", entity.id, entity.body));
            } else {
                out.push_str(&entity.raw);
            }
            out.push_str(" ;\n");
        }
        out.push_str(&self.tail);
        out
    }
}

/// `body` with every quoted string emptied, so brackets and `#` inside names
/// do not count.
fn without_strings(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut in_string = false;
    for c in body.chars() {
        if c == '\'' {
            in_string = !in_string;
            out.push(c);
        } else if !in_string {
            out.push(c);
        }
    }
    out
}

fn entity_kind(body: &str) -> String {
    without_strings(body)
        .trim_start_matches(|c: char| c == '(' || c.is_whitespace())
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect()
}

fn refs_of(body: &str) -> Vec<u64> {
    let clean = without_strings(body);
    let mut refs = Vec::new();
    let bytes = clean.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if let Ok(id) = clean[start..end].parse() {
                refs.push(id);
            }
            i = end;
        } else {
            i += 1;
        }
    }
    refs
}

/// Drops `#target` from the plain reference list it sits in. `None` when it is
/// not in such a list, or is the list's only member.
fn remove_ref(body: &str, target: u64) -> Option<String> {
    let token = format!("#{target}");
    let bytes = body.as_bytes();
    let mut in_string = false;
    let mut at = None;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' => in_string = !in_string,
            b'#' if !in_string && body[i..].starts_with(&token) => {
                let end = i + token.len();
                if end == bytes.len() || !bytes[end].is_ascii_digit() {
                    at = Some(i);
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }
    let at = at?;
    let list_char = |c: u8| c == b'#' || c == b',' || c.is_ascii_digit() || c.is_ascii_whitespace();
    let mut open = at;
    loop {
        if open == 0 {
            return None;
        }
        open -= 1;
        match bytes[open] {
            b'(' => break,
            c if list_char(c) => {}
            _ => return None,
        }
    }
    let mut close = at;
    loop {
        close += 1;
        match bytes.get(close)? {
            b')' => break,
            &c if list_char(c) => {}
            _ => return None,
        }
    }
    let items: Vec<&str> = body[open + 1..close].split(',').map(str::trim).collect();
    if items.len() < 2 {
        return None;
    }
    let kept: Vec<&str> = items.into_iter().filter(|item| *item != token).collect();
    Some(format!(
        "{} {} {}",
        &body[..=open],
        kept.join(", "),
        &body[close..]
    ))
}

fn numbers(body: &str) -> Vec<f64> {
    let clean = without_strings(body);
    let Some(open) = clean.rfind('(') else {
        return Vec::new();
    };
    let close = clean[open..]
        .find(')')
        .map(|c| open + c)
        .unwrap_or(clean.len());
    clean[open + 1..close]
        .split(',')
        .filter_map(|v| v.trim().parse::<f64>().ok())
        .collect()
}

// ---------------------------------------------------------------------------
// Topology

/// Bounds of an `ADVANCED_FACE`: every reference but the last (the surface).
fn face_bounds(model: &Model, face: u64) -> Vec<u64> {
    let refs = model.refs(face);
    refs[..refs.len().saturating_sub(1)].to_vec()
}

fn face_surface(model: &Model, face: u64) -> Option<u64> {
    model.refs(face).last().copied()
}

fn bound_edges(model: &Model, bound: u64) -> Vec<u64> {
    let Some(&edge_loop) = model.refs(bound).first() else {
        return Vec::new();
    };
    if model.kind(edge_loop) != "EDGE_LOOP" {
        return Vec::new();
    }
    model
        .refs(edge_loop)
        .into_iter()
        .filter_map(|oriented| model.refs(oriented).last().copied())
        .collect()
}

fn face_edges(model: &Model, face: u64) -> Vec<u64> {
    face_bounds(model, face)
        .into_iter()
        .flat_map(|bound| bound_edges(model, bound))
        .collect()
}

type Point = [f64; 3];

struct Geometry<'a> {
    model: &'a Model,
    edge_faces: HashMap<u64, Vec<u64>>,
}

/// A logo candidate: one relief feature or one thin solid.
#[derive(Clone, Copy, Debug)]
struct Cylinder {
    origin: Point,
    axis: Point,
    radius: f64,
}

impl Cylinder {
    /// Distance of `p` from the axis.
    fn distance(&self, p: Point) -> f64 {
        let v = sub(p, self.origin);
        let along = dot(v, self.axis);
        let r = sub(
            v,
            [
                self.axis[0] * along,
                self.axis[1] * along,
                self.axis[2] * along,
            ],
        );
        dot(r, r).sqrt()
    }

    fn depth(&self, p: Point) -> f64 {
        (self.distance(p) - self.radius).abs()
    }

    /// Same axis line (the radius may differ).
    fn coaxial(&self, other: &Cylinder) -> bool {
        dot(self.axis, other.axis).abs() > 0.999
            && self.distance(other.origin) <= PLANE_TOLERANCE_MM
    }

    /// Outward unit direction at `p`.
    fn radial(&self, p: Point) -> Option<Point> {
        let v = sub(p, self.origin);
        let along = dot(v, self.axis);
        unit(sub(
            v,
            [
                self.axis[0] * along,
                self.axis[1] * along,
                self.axis[2] * along,
            ],
        ))
    }
}

struct Piece {
    faces: HashSet<u64>,
    /// (face, inner bound) pairs to drop when the piece goes.
    loops: Vec<(u64, u64)>,
    solid: Option<u64>,
    /// Unit normal of the base plane; for a piece on a cylinder, the radial
    /// direction at the piece.
    normal: Point,
    /// Base plane offset along `normal`; the radius for a piece on a cylinder.
    offset: f64,
    /// The cylinder the piece is cut into, when it is not on a flat face.
    cylinder: Option<Cylinder>,
    /// Height of the relief / depth of the engraving (thickness of a thin solid).
    depth: f64,
    lo: Point,
    hi: Point,
    shape: (usize, usize),
}

impl<'a> Geometry<'a> {
    fn new(model: &'a Model) -> Self {
        let mut edge_faces: HashMap<u64, Vec<u64>> = HashMap::new();
        for entity in model.entities.iter() {
            if entity.kind == "ADVANCED_FACE" {
                for edge in face_edges(model, entity.id) {
                    edge_faces.entry(edge).or_default().push(entity.id);
                }
            }
        }
        Self { model, edge_faces }
    }

    fn point(&self, id: u64) -> Option<Point> {
        match numbers(self.model.body(id)?).as_slice() {
            [x, y, z, ..] => Some([*x, *y, *z]),
            _ => None,
        }
    }

    fn face_points(&self, faces: &HashSet<u64>) -> Vec<Point> {
        let mut points = Vec::new();
        for &face in faces {
            for edge in face_edges(self.model, face) {
                for vertex in self.model.refs(edge).into_iter().take(2) {
                    if let Some(p) = self.model.refs(vertex).first().and_then(|&p| self.point(p)) {
                        points.push(p);
                    }
                }
            }
        }
        points
    }

    /// Origin and unit normal of a planar face.
    fn plane(&self, face: u64) -> Option<(Point, Point)> {
        let surface = face_surface(self.model, face)?;
        if self.model.kind(surface) != "PLANE" {
            return None;
        }
        let placement = *self.model.refs(surface).first()?;
        let refs = self.model.refs(placement);
        let origin = self.point(*refs.first()?)?;
        let normal = unit(self.point(*refs.get(1)?)?)?;
        Some((origin, normal))
    }

    /// Axis and radius of a cylindrical face.
    fn cylinder(&self, face: u64) -> Option<Cylinder> {
        let surface = face_surface(self.model, face)?;
        if self.model.kind(surface) != "CYLINDRICAL_SURFACE" {
            return None;
        }
        let radius = *numbers(self.model.body(surface)?).last()?;
        let placement = *self.model.refs(surface).first()?;
        let refs = self.model.refs(placement);
        let origin = self.point(*refs.first()?)?;
        let axis = unit(self.point(*refs.get(1)?)?)?;
        Some(Cylinder {
            origin,
            axis,
            radius,
        })
    }

    fn pieces(&self) -> Vec<Piece> {
        let model = self.model;
        let mut pieces = Vec::new();
        let mut solid_faces: HashSet<u64> = HashSet::new();

        // Thin solids.
        let solids: Vec<u64> = model
            .entities
            .iter()
            .filter(|e| e.kind == "MANIFOLD_SOLID_BREP")
            .map(|e| e.id)
            .collect();
        if solids.len() > 1 {
            for &solid in &solids {
                let Some(&shell) = model.refs(solid).first() else {
                    continue;
                };
                let faces: HashSet<u64> = model.refs(shell).into_iter().collect();
                let Some((lo, hi)) = bounds(&self.face_points(&faces)) else {
                    continue;
                };
                let extent = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
                let axis = (0..3)
                    .min_by(|&a, &b| extent[a].total_cmp(&extent[b]))
                    .unwrap_or(2);
                if extent[axis] > MAX_THIN_SOLID_MM {
                    continue;
                }
                let mut normal = [0.0; 3];
                normal[axis] = 1.0;
                let shape = self.shape(&faces, normal);
                solid_faces.extend(faces.iter().copied());
                pieces.push(Piece {
                    faces,
                    loops: Vec::new(),
                    solid: Some(solid),
                    normal,
                    offset: (lo[axis] + hi[axis]) / 2.0,
                    cylinder: None,
                    depth: extent[axis],
                    lo,
                    hi,
                    shape,
                });
            }
        }

        // Relief / engraving behind inner loops of flat faces.
        let mut features: Vec<Piece> = Vec::new();
        for entity in model.entities.iter() {
            if entity.kind != "ADVANCED_FACE" || solid_faces.contains(&entity.id) {
                continue;
            }
            let face = entity.id;
            let inner: Vec<u64> = face_bounds(model, face)
                .into_iter()
                .filter(|&b| model.kind(b) == "FACE_BOUND")
                .collect();
            if inner.is_empty() {
                continue;
            }
            let plane = self.plane(face);
            let cylinder = if plane.is_none() {
                self.cylinder(face)
            } else {
                None
            };
            if plane.is_none() && cylinder.is_none() {
                continue;
            }
            for bound in inner {
                let Some(faces) = self.flood(face, bound) else {
                    continue;
                };
                let points = self.face_points(&faces);
                let depth_of = |p: &Point| match (plane, cylinder) {
                    (Some((origin, normal)), _) => dot(sub(*p, origin), normal).abs(),
                    (None, Some(c)) => c.depth(*p),
                    _ => f64::MAX,
                };
                let depth = points.iter().map(depth_of).fold(0.0, f64::max);
                if depth > MAX_RELIEF_MM {
                    continue;
                }
                let Some((lo, hi)) = bounds(&points) else {
                    continue;
                };
                if let Some(same) = features.iter_mut().find(|p| p.faces == faces) {
                    same.loops.push((face, bound));
                    continue;
                }
                let (normal, offset) = match (plane, cylinder) {
                    (Some((origin, normal)), _) => (normal, dot(origin, normal)),
                    (None, Some(c)) => {
                        let centre = [
                            (lo[0] + hi[0]) / 2.0,
                            (lo[1] + hi[1]) / 2.0,
                            (lo[2] + hi[2]) / 2.0,
                        ];
                        let Some(radial) = c.radial(centre) else {
                            continue;
                        };
                        (radial, c.radius)
                    }
                    _ => continue,
                };
                let shape = self.shape_on(&faces, normal, cylinder);
                features.push(Piece {
                    faces,
                    loops: vec![(face, bound)],
                    solid: None,
                    normal,
                    offset,
                    cylinder,
                    depth,
                    lo,
                    hi,
                    shape,
                });
            }
        }
        // A letter's island (inside an "A" or "D") is reached again from the
        // letter's own floor; only the whole letter counts as a piece. A deeper
        // recess that merely holds shallower letters (C9002) does not swallow them.
        let holds = |g: &Piece, f: &Piece| {
            g.faces.len() > f.faces.len()
                && f.faces.is_subset(&g.faces)
                && (g.depth - f.depth).abs() <= 0.005
        };
        let outer: Vec<bool> = features
            .iter()
            .map(|f| !features.iter().any(|g| holds(g, f)))
            .collect();
        let mut nested_loops: Vec<(usize, Vec<(u64, u64)>)> = Vec::new();
        for (i, f) in features.iter().enumerate() {
            if !outer[i] {
                if let Some(j) = features.iter().position(|g| holds(g, f)) {
                    nested_loops.push((j, f.loops.clone()));
                }
            }
        }
        for (j, loops) in nested_loops {
            features[j].loops.extend(loops);
        }
        pieces.extend(
            features
                .into_iter()
                .zip(outer)
                .filter(|(_, o)| *o)
                .map(|(f, _)| f),
        );
        pieces
    }

    /// Small faces in the plane of the words `words` (within relief depth) close
    /// to them: the logo icon when it is not a separate relief.
    fn faces_beside(&self, pieces: &[Piece], words: &[usize]) -> HashSet<u64> {
        let first = &pieces[words[0]];
        let (normal, offset, cylinder) = (first.normal, first.offset, first.cylinder);
        let (mut lo, mut hi) = (first.lo, first.hi);
        let mut heights: Vec<f64> = Vec::new();
        let mut taken: HashSet<u64> = HashSet::new();
        for &i in words {
            let p = &pieces[i];
            for a in 0..3 {
                lo[a] = lo[a].min(p.lo[a]);
                hi[a] = hi[a].max(p.hi[a]);
            }
            heights.push((0..3).map(|a| p.hi[a] - p.lo[a]).fold(0.0, f64::max));
            taken.extend(p.faces.iter().copied());
        }
        heights.sort_by(f64::total_cmp);
        let reach = 6.0 * heights[heights.len() / 2];
        for a in 0..3 {
            lo[a] -= reach;
            hi[a] += reach;
        }
        let mut found = HashSet::new();
        for entity in &self.model.entities {
            if entity.kind != "ADVANCED_FACE" || taken.contains(&entity.id) {
                continue;
            }
            let one: HashSet<u64> = [entity.id].into_iter().collect();
            let points = self.face_points(&one);
            if !points.is_empty()
                && points.iter().all(|p| {
                    (0..3).all(|a| p[a] >= lo[a] && p[a] <= hi[a])
                        && match cylinder {
                            Some(c) => c.depth(*p) <= MAX_RELIEF_MM,
                            None => (dot(*p, normal) - offset).abs() <= MAX_RELIEF_MM,
                        }
                })
            {
                found.insert(entity.id);
            }
        }
        found
    }

    /// Faces reached from the inner loop `bound` of `face` without crossing it.
    fn flood(&self, face: u64, bound: u64) -> Option<HashSet<u64>> {
        let mut found: HashSet<u64> = HashSet::new();
        let mut stack: Vec<u64> = Vec::new();
        for edge in bound_edges(self.model, bound) {
            stack.extend(self.edge_faces.get(&edge).into_iter().flatten().copied());
        }
        while let Some(next) = stack.pop() {
            if next == face || !found.insert(next) {
                continue;
            }
            if found.len() > MAX_FEATURE_FACES {
                return None;
            }
            for edge in face_edges(self.model, next) {
                stack.extend(self.edge_faces.get(&edge).into_iter().flatten().copied());
            }
        }
        (!found.is_empty()).then_some(found)
    }

    /// (side faces, curved side faces): faces not parallel to the base plane.
    fn shape(&self, faces: &HashSet<u64>, normal: Point) -> (usize, usize) {
        self.shape_on(faces, normal, None)
    }

    /// [`Self::shape`] for a piece on a flat face or, with `cylinder`, cut into a
    /// round one: there the floor is coaxial with it rather than parallel.
    fn shape_on(
        &self,
        faces: &HashSet<u64>,
        normal: Point,
        cylinder: Option<Cylinder>,
    ) -> (usize, usize) {
        let mut sides = 0;
        let mut curved = 0;
        for &face in faces {
            if let Some(base) = cylinder {
                if self.cylinder(face).is_some_and(|c| c.coaxial(&base)) {
                    continue;
                }
            }
            if let Some((_, n)) = self.plane(face) {
                if (dot(n, normal).abs() - 1.0).abs() < 1e-3 {
                    continue;
                }
            }
            sides += 1;
            let kind = face_surface(self.model, face)
                .map(|s| self.model.kind(s))
                .unwrap_or("");
            if kind.starts_with("B_SPLINE") {
                curved += 1;
            }
        }
        (sides, curved)
    }
}

/// Gives `faces` the colour of `like`: their STYLED_ITEMs take its styles.
/// Returns how many faces changed.
fn recolour(model: &mut Model, faces: &HashSet<u64>, like: u64) -> usize {
    let mut styled: HashMap<u64, Vec<u64>> = HashMap::new();
    for entity in &model.entities {
        if entity.kind == "STYLED_ITEM" {
            if let Some(&item) = entity.refs.last() {
                styled.entry(item).or_default().push(entity.id);
            }
        }
    }
    let Some(template) = styled
        .get(&like)
        .and_then(|items| items.first())
        .and_then(|&id| model.body(id))
        .map(str::to_string)
    else {
        return 0;
    };
    let tag = format!("#{like}");
    let Some(at) = template.rfind(&tag) else {
        return 0;
    };
    let mut changed = 0;
    for &face in faces {
        let Some(items) = styled.get(&face) else {
            continue;
        };
        for &item in items {
            let body = format!("{}#{face}{}", &template[..at], &template[at + tag.len()..]);
            model.set_body(item, body);
        }
        changed += 1;
    }
    changed
}

/// The "LCEDA" and "EasyEDA" words without their icon: twelve coplanar letters
/// next to each other that include every letter shape of the logo.
fn select_words(pieces: &[Piece]) -> Vec<usize> {
    let letters: Vec<usize> = (0..pieces.len())
        .filter(|&i| pieces[i].solid.is_none() && LOGO_LETTERS.contains(&pieces[i].shape))
        .collect();
    let mut seen: HashSet<usize> = HashSet::new();
    for &start in &letters {
        if seen.contains(&start) {
            continue;
        }
        let mut group = vec![start];
        let mut k = 0;
        while k < group.len() {
            let current = &pieces[group[k]];
            let height = (0..3)
                .map(|a| current.hi[a] - current.lo[a])
                .fold(0.0, f64::max);
            for &other in &letters {
                if !group.contains(&other)
                    && coplanar(&pieces[other], current)
                    && gap(&pieces[other], current) <= 0.8 * height
                {
                    group.push(other);
                }
            }
            k += 1;
        }
        seen.extend(group.iter().copied());
        let shapes: Vec<(usize, usize)> = group.iter().map(|&i| pieces[i].shape).collect();
        if group.len() == 12 && LOGO_LETTERS.iter().all(|s| shapes.contains(s)) {
            group.sort_unstable();
            return group;
        }
    }
    Vec::new()
}

/// Indices of the pieces that make up the logo.
fn select_logo(pieces: &[Piece]) -> Vec<usize> {
    let mut chosen: HashSet<usize> = HashSet::new();
    // Icon and the words next to it.
    for (icon, piece) in pieces.iter().enumerate() {
        if !shape_like(piece, ICON) || chosen.contains(&icon) {
            continue;
        }
        let size = (0..3)
            .map(|a| piece.hi[a] - piece.lo[a])
            .fold(0.0, f64::max);
        let reach = if piece.cylinder.is_some() { 0.5 } else { 0.35 } * size;
        let mut group = vec![icon];
        let (mut lo, mut hi) = (piece.lo, piece.hi);
        loop {
            let next = pieces.iter().enumerate().position(|(i, p)| {
                !group.contains(&i)
                    && coplanar(p, piece)
                    && (inside(p, lo, hi)
                        || ((LOGO_LETTERS.iter().any(|&s| shape_like(p, s))
                            || shape_like(p, ICON))
                            && group.iter().any(|&g| gap(p, &pieces[g]) <= reach)))
            });
            let Some(next) = next else { break };
            group.push(next);
            for a in 0..3 {
                lo[a] = lo[a].min(pieces[next].lo[a]);
                hi[a] = hi[a].max(pieces[next].hi[a]);
            }
        }
        // Icon plus at least the five letters of one word.
        if group.len() >= 6 {
            chosen.extend(group);
        }
    }
    // Separate "LCEDA" rows.
    let rest: Vec<usize> = (0..pieces.len())
        .filter(|i| !chosen.contains(i) && ROW_LETTERS.contains(&pieces[*i].shape))
        .collect();
    let mut seen: HashSet<usize> = HashSet::new();
    for &start in &rest {
        if seen.contains(&start) {
            continue;
        }
        let mut group = vec![start];
        let mut k = 0;
        while k < group.len() {
            let current = &pieces[group[k]];
            let height = (0..3)
                .map(|a| current.hi[a] - current.lo[a])
                .fold(0.0, f64::max);
            for &other in &rest {
                if !group.contains(&other)
                    && coplanar(&pieces[other], current)
                    && gap(&pieces[other], current) <= 0.6 * height
                {
                    group.push(other);
                }
            }
            k += 1;
        }
        seen.extend(group.iter().copied());
        let shapes: Vec<(usize, usize)> = group.iter().map(|&i| pieces[i].shape).collect();
        if group.len() == 5 && ROW_REQUIRED.iter().all(|s| shapes.contains(s)) {
            chosen.extend(group);
        }
    }
    let mut chosen: Vec<usize> = chosen.into_iter().collect();
    chosen.sort_unstable();
    chosen
}

/// Whether `piece` has the logo shape `shape` (side faces, curved side faces).
/// Cut into a cylinder, outlines split into a few more faces than on a flat face.
fn shape_like(piece: &Piece, shape: (usize, usize)) -> bool {
    if piece.cylinder.is_none() {
        return piece.shape == shape;
    }
    piece.shape.0.abs_diff(shape.0) <= 2 && piece.shape.1.abs_diff(shape.1) <= 3
}

fn coplanar(a: &Piece, b: &Piece) -> bool {
    match (a.cylinder, b.cylinder) {
        (Some(ca), Some(cb)) => {
            return ca.coaxial(&cb) && (ca.radius - cb.radius).abs() <= PLANE_TOLERANCE_MM;
        }
        (None, None) => {}
        _ => return false,
    }
    let d = dot(a.normal, b.normal);
    d.abs() > 0.999 && (a.offset - d.signum() * b.offset).abs() <= PLANE_TOLERANCE_MM
}

/// Distance between two bounding boxes (0 when they touch or overlap).
fn gap(a: &Piece, b: &Piece) -> f64 {
    (0..3)
        .map(|i| (a.lo[i] - b.hi[i]).max(b.lo[i] - a.hi[i]).max(0.0))
        .fold(0.0, f64::max)
}

fn inside(p: &Piece, lo: Point, hi: Point) -> bool {
    (0..3).all(|a| p.lo[a] >= lo[a] - 1e-3 && p.hi[a] <= hi[a] + 1e-3)
}

fn bounds(points: &[Point]) -> Option<(Point, Point)> {
    let first = *points.first()?;
    let (mut lo, mut hi) = (first, first);
    for p in points {
        for a in 0..3 {
            lo[a] = lo[a].min(p[a]);
            hi[a] = hi[a].max(p[a]);
        }
    }
    Some((lo, hi))
}

fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn unit(v: Point) -> Option<Point> {
    let length = dot(v, v).sqrt();
    (length > 1e-12).then(|| [v[0] / length, v[1] / length, v[2] / length])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_reference_from_list_only() {
        assert_eq!(
            remove_ref("CLOSED_SHELL ( 'NONE', ( #1, #22, #3 ) )", 22).as_deref(),
            Some("CLOSED_SHELL ( 'NONE', ( #1, #3 ) )")
        );
        assert_eq!(remove_ref("STYLED_ITEM ( 'color', ( #5 ), #22 )", 22), None);
        assert_eq!(remove_ref("X ( ( #22 ) )", 22), None);
        assert_eq!(
            remove_ref("X ( ( #220, #22 ) )", 22).as_deref(),
            Some("X ( ( #220 ) )")
        );
        assert_eq!(
            remove_ref("X ( '#22', ( #1, #22 ) )", 22).as_deref(),
            Some("X ( '#22', ( #1 ) )")
        );
    }

    #[test]
    fn parses_kind_refs_and_numbers() {
        let body = "CARTESIAN_POINT ( 'p(1)', ( 0.5, -1.E-05, 2. ) )";
        assert_eq!(entity_kind(body), "CARTESIAN_POINT");
        assert_eq!(numbers(body), vec![0.5, -1e-5, 2.0]);
        assert_eq!(
            refs_of("EDGE_CURVE ( '#9', #1, #23, #4, .T. )"),
            vec![1, 23, 4]
        );
        assert_eq!(
            entity_kind("( NAMED_UNIT ( * ) SI_UNIT ( $, .METRE. ) )"),
            "NAMED_UNIT"
        );
    }

    /// A unit cube with a 3-letter "logo" would be too contrived to build by
    /// hand; models without a logo must come back untouched.
    #[test]
    fn model_without_logo_is_left_alone() {
        let step = "ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\n#1 = CARTESIAN_POINT ( 'NONE', ( 0., 0., 0. ) ) ;\nENDSEC;\nEND-ISO-10303-21;\n";
        assert_eq!(strip_logo(step.as_bytes()), (None, LogoOutcome::NotFound));
    }

    #[test]
    fn recolour_copies_the_base_face_style() {
        let step = "ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\n\
            #1 = ADVANCED_FACE ( 'NONE', ( ), #9, .T. ) ;\n\
            #2 = ADVANCED_FACE ( 'NONE', ( ), #9, .T. ) ;\n\
            #3 = STYLED_ITEM ( 'NONE', ( #7 ), #1 ) ;\n\
            #4 = STYLED_ITEM ( 'NONE', ( #8 ), #2 ) ;\n\
            #7 = PRESENTATION_STYLE_ASSIGNMENT ( ( ) ) ;\n\
            #8 = PRESENTATION_STYLE_ASSIGNMENT ( ( ) ) ;\n\
            #9 = PLANE ( 'NONE', $ ) ;\n\
            ENDSEC;\nEND-ISO-10303-21;\n";
        let mut model = Model::parse(step).unwrap();
        let faces: HashSet<u64> = [2].into_iter().collect();
        assert_eq!(recolour(&mut model, &faces, 1), 1);
        assert_eq!(model.body(4), Some("STYLED_ITEM ( 'NONE', ( #7 ), #2 )"));
        assert_eq!(model.refs(4), vec![7, 2]);
    }
}
