//! STEP (AP203/AP214) boundary representation -> triangle mesh, for the real
//! extent of a model: round bodies are bounded by surfaces whose extremes are no
//! vertices, so placing a model on its footprint needs its surfaces.
//!
//! Faces are triangulated from their edge loops.
//! Planar faces are triangulated in their plane; cylinders, cones, spheres and
//! tori in their (angle, height) parameters and evaluated on the real surface, so
//! round bodies stay round; other surfaces (B-splines) use the best-fit plane of
//! their boundary, which is close enough for small fillets. Assemblies are
//! resolved through their placement transforms, and face / body colours are kept.

use std::collections::HashMap;

/// Triangle mesh in millimetres with one colour per triangle.
#[derive(Debug, Default, Clone)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    pub colors: Vec<[u8; 3]>,
    /// Surface normal at each position, for smooth shading. May be shorter than
    /// `positions`; a missing or zero normal is worked out from the triangles.
    pub normals: Vec<[f32; 3]>,
}

/// Points, triangles and (when known) the surface normal at each point.
type FaceMesh = (Vec<V3>, Vec<[usize; 3]>, Vec<V3>);

impl Mesh {
    pub fn is_empty(&self) -> bool {
        self.triangles.is_empty()
    }

    /// (min, max) corners.
    #[cfg(test)]
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        let first = *self.positions.first()?;
        let (mut lo, mut hi) = (first, first);
        for p in &self.positions {
            for a in 0..3 {
                lo[a] = lo[a].min(p[a]);
                hi[a] = hi[a].max(p[a]);
            }
        }
        Some((lo, hi))
    }
}

const DEFAULT_COLOR: [u8; 3] = [178, 178, 184];
/// Angular step on curved edges and surfaces (radians).
const ANGLE_STEP: f64 = std::f64::consts::PI / 12.0;

/// Mesh of every solid / shell in `data`; `None` if nothing could be read.
pub fn tessellate(data: &[u8]) -> Option<Mesh> {
    let text: String = data.iter().map(|&b| b as char).collect();
    let step = Step::parse(&text)?;
    let mesh = Mesher::new(&step).run();
    (!mesh.is_empty()).then_some(mesh)
}

// ---------------------------------------------------------------------------
// STEP parameters

#[derive(Debug, Clone)]
enum P {
    Ref(u64),
    Num(f64),
    Str,
    Enum(String),
    List(Vec<P>),
    Typed(String, Vec<P>),
    Null,
}

impl P {
    fn reference(&self) -> Option<u64> {
        match self {
            P::Ref(r) => Some(*r),
            _ => None,
        }
    }
    fn num(&self) -> Option<f64> {
        match self {
            P::Num(n) => Some(*n),
            P::Typed(_, args) => args.first()?.num(),
            _ => None,
        }
    }
    fn list(&self) -> &[P] {
        match self {
            P::List(items) => items,
            _ => &[],
        }
    }
    fn is_true(&self) -> bool {
        matches!(self, P::Enum(e) if e == "T")
    }
}

/// One entity: `KIND(args)`, or a complex entity as several (kind, args) parts.
struct Entity {
    parts: Vec<(String, Vec<P>)>,
}

impl Entity {
    fn kind(&self) -> &str {
        self.parts.first().map(|(k, _)| k.as_str()).unwrap_or("")
    }
    fn args(&self) -> &[P] {
        self.parts.first().map(|(_, a)| a.as_slice()).unwrap_or(&[])
    }
    fn part(&self, kind: &str) -> Option<&[P]> {
        self.parts
            .iter()
            .find(|(k, _)| k == kind)
            .map(|(_, a)| a.as_slice())
    }
}

struct Step {
    entities: HashMap<u64, Entity>,
}

impl Step {
    fn parse(text: &str) -> Option<Self> {
        let start = text.find("DATA;")? + 5;
        let bytes = text.as_bytes();
        let mut entities = HashMap::new();
        let mut i = start;
        let mut stmt_start = start;
        let mut in_string = false;
        while i < bytes.len() {
            let c = bytes[i];
            if c == b'\'' {
                in_string = !in_string;
            } else if c == b';' && !in_string {
                let stmt = text[stmt_start..i].trim();
                if stmt == "ENDSEC" {
                    break;
                }
                if let Some(rest) = stmt.strip_prefix('#') {
                    if let Some((id, body)) = rest.split_once('=') {
                        if let (Ok(id), Some(entity)) =
                            (id.trim().parse(), parse_entity(body.trim()))
                        {
                            entities.insert(id, entity);
                        }
                    }
                }
                stmt_start = i + 1;
            }
            i += 1;
        }
        (!entities.is_empty()).then_some(Self { entities })
    }

    fn get(&self, id: u64) -> Option<&Entity> {
        self.entities.get(&id)
    }
    fn kind(&self, id: u64) -> &str {
        self.get(id).map(Entity::kind).unwrap_or("")
    }
    fn args(&self, id: u64) -> &[P] {
        self.get(id).map(Entity::args).unwrap_or(&[])
    }
    fn point(&self, id: u64) -> Option<V3> {
        let coords = self.args(id).get(1)?.list();
        Some(V3(
            coords.first()?.num()?,
            coords.get(1).and_then(P::num).unwrap_or(0.0),
            coords.get(2).and_then(P::num).unwrap_or(0.0),
        ))
    }
    /// Position, axis and reference direction of an AXIS2_PLACEMENT_3D.
    fn frame(&self, id: u64) -> Option<Frame> {
        let args = self.args(id);
        let origin = self.point(args.get(1)?.reference()?)?;
        let z = args
            .get(2)
            .and_then(P::reference)
            .and_then(|d| self.point(d))
            .unwrap_or(V3(0.0, 0.0, 1.0))
            .unit()?;
        let x_hint = args
            .get(3)
            .and_then(P::reference)
            .and_then(|d| self.point(d))
            .unwrap_or(if z.0.abs() < 0.9 {
                V3(1.0, 0.0, 0.0)
            } else {
                V3(0.0, 1.0, 0.0)
            });
        let x = (x_hint - z * x_hint.dot(z)).unit()?;
        let y = z.cross(x);
        Some(Frame { origin, x, y, z })
    }
}

fn parse_entity(body: &str) -> Option<Entity> {
    let mut parser = Parser {
        s: body.as_bytes(),
        i: 0,
    };
    parser.ws();
    if parser.peek() == Some(b'(') {
        // Complex entity: ( A(..) B(..) ... )
        parser.i += 1;
        let mut parts = Vec::new();
        loop {
            parser.ws();
            match parser.peek() {
                Some(b')') | None => break,
                _ => {
                    let name = parser.ident();
                    if name.is_empty() {
                        return None;
                    }
                    parser.ws();
                    let args = if parser.peek() == Some(b'(') {
                        parser.list()?
                    } else {
                        Vec::new()
                    };
                    parts.push((name, args));
                }
            }
        }
        Some(Entity { parts })
    } else {
        let name = parser.ident();
        parser.ws();
        let args = parser.list()?;
        Some(Entity {
            parts: vec![(name, args)],
        })
    }
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }
    fn ws(&mut self) {
        while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            self.i += 1;
        }
    }
    fn ident(&mut self) -> String {
        let start = self.i;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            self.i += 1;
        }
        String::from_utf8_lossy(&self.s[start..self.i]).to_string()
    }
    /// `( item, item, ... )`
    fn list(&mut self) -> Option<Vec<P>> {
        self.ws();
        if self.peek() != Some(b'(') {
            return None;
        }
        self.i += 1;
        let mut items = Vec::new();
        loop {
            self.ws();
            match self.peek()? {
                b')' => {
                    self.i += 1;
                    return Some(items);
                }
                b',' => self.i += 1,
                _ => items.push(self.value()?),
            }
        }
    }
    fn value(&mut self) -> Option<P> {
        self.ws();
        let c = self.peek()?;
        match c {
            b'#' => {
                self.i += 1;
                let start = self.i;
                while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    self.i += 1;
                }
                std::str::from_utf8(&self.s[start..self.i])
                    .ok()?
                    .parse()
                    .ok()
                    .map(P::Ref)
            }
            b'\'' => {
                self.i += 1;
                loop {
                    match self.peek()? {
                        b'\'' if self.s.get(self.i + 1) == Some(&b'\'') => self.i += 2,
                        b'\'' => {
                            self.i += 1;
                            break;
                        }
                        _ => self.i += 1,
                    }
                }
                Some(P::Str)
            }
            b'.' => {
                self.i += 1;
                let name = self.ident();
                if self.peek() == Some(b'.') {
                    self.i += 1;
                }
                Some(P::Enum(name))
            }
            b'(' => self.list().map(P::List),
            b'$' | b'*' => {
                self.i += 1;
                Some(P::Null)
            }
            c if c.is_ascii_digit() || c == b'-' || c == b'+' => {
                let start = self.i;
                self.i += 1;
                while self.peek().is_some_and(|c| {
                    c.is_ascii_digit() || matches!(c, b'.' | b'E' | b'e' | b'-' | b'+')
                }) {
                    self.i += 1;
                }
                let text = std::str::from_utf8(&self.s[start..self.i]).ok()?;
                Some(P::Num(text.parse().unwrap_or(0.0)))
            }
            c if c.is_ascii_alphabetic() => {
                let name = self.ident();
                self.ws();
                if self.peek() == Some(b'(') {
                    let args = self.list()?;
                    Some(P::Typed(name, args))
                } else {
                    Some(P::Enum(name))
                }
            }
            _ => {
                self.i += 1;
                Some(P::Null)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Vector maths

#[derive(Debug, Clone, Copy, PartialEq)]
struct V3(f64, f64, f64);

impl std::ops::Add for V3 {
    type Output = V3;
    fn add(self, o: V3) -> V3 {
        V3(self.0 + o.0, self.1 + o.1, self.2 + o.2)
    }
}
impl std::ops::Sub for V3 {
    type Output = V3;
    fn sub(self, o: V3) -> V3 {
        V3(self.0 - o.0, self.1 - o.1, self.2 - o.2)
    }
}
impl std::ops::Mul<f64> for V3 {
    type Output = V3;
    fn mul(self, k: f64) -> V3 {
        V3(self.0 * k, self.1 * k, self.2 * k)
    }
}
impl V3 {
    fn dot(self, o: V3) -> f64 {
        self.0 * o.0 + self.1 * o.1 + self.2 * o.2
    }
    fn cross(self, o: V3) -> V3 {
        V3(
            self.1 * o.2 - self.2 * o.1,
            self.2 * o.0 - self.0 * o.2,
            self.0 * o.1 - self.1 * o.0,
        )
    }
    fn len(self) -> f64 {
        self.dot(self).sqrt()
    }
    fn unit(self) -> Option<V3> {
        let l = self.len();
        (l > 1e-12).then(|| self * (1.0 / l))
    }
}

#[derive(Debug, Clone, Copy)]
struct Frame {
    origin: V3,
    x: V3,
    y: V3,
    z: V3,
}

impl Frame {
    fn local(&self, p: V3) -> V3 {
        let d = p - self.origin;
        V3(d.dot(self.x), d.dot(self.y), d.dot(self.z))
    }
    fn world(&self, l: V3) -> V3 {
        self.origin + self.x * l.0 + self.y * l.1 + self.z * l.2
    }
}

/// Rigid transform: rotation rows + translation.
#[derive(Debug, Clone, Copy)]
struct Xf {
    r: [[f64; 3]; 3],
    t: V3,
}

impl Xf {
    const ID: Xf = Xf {
        r: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        t: V3(0.0, 0.0, 0.0),
    };
    fn from_frame(f: &Frame) -> Xf {
        // Columns are the frame axes.
        Xf {
            r: [
                [f.x.0, f.y.0, f.z.0],
                [f.x.1, f.y.1, f.z.1],
                [f.x.2, f.y.2, f.z.2],
            ],
            t: f.origin,
        }
    }
    fn apply(&self, p: V3) -> V3 {
        V3(
            self.r[0][0] * p.0 + self.r[0][1] * p.1 + self.r[0][2] * p.2 + self.t.0,
            self.r[1][0] * p.0 + self.r[1][1] * p.1 + self.r[1][2] * p.2 + self.t.1,
            self.r[2][0] * p.0 + self.r[2][1] * p.1 + self.r[2][2] * p.2 + self.t.2,
        )
    }
    fn then(&self, outer: &Xf) -> Xf {
        // outer * self
        let mut r = [[0.0; 3]; 3];
        for (i, row) in r.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = (0..3).map(|k| outer.r[i][k] * self.r[k][j]).sum();
            }
        }
        Xf {
            r,
            t: outer.apply(self.t),
        }
    }
    fn inverse(&self) -> Xf {
        let mut r = [[0.0; 3]; 3];
        for (i, row) in r.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = self.r[j][i];
            }
        }
        let inv = Xf {
            r,
            t: V3(0.0, 0.0, 0.0),
        };
        let t = inv.apply(self.t) * -1.0;
        Xf { r, t }
    }
}

// ---------------------------------------------------------------------------
// Meshing

struct Mesher<'a> {
    step: &'a Step,
    /// Diagonal of the model's points: curved faces are refined relative to it.
    scale: f64,
    colors: HashMap<u64, [u8; 3]>,
    mesh: Mesh,
}

impl<'a> Mesher<'a> {
    fn new(step: &'a Step) -> Self {
        let mut colors = HashMap::new();
        for (&id, entity) in &step.entities {
            if matches!(entity.kind(), "STYLED_ITEM" | "OVER_RIDING_STYLED_ITEM") {
                let args = entity.args();
                let target = args.get(2).and_then(P::reference);
                if let (Some(target), Some(color)) = (target, find_colour(step, id)) {
                    colors.entry(target).or_insert(color);
                }
            }
        }
        let (mut lo, mut hi) = (
            V3(f64::MAX, f64::MAX, f64::MAX),
            V3(f64::MIN, f64::MIN, f64::MIN),
        );
        for &id in step.entities.keys() {
            if step.kind(id) == "CARTESIAN_POINT" {
                if let Some(p) = step.point(id) {
                    lo = V3(lo.0.min(p.0), lo.1.min(p.1), lo.2.min(p.2));
                    hi = V3(hi.0.max(p.0), hi.1.max(p.1), hi.2.max(p.2));
                }
            }
        }
        let scale = if lo.0 <= hi.0 {
            (hi - lo).len().clamp(0.1, 1000.0)
        } else {
            10.0
        };
        Self {
            step,
            scale,
            colors,
            mesh: Mesh::default(),
        }
    }

    fn run(mut self) -> Mesh {
        let step = self.step;
        let placements = representation_placements(step);
        // Representation holding each solid / shell model.
        let mut owner: HashMap<u64, Vec<u64>> = HashMap::new();
        for (&id, entity) in &step.entities {
            if entity.kind().ends_with("SHAPE_REPRESENTATION")
                || entity.kind() == "SHAPE_REPRESENTATION"
            {
                for item in entity.args().get(1).map(P::list).unwrap_or(&[]) {
                    if let Some(r) = item.reference() {
                        owner.entry(r).or_default().push(id);
                    }
                }
            }
        }
        let mut bodies: Vec<u64> = step
            .entities
            .iter()
            .filter(|(_, e)| {
                matches!(
                    e.kind(),
                    "MANIFOLD_SOLID_BREP" | "BREP_WITH_VOIDS" | "SHELL_BASED_SURFACE_MODEL"
                )
            })
            .map(|(&id, _)| id)
            .collect();
        bodies.sort_unstable();
        for body in bodies {
            let body_color = self.colors.get(&body).copied();
            let shells: Vec<u64> = match step.kind(body) {
                "SHELL_BASED_SURFACE_MODEL" => step
                    .args(body)
                    .get(1)
                    .map(P::list)
                    .unwrap_or(&[])
                    .iter()
                    .filter_map(P::reference)
                    .collect(),
                _ => step
                    .args(body)
                    .get(1)
                    .and_then(P::reference)
                    .into_iter()
                    .collect(),
            };
            let mut transforms: Vec<Xf> = owner
                .get(&body)
                .into_iter()
                .flatten()
                .flat_map(|rep| placements.get(rep).cloned().unwrap_or_else(|| vec![Xf::ID]))
                .collect();
            if transforms.is_empty() {
                transforms.push(Xf::ID);
            }
            for shell in shells {
                let shell_color = self.colors.get(&shell).copied().or(body_color);
                let faces: Vec<u64> = step
                    .args(shell)
                    .get(1)
                    .map(P::list)
                    .unwrap_or(&[])
                    .iter()
                    .filter_map(P::reference)
                    .collect();
                for face in faces {
                    let color = self
                        .colors
                        .get(&face)
                        .copied()
                        .or(shell_color)
                        .unwrap_or(DEFAULT_COLOR);
                    if let Some((points, tris, normals)) = self.face(face) {
                        for xf in &transforms {
                            self.push(&points, &tris, &normals, color, xf);
                        }
                    }
                }
            }
        }
        self.mesh
    }

    fn push(
        &mut self,
        points: &[V3],
        tris: &[[usize; 3]],
        normals: &[V3],
        color: [u8; 3],
        xf: &Xf,
    ) {
        let base = self.mesh.positions.len() as u32;
        for (i, p) in points.iter().enumerate() {
            let w = xf.apply(*p);
            self.mesh
                .positions
                .push([w.0 as f32, w.1 as f32, w.2 as f32]);
            let n = normals.get(i).copied().unwrap_or(V3(0.0, 0.0, 0.0));
            let r = &xf.r;
            let turned = [0, 1, 2].map(|k| (r[k][0] * n.0 + r[k][1] * n.1 + r[k][2] * n.2) as f32);
            self.mesh.normals.push(turned);
        }
        for t in tris {
            self.mesh
                .triangles
                .push([base + t[0] as u32, base + t[1] as u32, base + t[2] as u32]);
            self.mesh.colors.push(color);
        }
    }

    /// Points and triangles of one ADVANCED_FACE (or FACE_SURFACE).
    fn face(&self, face: u64) -> Option<FaceMesh> {
        let step = self.step;
        let args = step.args(face);
        let bounds: Vec<u64> = args
            .get(1)?
            .list()
            .iter()
            .filter_map(P::reference)
            .collect();
        let surface = args.get(2)?.reference()?;
        let mut loops: Vec<Vec<V3>> = Vec::new();
        for bound in bounds {
            let loop_id = step.args(bound).get(1)?.reference()?;
            let ring = self.loop_points(loop_id);
            if ring.len() >= 2 {
                loops.push(ring);
            }
        }
        if loops.is_empty() {
            return None;
        }
        let surface_kind = step.kind(surface);
        match surface_kind {
            "PLANE" => {
                let frame = step.frame(step.args(surface).get(1)?.reference()?)?;
                let rings: Vec<Vec<(f64, f64)>> = loops
                    .iter()
                    .map(|l| {
                        l.iter()
                            .map(|p| {
                                let q = frame.local(*p);
                                (q.0, q.1)
                            })
                            .collect()
                    })
                    .collect();
                let (flat, tris) = triangulate(&rings)?;
                let points: Vec<V3> = flat
                    .iter()
                    .map(|&(u, v)| frame.world(V3(u, v, 0.0)))
                    .collect();
                let normals = vec![frame.z; points.len()];
                Some((points, tris, normals))
            }
            "CYLINDRICAL_SURFACE"
            | "CONICAL_SURFACE"
            | "SPHERICAL_SURFACE"
            | "TOROIDAL_SURFACE" => self
                .revolved_face(surface, surface_kind, &loops)
                .or_else(|| projected_face(&loops)),
            _ if step.get(surface).is_some_and(|e| {
                e.kind() == "B_SPLINE_SURFACE_WITH_KNOTS" || e.part("B_SPLINE_SURFACE").is_some()
            }) =>
            {
                spline_surface(step, step.get(surface)?)
                    .and_then(|surface| param_face(&surface.closed(), &loops, self.scale))
                    .or_else(|| projected_face(&loops))
            }
            _ => projected_face(&loops),
        }
    }

    /// Faces on surfaces of revolution, meshed in their (angle, height) parameters.
    fn revolved_face(&self, surface: u64, kind: &str, loops: &[Vec<V3>]) -> Option<FaceMesh> {
        let step = self.step;
        let args = step.args(surface);
        let frame = step.frame(args.get(1)?.reference()?)?;
        let r1 = args.get(2).and_then(P::num).unwrap_or(1.0);
        let r2 = args.get(3).and_then(P::num).unwrap_or(0.0);
        let surface = Revolved {
            frame,
            kind: kind.to_string(),
            r1,
            r2,
        };
        param_face(&surface, loops, self.scale)
    }

    /// Points along an EDGE_LOOP, each edge sampled along its curve.
    fn loop_points(&self, loop_id: u64) -> Vec<V3> {
        let step = self.step;
        let mut out: Vec<V3> = Vec::new();
        if step.kind(loop_id) == "VERTEX_LOOP" {
            return Vec::new();
        }
        let edges: Vec<u64> = step
            .args(loop_id)
            .get(1)
            .map(P::list)
            .unwrap_or(&[])
            .iter()
            .filter_map(P::reference)
            .collect();
        for oriented in edges {
            let oargs = step.args(oriented);
            let Some(edge) = oargs.get(3).and_then(P::reference) else {
                continue;
            };
            let forward = oargs.get(4).map(P::is_true).unwrap_or(true);
            let mut pts = self.edge_points(edge);
            if !forward {
                pts.reverse();
            }
            for p in pts {
                if out.last().is_none_or(|q| (*q - p).len() > 1e-9) {
                    out.push(p);
                }
            }
        }
        if out.len() > 1 && (out[0] - *out.last().unwrap()).len() < 1e-9 {
            out.pop();
        }
        out
    }

    fn edge_points(&self, edge: u64) -> Vec<V3> {
        let step = self.step;
        let args = step.args(edge);
        let vertex = |i: usize| -> Option<V3> {
            let v = args.get(i)?.reference()?;
            step.point(step.args(v).get(1)?.reference()?)
        };
        let (Some(a), Some(b)) = (vertex(1), vertex(2)) else {
            return Vec::new();
        };
        let same = args.get(4).map(P::is_true).unwrap_or(true);
        let Some(curve) = args.get(3).and_then(P::reference) else {
            return vec![a, b];
        };
        let pts = self.curve_points(curve, if same { a } else { b }, if same { b } else { a });
        if same {
            pts
        } else {
            pts.into_iter().rev().collect()
        }
    }

    /// Samples of `curve` from `a` to `b` in the curve's own direction.
    fn curve_points(&self, curve: u64, a: V3, b: V3) -> Vec<V3> {
        let step = self.step;
        let Some(entity) = step.get(curve) else {
            return vec![a, b];
        };
        match entity.kind() {
            "CIRCLE" | "ELLIPSE" => {
                let args = entity.args();
                let Some(frame) = args
                    .get(1)
                    .and_then(P::reference)
                    .and_then(|f| step.frame(f))
                else {
                    return vec![a, b];
                };
                let r1 = args.get(2).and_then(P::num).unwrap_or(1.0);
                let r2 = if entity.kind() == "ELLIPSE" {
                    args.get(3).and_then(P::num).unwrap_or(r1)
                } else {
                    r1
                };
                let angle = |p: V3| {
                    let q = frame.local(p);
                    (q.1 / r2).atan2(q.0 / r1)
                };
                let a0 = angle(a);
                let mut a1 = angle(b);
                while a1 <= a0 + 1e-9 {
                    a1 += std::f64::consts::TAU;
                }
                let sweep = if (a - b).len() < 1e-9 {
                    std::f64::consts::TAU
                } else {
                    a1 - a0
                };
                let n = ((sweep / ANGLE_STEP).ceil() as usize).max(2);
                let mut pts: Vec<V3> = (0..=n)
                    .map(|i| {
                        let t = a0 + sweep * i as f64 / n as f64;
                        frame.world(V3(r1 * t.cos(), r2 * t.sin(), 0.0))
                    })
                    .collect();
                pts[0] = a;
                *pts.last_mut().unwrap() = b;
                pts
            }
            "SURFACE_CURVE" | "SEAM_CURVE" => match entity.args().get(1).and_then(P::reference) {
                Some(inner) => self.curve_points(inner, a, b),
                None => vec![a, b],
            },
            "TRIMMED_CURVE" => match entity.args().get(1).and_then(P::reference) {
                Some(inner) => self.curve_points(inner, a, b),
                None => vec![a, b],
            },
            _ if entity.part("B_SPLINE_CURVE").is_some()
                || entity.kind() == "B_SPLINE_CURVE_WITH_KNOTS" =>
            {
                match bspline(step, entity) {
                    Some(spline) => spline.segment(a, b),
                    None => vec![a, b],
                }
            }
            _ => vec![a, b],
        }
    }
}

/// COLOUR_RGB reachable from a styled item.
fn find_colour(step: &Step, root: u64) -> Option<[u8; 3]> {
    let mut stack = vec![root];
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) || seen.len() > 64 {
            continue;
        }
        let entity = step.get(id)?;
        if entity.kind() == "COLOUR_RGB" {
            let a = entity.args();
            let c =
                |i: usize| (a.get(i).and_then(P::num).unwrap_or(0.7).clamp(0.0, 1.0) * 255.0) as u8;
            return Some([c(1), c(2), c(3)]);
        }
        if entity.kind() == "DRAUGHTING_PRE_DEFINED_COLOUR" {
            return Some([200, 200, 200]);
        }
        // Do not walk from a styled item into the geometry it styles.
        let args: Vec<&P> = if matches!(entity.kind(), "STYLED_ITEM" | "OVER_RIDING_STYLED_ITEM") {
            entity.args().get(1).into_iter().collect()
        } else {
            entity.parts.iter().flat_map(|(_, a)| a.iter()).collect()
        };
        for arg in args {
            collect_refs(arg, &mut stack);
        }
    }
    None
}

fn collect_refs(p: &P, out: &mut Vec<u64>) {
    match p {
        P::Ref(r) => out.push(*r),
        P::List(items) | P::Typed(_, items) => items.iter().for_each(|i| collect_refs(i, out)),
        _ => {}
    }
}

/// World placements of every shape representation, from assembly relationships.
fn representation_placements(step: &Step) -> HashMap<u64, Vec<Xf>> {
    // child representation -> (parent representation, child-to-parent transform)
    let mut children: HashMap<u64, Vec<(u64, Xf)>> = HashMap::new();
    let mut is_child = std::collections::HashSet::new();
    let mut same: Vec<(u64, u64)> = Vec::new();
    for entity in step.entities.values() {
        let Some(rel) = entity.part("REPRESENTATION_RELATIONSHIP") else {
            continue;
        };
        let (Some(rep1), Some(rep2)) = (
            rel.get(2).and_then(P::reference),
            rel.get(3).and_then(P::reference),
        ) else {
            continue;
        };
        let transform = entity
            .part("REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION")
            .and_then(|a| a.first()?.reference())
            .and_then(|idt| {
                let args = step.args(idt);
                let f1 = step.frame(args.get(2)?.reference()?)?;
                let f2 = step.frame(args.get(3)?.reference()?)?;
                Some(Xf::from_frame(&f1).inverse().then(&Xf::from_frame(&f2)))
            });
        match transform {
            Some(xf) => {
                children.entry(rep2).or_default().push((rep1, xf));
                is_child.insert(rep1);
            }
            None => same.push((rep1, rep2)),
        }
    }
    // Representations tied without a transform share placements: treat each as a
    // child of the other with identity, avoiding loops below.
    for (a, b) in same {
        if !is_child.contains(&a) {
            children.entry(b).or_default().push((a, Xf::ID));
            is_child.insert(a);
        } else if !is_child.contains(&b) {
            children.entry(a).or_default().push((b, Xf::ID));
            is_child.insert(b);
        }
    }
    let mut out: HashMap<u64, Vec<Xf>> = HashMap::new();
    let roots: Vec<u64> = children
        .keys()
        .copied()
        .filter(|r| !is_child.contains(r))
        .collect();
    let mut stack: Vec<(u64, Xf, usize)> = roots.into_iter().map(|r| (r, Xf::ID, 0)).collect();
    while let Some((rep, xf, depth)) = stack.pop() {
        out.entry(rep).or_default().push(xf);
        if depth > 16 {
            continue;
        }
        for (child, local) in children.get(&rep).into_iter().flatten() {
            stack.push((*child, local.then(&xf), depth + 1));
        }
    }
    out
}

struct Revolved {
    frame: Frame,
    kind: String,
    r1: f64,
    r2: f64,
}

impl Revolved {
    /// (angle, height-like parameter) of a point.
    fn param(&self, p: V3) -> (f64, f64) {
        let q = self.frame.local(p);
        let a = q.1.atan2(q.0);
        match self.kind.as_str() {
            "SPHERICAL_SURFACE" => (a, (q.2 / self.r1.max(1e-9)).clamp(-1.0, 1.0).asin()),
            "TOROIDAL_SURFACE" => {
                let rho = (q.0 * q.0 + q.1 * q.1).sqrt() - self.r1;
                (a, q.2.atan2(rho))
            }
            _ => (a, q.2),
        }
    }
    fn eval(&self, a: f64, h: f64) -> V3 {
        let (s, c) = a.sin_cos();
        let local = match self.kind.as_str() {
            "CONICAL_SURFACE" => {
                let r = self.r1 + h * self.r2.tan();
                V3(r * c, r * s, h)
            }
            "SPHERICAL_SURFACE" => {
                let r = self.r1 * h.cos();
                V3(r * c, r * s, self.r1 * h.sin())
            }
            "TOROIDAL_SURFACE" => {
                let r = self.r1 + self.r2 * h.cos();
                V3(r * c, r * s, self.r2 * h.sin())
            }
            _ => V3(self.r1 * c, self.r1 * s, h),
        };
        self.frame.world(local)
    }
}

impl ParamSurface for Revolved {
    fn eval(&self, u: f64, v: f64) -> V3 {
        Revolved::eval(self, u, v)
    }
    fn project(&self, p: V3, _seed: Option<(f64, f64)>) -> Option<(f64, f64)> {
        Some(self.param(p))
    }
    fn periods(&self) -> (Option<f64>, Option<f64>) {
        let tau = std::f64::consts::TAU;
        (Some(tau), (self.kind == "TOROIDAL_SURFACE").then_some(tau))
    }
    fn limits(&self) -> (Option<(f64, f64)>, Option<(f64, f64)>) {
        let half = std::f64::consts::FRAC_PI_2;
        (
            None,
            (self.kind == "SPHERICAL_SURFACE").then_some((-half, half)),
        )
    }
}

/// Most triangles one curved face may be refined into.
const MAX_FACE_TRIANGLES: usize = 6000;

/// Split every edge `split(a, b)` asks for, in rounds, at its parameter midpoint.
/// The decision depends on the edge alone, so both triangles sharing an edge
/// split it the same way and the mesh stays free of cracks (T-junctions).
fn refine(
    mut params: Vec<(f64, f64)>,
    mut tris: Vec<[usize; 3]>,
    split: impl Fn((f64, f64), (f64, f64)) -> bool,
    max_triangles: usize,
) -> (Vec<(f64, f64)>, Vec<[usize; 3]>) {
    for _ in 0..10 {
        let mut mids: HashMap<(usize, usize), usize> = HashMap::new();
        for t in &tris {
            for k in 0..3 {
                let (i, j) = (t[k], t[(k + 1) % 3]);
                let key = (i.min(j), i.max(j));
                if !mids.contains_key(&key) && split(params[key.0], params[key.1]) {
                    let (a, b) = (params[key.0], params[key.1]);
                    params.push(((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0));
                    mids.insert(key, params.len() - 1);
                }
            }
        }
        if mids.is_empty() {
            break;
        }
        let mut next = Vec::with_capacity(tris.len() * 3);
        for t in &tris {
            let mid = |k: usize| {
                let (i, j) = (t[k], t[(k + 1) % 3]);
                mids.get(&(i.min(j), i.max(j))).copied()
            };
            let m = [mid(0), mid(1), mid(2)];
            match m.iter().filter(|m| m.is_some()).count() {
                0 => next.push(*t),
                1 => {
                    // Rotate so the split edge is (0, 1).
                    let k = m.iter().position(Option::is_some).unwrap();
                    let (a, b, c) = (t[k], t[(k + 1) % 3], t[(k + 2) % 3]);
                    let ab = m[k].unwrap();
                    next.push([a, ab, c]);
                    next.push([ab, b, c]);
                }
                2 => {
                    // Rotate so the unsplit edge is (2, 0).
                    let k = m.iter().position(Option::is_none).unwrap();
                    let (a, b, c) = (t[(k + 1) % 3], t[(k + 2) % 3], t[k]);
                    let (ab, bc) = (m[(k + 1) % 3].unwrap(), m[(k + 2) % 3].unwrap());
                    next.push([ab, b, bc]);
                    next.push([a, ab, bc]);
                    next.push([a, bc, c]);
                }
                _ => {
                    let (ab, bc, ca) = (m[0].unwrap(), m[1].unwrap(), m[2].unwrap());
                    next.push([t[0], ab, ca]);
                    next.push([ab, t[1], bc]);
                    next.push([ca, bc, t[2]]);
                    next.push([ab, bc, ca]);
                }
            }
        }
        tris = next;
        if tris.len() > max_triangles {
            break;
        }
    }
    (params, tris)
}

/// Triangulate a face through the best-fit plane of its outer loop.
fn projected_face(loops: &[Vec<V3>]) -> Option<FaceMesh> {
    // Newell normal of the longest loop (the outer one, in practice).
    let outer = loops.iter().max_by_key(|l| l.len())?;
    let mut n = V3(0.0, 0.0, 0.0);
    for i in 0..outer.len() {
        let a = outer[i];
        let b = outer[(i + 1) % outer.len()];
        n = n + V3(
            (a.1 - b.1) * (a.2 + b.2),
            (a.2 - b.2) * (a.0 + b.0),
            (a.0 - b.0) * (a.1 + b.1),
        );
    }
    let z = n.unit().unwrap_or(V3(0.0, 0.0, 1.0));
    let x = (if z.0.abs() < 0.9 {
        V3(1.0, 0.0, 0.0)
    } else {
        V3(0.0, 1.0, 0.0)
    } - z * z.0.abs().min(1.0))
    .unit()?;
    let x = (x - z * x.dot(z)).unit()?;
    let y = z.cross(x);
    // Keep the original 3D points: map each 2D vertex back by index.
    let all: Vec<V3> = loops.iter().flatten().copied().collect();
    let rings: Vec<Vec<(f64, f64)>> = loops
        .iter()
        .map(|l| l.iter().map(|p| (p.dot(x), p.dot(y))).collect())
        .collect();
    let (flat, tris) = triangulate(&rings)?;
    // triangulate() keeps the input vertices first, in order; extra vertices
    // (none for ear clipping) would map to the plane.
    let points = flat
        .iter()
        .enumerate()
        .map(|(i, &(u, v))| all.get(i).copied().unwrap_or(x * u + y * v))
        .collect();
    // The face may be curved: let the renderer work its normals out.
    Some((points, tris, Vec::new()))
}

// ---------------------------------------------------------------------------
// B-spline curves

struct BSpline {
    degree: usize,
    points: Vec<V3>,
    weights: Option<Vec<f64>>,
    knots: Vec<f64>,
}

fn bspline(step: &Step, entity: &Entity) -> Option<BSpline> {
    let (degree, points_list, mults, knots) = if let Some(args) = entity.part("B_SPLINE_CURVE") {
        let with = entity.part("B_SPLINE_CURVE_WITH_KNOTS")?;
        (
            args.first()?.num()?,
            args.get(1)?.list(),
            with.first()?.list(),
            with.get(1)?.list(),
        )
    } else {
        let args = entity.args();
        (
            args.get(1)?.num()?,
            args.get(2)?.list(),
            args.get(6)?.list(),
            args.get(7)?.list(),
        )
    };
    let points: Vec<V3> = points_list
        .iter()
        .filter_map(|p| step.point(p.reference()?))
        .collect();
    let mut flat = Vec::new();
    for (m, k) in mults.iter().zip(knots.iter()) {
        for _ in 0..m.num()? as usize {
            flat.push(k.num()?);
        }
    }
    let weights = entity.part("RATIONAL_B_SPLINE_CURVE").and_then(|a| {
        Some(
            a.first()?
                .list()
                .iter()
                .filter_map(P::num)
                .collect::<Vec<f64>>(),
        )
    });
    let degree = degree as usize;
    (points.len() > degree && flat.len() == points.len() + degree + 1).then_some(BSpline {
        degree,
        points,
        weights,
        knots: flat,
    })
}

impl BSpline {
    fn eval(&self, t: f64) -> V3 {
        let p = self.degree;
        let n = self.points.len();
        let mut k = p;
        while k < n - 1 && t >= self.knots[k + 1] {
            k += 1;
        }
        let w = |i: usize| {
            self.weights
                .as_ref()
                .and_then(|w| w.get(i).copied())
                .unwrap_or(1.0)
        };
        let mut d: Vec<(V3, f64)> = (0..=p)
            .map(|j| {
                let i = j + k - p;
                (self.points[i] * w(i), w(i))
            })
            .collect();
        for r in 1..=p {
            for j in (r..=p).rev() {
                let i = j + k - p;
                let denom = self.knots[i + p + 1 - r] - self.knots[i];
                let alpha = if denom.abs() < 1e-12 {
                    0.0
                } else {
                    (t - self.knots[i]) / denom
                };
                d[j] = (
                    d[j - 1].0 * (1.0 - alpha) + d[j].0 * alpha,
                    d[j - 1].1 * (1.0 - alpha) + d[j].1 * alpha,
                );
            }
        }
        let (pt, wt) = d[p];
        pt * (1.0 / wt.max(1e-12))
    }

    /// Samples between the parameters nearest to `a` and `b`.
    fn segment(&self, a: V3, b: V3) -> Vec<V3> {
        let t0 = self.knots[self.degree];
        let t1 = self.knots[self.points.len()];
        let n = (self.points.len() * 4).clamp(8, 64);
        let samples: Vec<(f64, V3)> = (0..=n)
            .map(|i| {
                let t = t0 + (t1 - t0) * i as f64 / n as f64;
                (t, self.eval(t.min(t1 - 1e-12)))
            })
            .collect();
        let nearest = |q: V3| {
            samples
                .iter()
                .enumerate()
                .min_by(|x, y| (x.1.1 - q).len().total_cmp(&(y.1.1 - q).len()))
                .map(|(i, _)| i)
                .unwrap_or(0)
        };
        let (ia, ib) = (nearest(a), nearest(b));
        let mut pts: Vec<V3> = if ia < ib {
            samples[ia..=ib].iter().map(|s| s.1).collect()
        } else if (a - b).len() < 1e-9 {
            samples.iter().map(|s| s.1).collect()
        } else {
            // Closed curve crossing its start: go round.
            samples[ia..]
                .iter()
                .chain(samples[..=ib].iter())
                .map(|s| s.1)
                .collect()
        };
        if pts.is_empty() {
            return vec![a, b];
        }
        pts[0] = a;
        let last = pts.len() - 1;
        if last == 0 {
            pts.push(b);
        } else {
            pts[last] = b;
        }
        pts
    }
}

// ---------------------------------------------------------------------------
// B-spline surfaces
//
// Faces are triangulated in the surface's (u, v) parameters: every boundary
// point is projected onto the surface, the parameter polygon is triangulated and
// refined until it follows the surface, and every vertex is evaluated on it.

/// Non-zero basis functions of degree `p` at `t`: index of the first one and
/// their values (NURBS Book A2.2).
fn basis(knots: &[f64], p: usize, n: usize, t: f64) -> (usize, Vec<f64>) {
    let t = t.clamp(knots[p], knots[n]);
    let mut k = p;
    while k < n - 1 && t >= knots[k + 1] {
        k += 1;
    }
    let mut values = vec![0.0; p + 1];
    let mut left = vec![0.0; p + 1];
    let mut right = vec![0.0; p + 1];
    values[0] = 1.0;
    for j in 1..=p {
        left[j] = t - knots[k + 1 - j];
        right[j] = knots[k + j] - t;
        let mut saved = 0.0;
        for r in 0..j {
            let denom = right[r + 1] + left[j - r];
            let temp = if denom.abs() < 1e-14 {
                0.0
            } else {
                values[r] / denom
            };
            values[r] = saved + right[r + 1] * temp;
            saved = left[j - r] * temp;
        }
        values[j] = saved;
    }
    (k - p, values)
}

struct SplineSurface {
    du: usize,
    dv: usize,
    nu: usize,
    nv: usize,
    /// Control points, row `i` (u) major: `points[i * nv + j]`.
    points: Vec<V3>,
    weights: Option<Vec<f64>>,
    ku: Vec<f64>,
    kv: Vec<f64>,
}

fn knot_vector(mults: &[P], knots: &[P]) -> Option<Vec<f64>> {
    let mut flat = Vec::new();
    for (m, k) in mults.iter().zip(knots) {
        for _ in 0..m.num()? as usize {
            flat.push(k.num()?);
        }
    }
    Some(flat)
}

fn spline_surface(step: &Step, entity: &Entity) -> Option<SplineSurface> {
    let (du, dv, grid, knots) = if let Some(args) = entity.part("B_SPLINE_SURFACE") {
        let with = entity.part("B_SPLINE_SURFACE_WITH_KNOTS")?;
        (
            args.first()?.num()?,
            args.get(1)?.num()?,
            args.get(2)?.list(),
            with,
        )
    } else {
        let args = entity.args();
        (
            args.get(1)?.num()?,
            args.get(2)?.num()?,
            args.get(3)?.list(),
            args.get(8..)?,
        )
    };
    let (du, dv) = (du as usize, dv as usize);
    let nu = grid.len();
    let nv = grid.first()?.list().len();
    let mut points = Vec::with_capacity(nu * nv);
    for row in grid {
        let row = row.list();
        if row.len() != nv {
            return None;
        }
        for p in row {
            points.push(step.point(p.reference()?)?);
        }
    }
    let ku = knot_vector(knots.first()?.list(), knots.get(2)?.list())?;
    let kv = knot_vector(knots.get(1)?.list(), knots.get(3)?.list())?;
    let weights = entity.part("RATIONAL_B_SPLINE_SURFACE").and_then(|a| {
        let w: Vec<f64> = a
            .first()?
            .list()
            .iter()
            .flat_map(|row| row.list().iter().filter_map(P::num))
            .collect();
        (w.len() == nu * nv && w.iter().all(|&w| w > 0.0)).then_some(w)
    });
    (nu > du && nv > dv && ku.len() == nu + du + 1 && kv.len() == nv + dv + 1).then_some(
        SplineSurface {
            du,
            dv,
            nu,
            nv,
            points,
            weights,
            ku,
            kv,
        },
    )
}

impl SplineSurface {
    fn u_range(&self) -> (f64, f64) {
        (self.ku[self.du], self.ku[self.nu])
    }
    fn v_range(&self) -> (f64, f64) {
        (self.kv[self.dv], self.kv[self.nv])
    }

    fn eval(&self, u: f64, v: f64) -> V3 {
        let (iu, bu) = basis(&self.ku, self.du, self.nu, u);
        let (iv, bv) = basis(&self.kv, self.dv, self.nv, v);
        let mut sum = V3(0.0, 0.0, 0.0);
        let mut wsum = 0.0;
        for (a, nu) in bu.iter().enumerate() {
            for (b, nv) in bv.iter().enumerate() {
                let k = (iu + a) * self.nv + iv + b;
                let w = self.weights.as_ref().map_or(1.0, |w| w[k]) * nu * nv;
                sum = sum + self.points[k] * w;
                wsum += w;
            }
        }
        sum * (1.0 / if wsum.abs() < 1e-14 { 1.0 } else { wsum })
    }

    /// Size of the control net: tolerances scale with it.
    fn size(&self) -> f64 {
        let (mut lo, mut hi) = (self.points[0], self.points[0]);
        for p in &self.points {
            lo = V3(lo.0.min(p.0), lo.1.min(p.1), lo.2.min(p.2));
            hi = V3(hi.0.max(p.0), hi.1.max(p.1), hi.2.max(p.2));
        }
        (hi - lo).len().max(1e-6)
    }

    /// Periods of the directions in which the surface closes on itself.
    fn periods(&self, tol: f64) -> (Option<f64>, Option<f64>) {
        let ((u0, u1), (v0, v1)) = (self.u_range(), self.v_range());
        let closes = |f: &dyn Fn(f64) -> (V3, V3)| {
            (0..=4).all(|i| {
                let (a, b) = f(i as f64 / 4.0);
                (a - b).len() < tol
            })
        };
        let u = closes(&|t| {
            let v = v0 + (v1 - v0) * t;
            (self.eval(u0, v), self.eval(u1, v))
        });
        let v = closes(&|t| {
            let u = u0 + (u1 - u0) * t;
            (self.eval(u, v0), self.eval(u, v1))
        });
        (u.then_some(u1 - u0), v.then_some(v1 - v0))
    }

    /// Parameters of the surface point nearest `p`, from `start` (Gauss-Newton).
    fn newton(&self, p: V3, start: (f64, f64)) -> ((f64, f64), f64) {
        let ((u0, u1), (v0, v1)) = (self.u_range(), self.v_range());
        let (hu, hv) = ((u1 - u0) * 1e-6, (v1 - v0) * 1e-6);
        let (mut u, mut v) = start;
        for _ in 0..12 {
            let s = self.eval(u, v);
            let r = p - s;
            let su = (self.eval((u + hu).min(u1), v) - self.eval((u - hu).max(u0), v))
                * (1.0 / ((u + hu).min(u1) - (u - hu).max(u0)));
            let sv = (self.eval(u, (v + hv).min(v1)) - self.eval(u, (v - hv).max(v0)))
                * (1.0 / ((v + hv).min(v1) - (v - hv).max(v0)));
            let (a, b, c) = (su.dot(su), su.dot(sv), sv.dot(sv));
            let det = a * c - b * b;
            if det.abs() < 1e-24 {
                break;
            }
            let (ru, rv) = (su.dot(r), sv.dot(r));
            let (du, dv) = ((c * ru - b * rv) / det, (a * rv - b * ru) / det);
            let (nu, nv) = ((u + du).clamp(u0, u1), (v + dv).clamp(v0, v1));
            let moved = ((nu - u) / (u1 - u0)).abs() + ((nv - v) / (v1 - v0)).abs();
            u = nu;
            v = nv;
            if moved < 1e-10 {
                break;
            }
        }
        ((u, v), (self.eval(u, v) - p).len())
    }

    /// The surface with its closed directions found, ready for `param_face`.
    fn closed(&self) -> ClosedSpline<'_> {
        let size = self.size();
        let (pu, pv) = self.periods(size * 1e-5);
        let ((u0, u1), (v0, v1)) = (self.u_range(), self.v_range());
        let n = |spans: usize| (spans * 4).clamp(8, 48);
        let (gu, gv) = (n(self.nu - self.du), n(self.nv - self.dv));
        let mut grid = Vec::with_capacity((gu + 1) * (gv + 1));
        for i in 0..=gu {
            for j in 0..=gv {
                let (u, v) = (
                    u0 + (u1 - u0) * i as f64 / gu as f64,
                    v0 + (v1 - v0) * j as f64 / gv as f64,
                );
                grid.push(((u, v), self.eval(u, v)));
            }
        }
        ClosedSpline {
            surface: self,
            size,
            pu,
            pv,
            grid,
        }
    }
}

/// A B-spline surface with its periods and a coarse grid of samples to start
/// projections from.
struct ClosedSpline<'a> {
    surface: &'a SplineSurface,
    size: f64,
    pu: Option<f64>,
    pv: Option<f64>,
    grid: Vec<((f64, f64), V3)>,
}

impl ParamSurface for ClosedSpline<'_> {
    fn eval(&self, u: f64, v: f64) -> V3 {
        let ((u0, _), (v0, _)) = (self.surface.u_range(), self.surface.v_range());
        let wrap = |value: f64, period: Option<f64>, lo: f64| match period {
            Some(p) => lo + (value - lo).rem_euclid(p),
            None => value,
        };
        self.surface
            .eval(wrap(u, self.pu, u0), wrap(v, self.pv, v0))
    }
    fn project(&self, p: V3, seed: Option<(f64, f64)>) -> Option<(f64, f64)> {
        let tol = self.size * 1e-4;
        let mut best = seed.map(|s| self.surface.newton(p, s));
        if best.is_none_or(|(_, d)| d > tol) {
            let start = self
                .grid
                .iter()
                .min_by(|a, b| (a.1 - p).len().total_cmp(&(b.1 - p).len()))?
                .0;
            let found = self.surface.newton(p, start);
            if best.is_none_or(|(_, d)| found.1 < d) {
                best = Some(found);
            }
        }
        // A boundary point far off the surface: the face is not on it.
        best.filter(|(_, d)| *d < (self.size * 0.02).max(0.01))
            .map(|(uv, _)| uv)
    }
    fn periods(&self) -> (Option<f64>, Option<f64>) {
        (self.pu, self.pv)
    }
    fn limits(&self) -> (Option<(f64, f64)>, Option<(f64, f64)>) {
        (
            self.pu.is_none().then(|| self.surface.u_range()),
            self.pv.is_none().then(|| self.surface.v_range()),
        )
    }
}

// ---------------------------------------------------------------------------
// Faces meshed in surface parameters
//
// Every boundary point is mapped to the surface's (u, v); the parameter polygon
// is triangulated, refined until its edges follow the surface, and every vertex
// is evaluated on the real surface. Closed directions (seams) and poles are
// handled, so revolved and free-form bodies stay smooth.

trait ParamSurface {
    fn eval(&self, u: f64, v: f64) -> V3;
    /// Parameters of `p` (near the previous boundary point's `seed`).
    fn project(&self, p: V3, seed: Option<(f64, f64)>) -> Option<(f64, f64)>;
    /// Periods of the directions that close on themselves.
    fn periods(&self) -> (Option<f64>, Option<f64>);
    /// Parameter ranges of bounded directions.
    fn limits(&self) -> (Option<(f64, f64)>, Option<(f64, f64)>);
}

/// Triangles of a face bounded by `loops` on `surface`, following it to about
/// 1/2000 of `scale` (the model's size).
fn param_face(surface: &impl ParamSurface, loops: &[Vec<V3>], scale: f64) -> Option<FaceMesh> {
    let (pu, pv) = surface.periods();
    let (lu, lv) = surface.limits();
    // Typical extent of each parameter, for finite differences.
    let extent = |period: Option<f64>, limits: Option<(f64, f64)>| {
        period
            .or(limits.map(|(lo, hi)| hi - lo))
            .filter(|e| *e > 0.0)
            .unwrap_or(1.0)
    };
    let (eu, ev) = (extent(pu, lu), extent(pv, lv));
    let exact = scale * 1e-7;
    // A point where a whole row of parameters meets (a pole, apex).
    let pole = |(u, v): (f64, f64), along_u: bool| {
        let d = 1e-3 * if along_u { eu } else { ev };
        let (a, b) = if along_u {
            (surface.eval(u - d, v), surface.eval(u + d, v))
        } else {
            (surface.eval(u, v - d), surface.eval(u, v + d))
        };
        (a - b).len() < exact
    };

    let mut rings: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut full: Vec<(bool, bool)> = Vec::new();
    for l in loops {
        let mut raw: Vec<(f64, f64)> = Vec::with_capacity(l.len());
        let mut seed = None;
        for p in l {
            let uv = surface.project(*p, seed)?;
            seed = Some(uv);
            raw.push(uv);
        }
        // Poles: points where a whole row of parameters meets, so one of their
        // parameters means nothing. Start the loop at an ordinary point.
        let poles: Vec<(bool, bool)> = raw
            .iter()
            .map(|&uv| (pole(uv, true), pole(uv, false)))
            .collect();
        let Some(first) = poles.iter().position(|&(a, b)| !a && !b) else {
            continue;
        };
        raw.rotate_left(first);
        let mut poles = poles;
        poles.rotate_left(first);
        let is_pole = |i: usize| poles[i].0 || poles[i].1;

        // Unwrap across the seams of closed directions, following ordinary points.
        let unwrap = |value: f64, reference: f64, period: Option<f64>| match period {
            Some(period) => value - ((value - reference) / period).round() * period,
            None => value,
        };
        let mut ring = raw.clone();
        let mut last = ring[0];
        for i in 1..ring.len() {
            if is_pole(i) {
                continue;
            }
            ring[i] = (unwrap(ring[i].0, last.0, pu), unwrap(ring[i].1, last.1, pv));
            last = ring[i];
        }
        // A pole stands in for a run along the collapsed row: from the previous
        // ordinary point's value of that parameter to the next one's (the loop's
        // first point after the last one, so the loop still closes).
        let count = ring.len();
        let mut fixed = Vec::with_capacity(count + 4);
        let mut prev = ring[0];
        for i in 0..count {
            if !is_pole(i) {
                fixed.push(ring[i]);
                prev = ring[i];
                continue;
            }
            if i > 0 && is_pole(i - 1) {
                // One run along the pole, however many points sit on it.
                continue;
            }
            let next = (i + 1..count)
                .find(|&k| !is_pole(k))
                .map_or(ring[0], |k| ring[k]);
            let cur = ring[i];
            if poles[i].0 {
                let v = unwrap(cur.1, prev.1, pv);
                fixed.push((prev.0, v));
                fixed.push((next.0, v));
            } else {
                let u = unwrap(cur.0, prev.0, pu);
                fixed.push((u, prev.1));
                fixed.push((u, next.1));
            }
        }
        let ring = fixed;
        if ring.len() < 2 {
            continue;
        }
        let (first, last) = (ring[0], *ring.last()?);
        // Closing the loop crosses the seam: it runs all the way round.
        let turns = |period: Option<f64>, d: f64| period.is_some_and(|p| d.abs() > p / 2.0);
        full.push((turns(pu, last.0 - first.0), turns(pv, last.1 - first.1)));
        rings.push(ring);
    }
    if rings.is_empty() {
        return None;
    }

    // Loops running all the way round a closed direction have no seam edge: close
    // them along the seam into one parameter polygon.
    let wraps_u = full.iter().filter(|f| f.0).count();
    let wraps_v = full.iter().filter(|f| f.1).count();
    let swap = |r: &mut Vec<(f64, f64)>| r.iter_mut().for_each(|p| *p = (p.1, p.0));
    let mut all: Vec<Vec<(f64, f64)>> = if wraps_u > 0 || wraps_v > 0 {
        let along_u = wraps_u >= wraps_v;
        let (period, limits) = if along_u { (pu?, lv) } else { (pv?, lu) };
        let mut wrapping = Vec::new();
        let mut holes = Vec::new();
        for (mut ring, f) in rings.into_iter().zip(&full) {
            if !along_u {
                swap(&mut ring);
            }
            if if along_u { f.0 } else { f.1 } {
                wrapping.push(ring);
            } else {
                holes.push(ring);
            }
        }
        // A wrapping loop as a strip in increasing u, closed by its start + period.
        let strip = |mut ring: Vec<(f64, f64)>| -> Vec<(f64, f64)> {
            if ring.last().unwrap().0 < ring[0].0 {
                ring.reverse();
            }
            let first = ring[0];
            ring.push((first.0 + period, first.1));
            ring
        };
        let outline = match wrapping.len() {
            1 => {
                // The other side is the edge of a bounded parameter (a pole).
                let (lo, hi) = limits?;
                let ring = strip(wrapping.pop()?);
                let mean = ring.iter().map(|p| p.1).sum::<f64>() / ring.len() as f64;
                let edge = if mean - lo < hi - mean { lo } else { hi };
                let (start, end) = (ring[0].0, ring.last()?.0);
                let mut outline = ring;
                outline.push((end, edge));
                outline.push((start, edge));
                outline
            }
            2 => {
                let b = strip(wrapping.pop()?);
                let a = strip(wrapping.pop()?);
                let mut outline = a;
                outline.extend(b.into_iter().rev());
                outline
            }
            _ => return None,
        };
        // Holes go where the outline is, a period along if need be.
        let start = outline.iter().map(|p| p.0).fold(f64::MAX, f64::min);
        for hole in &mut holes {
            let mean = hole.iter().map(|p| p.0).sum::<f64>() / hole.len() as f64;
            let shift = ((start - mean) / period).ceil() * period;
            hole.iter_mut().for_each(|p| p.0 += shift);
        }
        let mut all = vec![outline];
        all.extend(holes);
        if !along_u {
            all.iter_mut().for_each(swap);
        }
        all
    } else {
        rings
    };
    // Holes inside the outline may sit a period away from it.
    if let Some(outer) = all
        .iter()
        .map(|r| ring_area(r).abs())
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
    {
        let (lo_u, lo_v) = all[outer]
            .iter()
            .fold((f64::MAX, f64::MAX), |m, p| (m.0.min(p.0), m.1.min(p.1)));
        for (i, ring) in all.iter_mut().enumerate() {
            if i == outer {
                continue;
            }
            for (period, lo, along_u) in [(pu, lo_u, true), (pv, lo_v, false)] {
                let Some(period) = period else { continue };
                let min = ring
                    .iter()
                    .map(|p| if along_u { p.0 } else { p.1 })
                    .fold(f64::MAX, f64::min);
                let shift = ((lo - min) / period).ceil() * period;
                let shift = if min + shift - lo > period - 1e-9 {
                    shift - period
                } else {
                    shift
                };
                if shift != 0.0 {
                    ring.iter_mut()
                        .for_each(|p| if along_u { p.0 += shift } else { p.1 += shift });
                }
            }
        }
    }

    // Triangulate with parameters scaled to rough lengths, so triangles keep sane
    // shapes, then refine until edges follow the surface.
    let (mut su, mut sv, mut count) = (0.0, 0.0, 0.0);
    let samples: Vec<(f64, f64)> = all.iter().flatten().copied().collect();
    let stride = (samples.len() / 24).max(1);
    for &(u, v) in samples.iter().step_by(stride) {
        let (du, dv) = (1e-4 * eu, 1e-4 * ev);
        su += (surface.eval(u + du, v) - surface.eval(u - du, v)).len() / (2.0 * du);
        sv += (surface.eval(u, v + dv) - surface.eval(u, v - dv)).len() / (2.0 * dv);
        count += 1.0;
    }
    let floor = scale * 1e-6;
    let (su, sv) = ((su / count).max(floor), (sv / count).max(floor));
    let scaled: Vec<Vec<(f64, f64)>> = all
        .iter()
        .map(|r| r.iter().map(|&(u, v)| (u * su, v * sv)).collect())
        .collect();
    let (flat, tris) = triangulate(&scaled)?;
    let at = |(x, y): (f64, f64)| surface.eval(x / su, y / sv);
    let deviation = scale * 5e-4;
    let longest = scale / 8.0;
    let (flat, tris) = refine(
        flat,
        tris,
        |a, b| {
            let (pa, pb) = (at(a), at(b));
            let mid = at(((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0));
            (mid - (pa + pb) * 0.5).len() > deviation || (pa - pb).len() > longest
        },
        MAX_FACE_TRIANGLES,
    );
    let points: Vec<V3> = flat.iter().map(|&q| at(q)).collect();
    // Surface normals from the parameter derivatives (their sign does not
    // matter to the renderer).
    let (hu, hv) = (1e-5 * eu * su, 1e-5 * ev * sv);
    let normals = flat
        .iter()
        .map(|&(x, y)| {
            let du = at((x + hu, y)) - at((x - hu, y));
            let dv = at((x, y + hv)) - at((x, y - hv));
            // At a pole or a collapsed edge one derivative vanishes and the cross
            // product is noise: leave the normal to the renderer.
            let (lu, lv) = (du.len(), dv.len());
            if lu < 1e-3 * lv || lv < 1e-3 * lu {
                return V3(0.0, 0.0, 0.0);
            }
            du.cross(dv).unit().unwrap_or(V3(0.0, 0.0, 0.0))
        })
        .collect();
    Some((points, tris, normals))
}

fn ring_area(ring: &[(f64, f64)]) -> f64 {
    (0..ring.len())
        .map(|i| {
            let (p, q) = (ring[i], ring[(i + 1) % ring.len()]);
            p.0 * q.1 - q.0 * p.1
        })
        .sum::<f64>()
        / 2.0
}

// ---------------------------------------------------------------------------
// Polygon triangulation (ear clipping, holes bridged into the outer ring)

/// Triangulate rings (the one enclosing the most area is the outer one, the rest
/// are holes). Returns the vertices in input order and triangles indexing them,
/// counter-clockwise.
///
/// Earcut (z-order hashed ear clipping) does the work: faces with hundreds of
/// holes, like the substrate of a BGA under its balls, took minutes with plain
/// ear clipping and take milliseconds with it. The plain version stays as the
/// fallback for outlines earcut rejects.
pub(crate) fn triangulate(rings: &[Vec<(f64, f64)>]) -> Option<(Vec<(f64, f64)>, Vec<[usize; 3]>)> {
    let ring_area_abs = |ring: &Vec<(f64, f64)>| ring_area(ring).abs();
    let outer = (0..rings.len())
        .max_by(|&a, &b| ring_area_abs(&rings[a]).total_cmp(&ring_area_abs(&rings[b])))?;
    if rings[outer].len() < 3 {
        return None;
    }
    // Earcut wants the outer ring first; remember where each input vertex went.
    let mut starts = Vec::with_capacity(rings.len());
    let mut offset = 0;
    for ring in rings {
        starts.push(offset);
        offset += ring.len();
    }
    let order: Vec<usize> = std::iter::once(outer)
        .chain((0..rings.len()).filter(|&i| i != outer && rings[i].len() >= 3))
        .collect();
    let mut data = Vec::with_capacity(offset * 2);
    let mut index = Vec::with_capacity(offset);
    let mut holes = Vec::new();
    for (k, &r) in order.iter().enumerate() {
        if k > 0 {
            holes.push(index.len());
        }
        for (j, p) in rings[r].iter().enumerate() {
            data.push(p.0);
            data.push(p.1);
            index.push(starts[r] + j);
        }
    }
    if let Ok(flat) = earcutr::earcut(&data, &holes, 2) {
        if !flat.is_empty() {
            let verts: Vec<(f64, f64)> = rings.iter().flatten().copied().collect();
            let cross = |a: (f64, f64), b: (f64, f64), c: (f64, f64)| {
                (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
            };
            let tris = flat
                .chunks_exact(3)
                .map(|t| {
                    let (a, b, c) = (index[t[0]], index[t[1]], index[t[2]]);
                    if cross(verts[a], verts[b], verts[c]) < 0.0 {
                        [a, c, b]
                    } else {
                        [a, b, c]
                    }
                })
                .collect();
            return Some((verts, tris));
        }
    }
    triangulate_ear_clip(rings)
}

/// Plain ear clipping with the holes bridged into the outer ring.
fn triangulate_ear_clip(rings: &[Vec<(f64, f64)>]) -> Option<(Vec<(f64, f64)>, Vec<[usize; 3]>)> {
    let mut verts: Vec<(f64, f64)> = Vec::new();
    let mut ring_idx: Vec<Vec<usize>> = Vec::new();
    for ring in rings {
        let start = verts.len();
        verts.extend(ring.iter().copied());
        ring_idx.push((start..verts.len()).collect());
    }
    let area = |idx: &[usize]| -> f64 {
        let mut a = 0.0;
        for i in 0..idx.len() {
            let p = verts[idx[i]];
            let q = verts[idx[(i + 1) % idx.len()]];
            a += p.0 * q.1 - q.0 * p.1;
        }
        a / 2.0
    };
    // STEP does not order a face's bounds: the outer one encloses the most area.
    let outer_at = (0..ring_idx.len()).max_by(|&a, &b| {
        area(&ring_idx[a])
            .abs()
            .total_cmp(&area(&ring_idx[b]).abs())
    })?;
    if ring_idx[outer_at].len() < 3 {
        return None;
    }
    // Outer counter-clockwise, holes clockwise.
    let mut outer = ring_idx[outer_at].clone();
    if area(&outer) < 0.0 {
        outer.reverse();
    }
    let mut holes: Vec<Vec<usize>> = ring_idx
        .iter()
        .enumerate()
        .filter(|&(i, h)| i != outer_at && h.len() >= 3)
        .map(|(_, h)| h)
        .map(|h| {
            let mut h = h.clone();
            if area(&h) > 0.0 {
                h.reverse();
            }
            h
        })
        .collect();
    // Bridge holes, rightmost first.
    holes.sort_by(|a, b| {
        let ma = a.iter().map(|&i| verts[i].0).fold(f64::MIN, f64::max);
        let mb = b.iter().map(|&i| verts[i].0).fold(f64::MIN, f64::max);
        mb.total_cmp(&ma)
    });
    for hole in holes {
        let (hi, &hv) = hole
            .iter()
            .enumerate()
            .max_by(|a, b| verts[*a.1].0.total_cmp(&verts[*b.1].0))?;
        let hp = verts[hv];
        // Nearest outer vertex to the right whose bridge does not cross the outer ring.
        let mut best: Option<(usize, f64)> = None;
        for (oi, &ov) in outer.iter().enumerate() {
            let op = verts[ov];
            let d = (op.0 - hp.0).powi(2) + (op.1 - hp.1).powi(2);
            if op.0 < hp.0 - 1e-12 && best.is_some() {
                continue;
            }
            let crosses = (0..outer.len()).any(|k| {
                let a = verts[outer[k]];
                let b = verts[outer[(k + 1) % outer.len()]];
                outer[k] != ov && outer[(k + 1) % outer.len()] != ov && segments_cross(hp, op, a, b)
            });
            if !crosses && best.is_none_or(|(_, bd)| d < bd) {
                best = Some((oi, d));
            }
        }
        let (oi, _) = best?;
        let ov = outer[oi];
        let mut merged = Vec::with_capacity(outer.len() + hole.len() + 2);
        merged.extend_from_slice(&outer[..=oi]);
        for k in 0..=hole.len() {
            merged.push(hole[(hi + k) % hole.len()]);
        }
        merged.push(ov);
        merged.extend_from_slice(&outer[oi + 1..]);
        outer = merged;
    }
    let tris = ear_clip(&verts, outer);
    (!tris.is_empty()).then_some((verts, tris))
}

fn segments_cross(p1: (f64, f64), p2: (f64, f64), q1: (f64, f64), q2: (f64, f64)) -> bool {
    let o = |a: (f64, f64), b: (f64, f64), c: (f64, f64)| {
        (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
    };
    let d1 = o(q1, q2, p1);
    let d2 = o(q1, q2, p2);
    let d3 = o(p1, p2, q1);
    let d4 = o(p1, p2, q2);
    d1 * d2 < -1e-18 && d3 * d4 < -1e-18
}

fn ear_clip(verts: &[(f64, f64)], mut poly: Vec<usize>) -> Vec<[usize; 3]> {
    let mut tris = Vec::with_capacity(poly.len());
    let cross = |a: (f64, f64), b: (f64, f64), c: (f64, f64)| {
        (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
    };
    // Every pass removes one vertex, so this bounds the loop without cutting it
    // short (the remaining outline must not be dropped).
    let mut guard = poly.len() + 16;
    // Resume the search where the last ear was cut: keeps big outlines near O(n²).
    let mut start = 0usize;
    while poly.len() > 3 && guard > 0 {
        guard -= 1;
        let n = poly.len();
        let mut clipped = false;
        for step in 0..n {
            let i = (start + step) % n;
            let (ia, ib, ic) = (poly[(i + n - 1) % n], poly[i], poly[(i + 1) % n]);
            let (a, b, c) = (verts[ia], verts[ib], verts[ic]);
            let turn = cross(a, b, c);
            if turn <= 1e-14 {
                continue;
            }
            let inside = poly.iter().any(|&j| {
                if j == ia || j == ib || j == ic {
                    return false;
                }
                let p = verts[j];
                if (p.0 - a.0).abs() < 1e-12 && (p.1 - a.1).abs() < 1e-12
                    || (p.0 - b.0).abs() < 1e-12 && (p.1 - b.1).abs() < 1e-12
                    || (p.0 - c.0).abs() < 1e-12 && (p.1 - c.1).abs() < 1e-12
                {
                    return false;
                }
                cross(a, b, p) >= -1e-14 && cross(b, c, p) >= -1e-14 && cross(c, a, p) >= -1e-14
            });
            if inside {
                continue;
            }
            tris.push([ia, ib, ic]);
            poly.remove(i);
            start = i.saturating_sub(1);
            clipped = true;
            break;
        }
        if !clipped {
            // Degenerate or self-touching outline: drop the flattest vertex.
            let n = poly.len();
            let (i, _) = (0..n)
                .map(|i| {
                    let (a, b, c) = (
                        verts[poly[(i + n - 1) % n]],
                        verts[poly[i]],
                        verts[poly[(i + 1) % n]],
                    );
                    (i, cross(a, b, c).abs())
                })
                .min_by(|x, y| x.1.total_cmp(&y.1))
                .unwrap();
            let (ia, ib, ic) = (poly[(i + n - 1) % n], poly[i], poly[(i + 1) % n]);
            if cross(verts[ia], verts[ib], verts[ic]).abs() > 1e-14 {
                tris.push([ia, ib, ic]);
            }
            poly.remove(i);
        }
    }
    if poly.len() == 3 {
        tris.push([poly[0], poly[1], poly[2]]);
    }
    tris
}

#[cfg(test)]
mod tests {
    use super::*;

    const CUBE_FACE: &str = "ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\n\
#1=CARTESIAN_POINT('',(0.,0.,0.));\n#2=CARTESIAN_POINT('',(1.,0.,0.));\n#3=CARTESIAN_POINT('',(1.,1.,0.));\n#4=CARTESIAN_POINT('',(0.,1.,0.));\n\
#11=VERTEX_POINT('',#1);\n#12=VERTEX_POINT('',#2);\n#13=VERTEX_POINT('',#3);\n#14=VERTEX_POINT('',#4);\n\
#21=EDGE_CURVE('',#11,#12,#90,.T.);\n#22=EDGE_CURVE('',#12,#13,#90,.T.);\n#23=EDGE_CURVE('',#13,#14,#90,.T.);\n#24=EDGE_CURVE('',#14,#11,#90,.T.);\n\
#31=ORIENTED_EDGE('',*,*,#21,.T.);\n#32=ORIENTED_EDGE('',*,*,#22,.T.);\n#33=ORIENTED_EDGE('',*,*,#23,.T.);\n#34=ORIENTED_EDGE('',*,*,#24,.T.);\n\
#40=EDGE_LOOP('',(#31,#32,#33,#34));\n#41=FACE_OUTER_BOUND('',#40,.T.);\n\
#50=DIRECTION('',(0.,0.,1.));\n#51=DIRECTION('',(1.,0.,0.));\n#52=AXIS2_PLACEMENT_3D('',#1,#50,#51);\n#53=PLANE('',#52);\n\
#60=ADVANCED_FACE('',(#41),#53,.T.);\n#70=OPEN_SHELL('',(#60));\n#80=SHELL_BASED_SURFACE_MODEL('',(#70));\n#90=LINE('',#1,#91);\n#91=VECTOR('',#50,1.);\n\
#95=COLOUR_RGB('',1.,0.,0.);\n#96=FILL_AREA_STYLE_COLOUR('',#95);\n#97=STYLED_ITEM('',(#96),#60);\nENDSEC;\n";

    #[test]
    fn square_face_becomes_two_red_triangles() {
        let mesh = tessellate(CUBE_FACE.as_bytes()).unwrap();
        assert_eq!(mesh.triangles.len(), 2);
        assert_eq!(mesh.colors[0], [255, 0, 0]);
        let (lo, hi) = mesh.bounds().unwrap();
        assert_eq!((lo, hi), ([0.0, 0.0, 0.0], [1.0, 1.0, 0.0]));
    }

    #[test]
    fn polygon_with_hole() {
        let outer = vec![(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)];
        let hole = vec![(1.0, 1.0), (1.0, 3.0), (3.0, 3.0), (3.0, 1.0)];
        let (verts, tris) = triangulate(&[outer, hole]).unwrap();
        let area: f64 = tris
            .iter()
            .map(|t| {
                let (a, b, c) = (verts[t[0]], verts[t[1]], verts[t[2]]);
                ((b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)).abs() / 2.0
            })
            .sum();
        assert!((area - 12.0).abs() < 1e-9, "{area}");
    }

    fn area(verts: &[(f64, f64)], tris: &[[usize; 3]]) -> f64 {
        tris.iter()
            .map(|t| {
                let (a, b, c) = (verts[t[0]], verts[t[1]], verts[t[2]]);
                ((b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)).abs() / 2.0
            })
            .sum()
    }

    #[test]
    fn outer_ring_need_not_come_first() {
        // STEP lists a face's bounds in any order.
        let hole = vec![(1.0, 1.0), (1.0, 3.0), (3.0, 3.0), (3.0, 1.0)];
        let outer = vec![(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)];
        let (verts, tris) = triangulate(&[hole, outer]).unwrap();
        assert!((area(&verts, &tris) - 12.0).abs() < 1e-9);
    }

    #[test]
    fn large_outlines_are_triangulated_completely() {
        // A 600-point star: the ear clipper must not give up part way.
        let ring: Vec<(f64, f64)> = (0..600)
            .map(|i| {
                let a = std::f64::consts::TAU * i as f64 / 600.0;
                let r = if i % 2 == 0 { 10.0 } else { 9.0 };
                (r * a.cos(), r * a.sin())
            })
            .collect();
        let expected: f64 = (0..600)
            .map(|i| {
                let (p, q) = (ring[i], ring[(i + 1) % 600]);
                p.0 * q.1 - q.0 * p.1
            })
            .sum::<f64>()
            / 2.0;
        let (verts, tris) = triangulate(&[ring]).unwrap();
        assert_eq!(tris.len(), 598);
        assert!((area(&verts, &tris) - expected).abs() < 1e-6);
    }

    fn mesh_area(points: &[V3], tris: &[[usize; 3]]) -> f64 {
        tris.iter()
            .map(|t| {
                (points[t[1]] - points[t[0]])
                    .cross(points[t[2]] - points[t[0]])
                    .len()
                    / 2.0
            })
            .sum()
    }

    #[test]
    fn rational_spline_faces_follow_the_surface() {
        // Quarter cylinder (radius 2, height 3) as a rational quadratic x linear patch.
        let w = std::f64::consts::FRAC_1_SQRT_2;
        let row = |x: f64, y: f64| [V3(x, y, 0.0), V3(x, y, 3.0)];
        let points: Vec<V3> = [row(2.0, 0.0), row(2.0, 2.0), row(0.0, 2.0)].concat();
        let surface = SplineSurface {
            du: 2,
            dv: 1,
            nu: 3,
            nv: 2,
            points,
            weights: Some(vec![1.0, 1.0, w, w, 1.0, 1.0]),
            ku: vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            kv: vec![0.0, 0.0, 1.0, 1.0],
        };
        let mid = surface.eval(0.5, 0.5);
        assert!(((mid.0 * mid.0 + mid.1 * mid.1).sqrt() - 2.0).abs() < 1e-9);
        // Boundary: the two straight edges and two coarsely sampled arcs.
        let arc = |z: f64, reverse: bool| {
            let mut pts: Vec<V3> = (0..=4)
                .map(|i| {
                    let a = std::f64::consts::FRAC_PI_2 * i as f64 / 4.0;
                    V3(2.0 * a.cos(), 2.0 * a.sin(), z)
                })
                .collect();
            if reverse {
                pts.reverse();
            }
            pts
        };
        let mut boundary = arc(0.0, false);
        boundary.extend(arc(3.0, true));
        let (points, tris, _) = param_face(&surface.closed(), &[boundary], 10.0).unwrap();
        let expected = std::f64::consts::FRAC_PI_2 * 2.0 * 3.0;
        let got = mesh_area(&points, &tris);
        assert!(
            (got - expected).abs() < 0.01 * expected,
            "{got} vs {expected}"
        );
        for p in &points {
            assert!(((p.0 * p.0 + p.1 * p.1).sqrt() - 2.0).abs() < 1e-6);
        }
    }

    #[test]
    fn horn_torus_faces_wrap_through_the_pole() {
        // Half of a horn torus (r1 = r2), the shape of a dipped disc body: its
        // boundary runs round the tube twice through the pole on the axis.
        let frame = Frame {
            origin: V3(0.0, 0.0, 0.0),
            x: V3(1.0, 0.0, 0.0),
            y: V3(0.0, 1.0, 0.0),
            z: V3(0.0, 0.0, 1.0),
        };
        let torus = Revolved {
            frame,
            kind: "TOROIDAL_SURFACE".into(),
            r1: 2.0,
            r2: 2.0,
        };
        let meridian = |a: f64, reverse: bool| {
            let mut pts: Vec<V3> = (0..=24)
                .map(|i| {
                    torus.eval(
                        a,
                        -std::f64::consts::PI + std::f64::consts::TAU * i as f64 / 24.0,
                    )
                })
                .collect();
            if reverse {
                pts.reverse();
            }
            pts
        };
        let mut boundary = meridian(0.0, false);
        boundary.extend(meridian(std::f64::consts::PI, true));
        let (points, tris, _) = param_face(&torus, &[boundary], 10.0).unwrap();
        // Area of half a horn torus: 2π² r1 r2.
        let expected = 2.0 * std::f64::consts::PI.powi(2) * 2.0 * 2.0;
        let got = mesh_area(&points, &tris);
        assert!(
            (got - expected).abs() < 0.02 * expected,
            "{got} vs {expected}"
        );
    }

    #[test]
    fn parses_complex_entities_and_strings() {
        let e = parse_entity("( NAMED_UNIT(*) LENGTH_UNIT() SI_UNIT(.MILLI.,.METRE.) )").unwrap();
        assert_eq!(e.parts.len(), 3);
        let e = parse_entity("PRODUCT('it''s (x)', 'n', '', (#5))").unwrap();
        assert_eq!(e.args().len(), 4);
        assert_eq!(e.args()[3].list()[0].reference(), Some(5));
    }
}
