//! Minimal Mapbox Vector Tile (MVT) decoder. No dependencies.
//!
//! Only what the map needs: layers, feature geometry (as paths of integer tile
//! coordinates), and feature properties.

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Num(f64),
    Bool(bool),
}

#[derive(Debug, Clone, Default)]
pub struct Feature {
    /// 1 = point, 2 = linestring, 3 = polygon
    pub geom_type: u8,
    tags: Vec<u32>,
    geometry: Vec<u32>,
}

#[derive(Debug, Clone, Default)]
pub struct Layer {
    pub name: String,
    pub extent: u32,
    keys: Vec<String>,
    values: Vec<Value>,
    pub features: Vec<Feature>,
}

impl Layer {
    pub fn prop(&self, f: &Feature, key: &str) -> Option<&Value> {
        let ki = self.keys.iter().position(|k| k == key)? as u32;
        for pair in f.tags.chunks_exact(2) {
            if pair[0] == ki {
                return self.values.get(pair[1] as usize);
            }
        }
        None
    }

    pub fn str_prop(&self, f: &Feature, key: &str) -> Option<&str> {
        match self.prop(f, key) {
            Some(Value::Str(s)) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn num_prop(&self, f: &Feature, key: &str) -> Option<f64> {
        match self.prop(f, key) {
            Some(Value::Num(n)) => Some(*n),
            Some(Value::Bool(b)) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }
}

fn unzig(v: u32) -> i32 {
    ((v >> 1) as i32) ^ -((v & 1) as i32)
}

impl Feature {
    /// Decodes the geometry command stream into paths of tile coordinates.
    ///
    /// * points: one single-vertex path per point
    /// * lines: one path per linestring
    /// * polygons: one path per ring (not repeated-closed), in stream order
    pub fn paths(&self) -> Vec<Vec<(i32, i32)>> {
        let g = &self.geometry;
        let mut out: Vec<Vec<(i32, i32)>> = Vec::new();
        let mut cur: Vec<(i32, i32)> = Vec::new();
        let (mut x, mut y) = (0i32, 0i32);
        let mut i = 0usize;
        while i < g.len() {
            let cmd = g[i];
            i += 1;
            let id = cmd & 7;
            let count = (cmd >> 3) as usize;
            match id {
                1 | 2 => {
                    for _ in 0..count {
                        let (Some(&dx), Some(&dy)) = (g.get(i), g.get(i + 1)) else {
                            return out;
                        };
                        i += 2;
                        x = x.wrapping_add(unzig(dx));
                        y = y.wrapping_add(unzig(dy));
                        if id == 1 && !cur.is_empty() {
                            out.push(std::mem::take(&mut cur));
                        }
                        cur.push((x, y));
                    }
                }
                7 => {
                    if !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                }
                _ => break,
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Protobuf reading
// ---------------------------------------------------------------------------

struct Rd<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Rd<'a> {
    fn new(b: &'a [u8]) -> Self {
        Rd { b, p: 0 }
    }

    fn eof(&self) -> bool {
        self.p >= self.b.len()
    }

    fn varint(&mut self) -> Option<u64> {
        let mut result = 0u64;
        let mut shift = 0u32;
        loop {
            let byte = *self.b.get(self.p)?;
            self.p += 1;
            result |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                return Some(result);
            }
            shift += 7;
            if shift >= 64 {
                return None;
            }
        }
    }

    fn key(&mut self) -> Option<(u64, u64)> {
        let k = self.varint()?;
        Some((k >> 3, k & 7))
    }

    fn fixed(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.p.checked_add(n)?;
        let s = self.b.get(self.p..end)?;
        self.p = end;
        Some(s)
    }

    fn len_delim(&mut self) -> Option<&'a [u8]> {
        let n = self.varint()? as usize;
        self.fixed(n)
    }

    fn skip(&mut self, wire: u64) -> Option<()> {
        match wire {
            0 => {
                self.varint()?;
            }
            1 => {
                self.fixed(8)?;
            }
            2 => {
                self.len_delim()?;
            }
            5 => {
                self.fixed(4)?;
            }
            _ => return None,
        }
        Some(())
    }
}

fn packed_u32(bytes: &[u8], out: &mut Vec<u32>) -> Option<()> {
    let mut r = Rd::new(bytes);
    while !r.eof() {
        out.push(r.varint()? as u32);
    }
    Some(())
}

fn decode_value(bytes: &[u8]) -> Option<Value> {
    let mut r = Rd::new(bytes);
    let mut out = None;
    while !r.eof() {
        let (field, wire) = r.key()?;
        match (field, wire) {
            (1, 2) => out = Some(Value::Str(String::from_utf8_lossy(r.len_delim()?).into_owned())),
            (2, 5) => {
                let b = r.fixed(4)?;
                out = Some(Value::Num(f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64));
            }
            (3, 1) => {
                let b = r.fixed(8)?;
                out = Some(Value::Num(f64::from_le_bytes([
                    b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
                ])));
            }
            (4, 0) => out = Some(Value::Num(r.varint()? as i64 as f64)),
            (5, 0) => out = Some(Value::Num(r.varint()? as f64)),
            (6, 0) => {
                let v = r.varint()?;
                let s = ((v >> 1) as i64) ^ -((v & 1) as i64);
                out = Some(Value::Num(s as f64));
            }
            (7, 0) => out = Some(Value::Bool(r.varint()? != 0)),
            _ => r.skip(wire)?,
        }
    }
    out
}

fn decode_feature(bytes: &[u8]) -> Option<Feature> {
    let mut r = Rd::new(bytes);
    let mut f = Feature::default();
    while !r.eof() {
        let (field, wire) = r.key()?;
        match (field, wire) {
            (2, 2) => packed_u32(r.len_delim()?, &mut f.tags)?,
            (2, 0) => f.tags.push(r.varint()? as u32),
            (3, 0) => f.geom_type = r.varint()? as u8,
            (4, 2) => packed_u32(r.len_delim()?, &mut f.geometry)?,
            (4, 0) => f.geometry.push(r.varint()? as u32),
            _ => r.skip(wire)?,
        }
    }
    Some(f)
}

fn decode_layer(bytes: &[u8]) -> Result<Layer, String> {
    let mut r = Rd::new(bytes);
    let mut layer = Layer { extent: 4096, ..Default::default() };
    while !r.eof() {
        let (field, wire) = r.key().ok_or("bad layer tag")?;
        match (field, wire) {
            (1, 2) => {
                layer.name = String::from_utf8_lossy(r.len_delim().ok_or("bad layer name")?).into_owned();
            }
            (2, 2) => {
                let b = r.len_delim().ok_or("bad feature")?;
                layer.features.push(decode_feature(b).ok_or("bad feature data")?);
            }
            (3, 2) => {
                let b = r.len_delim().ok_or("bad key")?;
                layer.keys.push(String::from_utf8_lossy(b).into_owned());
            }
            (4, 2) => {
                let b = r.len_delim().ok_or("bad value")?;
                layer.values.push(decode_value(b).unwrap_or(Value::Bool(false)));
            }
            (5, 0) => layer.extent = r.varint().ok_or("bad extent")? as u32,
            _ => {
                r.skip(wire).ok_or("bad layer field")?;
            }
        }
    }
    Ok(layer)
}

pub fn decode_tile(data: &[u8]) -> Result<Vec<Layer>, String> {
    let mut r = Rd::new(data);
    let mut layers = Vec::new();
    while !r.eof() {
        let (field, wire) = r.key().ok_or("bad tile tag")?;
        if field == 3 && wire == 2 {
            let b = r.len_delim().ok_or("truncated layer")?;
            layers.push(decode_layer(b)?);
        } else {
            r.skip(wire).ok_or("bad tile field")?;
        }
    }
    Ok(layers)
}

/// Tiny MVT encoder used by the unit tests in this crate.
#[cfg(test)]
pub mod test_enc {
    pub fn varint(mut v: u64, out: &mut Vec<u8>) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                break;
            }
            out.push(b | 0x80);
        }
    }

    pub fn field_bytes(num: u64, data: &[u8], out: &mut Vec<u8>) {
        varint((num << 3) | 2, out);
        varint(data.len() as u64, out);
        out.extend_from_slice(data);
    }

    pub fn field_varint(num: u64, v: u64, out: &mut Vec<u8>) {
        varint(num << 3, out);
        varint(v, out);
    }

    pub fn zz(v: i32) -> u32 {
        ((v << 1) ^ (v >> 31)) as u32
    }

    fn packed(vals: &[u32]) -> Vec<u8> {
        let mut b = Vec::new();
        for v in vals {
            varint(*v as u64, &mut b);
        }
        b
    }

    pub enum Geom {
        Point(i32, i32),
        Line(Vec<(i32, i32)>),
        Polygon(Vec<Vec<(i32, i32)>>),
    }

    pub fn geometry(g: &Geom) -> (u8, Vec<u32>) {
        let mut cmds = Vec::new();
        let mut cursor = (0i32, 0i32);
        match g {
            Geom::Point(x, y) => {
                cmds.push(1 | (1 << 3));
                cmds.push(zz(*x));
                cmds.push(zz(*y));
                (1, cmds)
            }
            Geom::Line(pts) => {
                path(&mut cmds, &mut cursor, pts, false);
                (2, cmds)
            }
            Geom::Polygon(rings) => {
                for r in rings {
                    path(&mut cmds, &mut cursor, r, true);
                }
                (3, cmds)
            }
        }
    }

    /// Appends MoveTo/LineTo(/ClosePath) commands. `cursor` is the running
    /// position, shared across paths of one geometry as the format requires.
    fn path(cmds: &mut Vec<u32>, cursor: &mut (i32, i32), pts: &[(i32, i32)], close: bool) {
        cmds.push(1 | (1 << 3));
        cmds.push(zz(pts[0].0 - cursor.0));
        cmds.push(zz(pts[0].1 - cursor.1));
        *cursor = pts[0];
        if pts.len() > 1 {
            cmds.push(2 | (((pts.len() - 1) as u32) << 3));
            for p in &pts[1..] {
                cmds.push(zz(p.0 - cursor.0));
                cmds.push(zz(p.1 - cursor.1));
                *cursor = *p;
            }
        }
        if close {
            cmds.push(7 | (1 << 3));
        }
    }

    pub enum Val {
        S(&'static str),
        N(f64),
    }

    pub struct Feat {
        pub geom: Geom,
        /// (key, value) pairs
        pub props: Vec<(&'static str, Val)>,
    }

    /// Builds a tile containing a single layer.
    pub fn tile(layer_name: &str, feats: Vec<Feat>) -> Vec<u8> {
        let mut keys: Vec<&'static str> = Vec::new();
        let mut values: Vec<Vec<u8>> = Vec::new();
        let mut feature_bytes: Vec<Vec<u8>> = Vec::new();
        for f in &feats {
            let mut tags = Vec::new();
            for (k, v) in &f.props {
                let ki = match keys.iter().position(|x| x == k) {
                    Some(i) => i,
                    None => {
                        keys.push(k);
                        keys.len() - 1
                    }
                };
                let mut vb = Vec::new();
                match v {
                    Val::S(s) => field_bytes(1, s.as_bytes(), &mut vb),
                    Val::N(n) => {
                        varint((3 << 3) | 1, &mut vb);
                        vb.extend_from_slice(&n.to_le_bytes());
                    }
                }
                values.push(vb);
                tags.push(ki as u32);
                tags.push((values.len() - 1) as u32);
            }
            let (gt, geom) = geometry(&f.geom);
            let mut fb = Vec::new();
            field_bytes(2, &packed(&tags), &mut fb);
            field_varint(3, gt as u64, &mut fb);
            field_bytes(4, &packed(&geom), &mut fb);
            feature_bytes.push(fb);
        }
        let mut lb = Vec::new();
        field_bytes(1, layer_name.as_bytes(), &mut lb);
        for fb in &feature_bytes {
            field_bytes(2, fb, &mut lb);
        }
        for k in &keys {
            field_bytes(3, k.as_bytes(), &mut lb);
        }
        for v in &values {
            field_bytes(4, v, &mut lb);
        }
        field_varint(5, 4096, &mut lb);
        let mut out = Vec::new();
        field_bytes(3, &lb, &mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::test_enc::*;
    use super::*;

    #[test]
    fn decodes_polygon_with_props() {
        let data = tile(
            "water",
            vec![Feat {
                geom: Geom::Polygon(vec![vec![(10, 10), (110, 10), (110, 110)]]),
                props: vec![("class", Val::S("lake")), ("rank", Val::N(3.0))],
            }],
        );
        let layers = decode_tile(&data).unwrap();
        assert_eq!(layers.len(), 1);
        let l = &layers[0];
        assert_eq!(l.name, "water");
        assert_eq!(l.extent, 4096);
        let f = &l.features[0];
        assert_eq!(f.geom_type, 3);
        assert_eq!(l.str_prop(f, "class"), Some("lake"));
        assert_eq!(l.num_prop(f, "rank"), Some(3.0));
        assert_eq!(l.str_prop(f, "missing"), None);
        assert_eq!(f.paths(), vec![vec![(10, 10), (110, 10), (110, 110)]]);
    }

    #[test]
    fn decodes_polygon_with_hole_and_line_and_point() {
        let data = tile(
            "x",
            vec![
                Feat {
                    geom: Geom::Polygon(vec![
                        vec![(0, 0), (100, 0), (100, 100), (0, 100)],
                        vec![(20, 20), (20, 80), (80, 80), (80, 20)],
                    ]),
                    props: vec![],
                },
                Feat { geom: Geom::Line(vec![(5, 5), (50, 5), (50, 50)]), props: vec![] },
                Feat { geom: Geom::Point(7, 9), props: vec![] },
            ],
        );
        let layers = decode_tile(&data).unwrap();
        let l = &layers[0];
        assert_eq!(l.features[0].paths().len(), 2);
        assert_eq!(l.features[0].paths()[1][0], (20, 20));
        assert_eq!(l.features[1].paths(), vec![vec![(5, 5), (50, 5), (50, 50)]]);
        assert_eq!(l.features[2].paths(), vec![vec![(7, 9)]]);
    }

    #[test]
    fn garbage_does_not_panic() {
        let _ = decode_tile(&[0xff, 0xff, 0xff]);
        let _ = decode_tile(&[0x1a, 0x05, 0x01]);
        assert!(decode_tile(&[]).unwrap().is_empty());
    }
}
