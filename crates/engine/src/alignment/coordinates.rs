//! Geometry snapshots give landmarks a stable mapping through the EXIF-oriented source.
use super::{Result, geometry};
use lightcraft_geom::Point;
use lightcraft_pipeline::geometry::Frame;
use serde_json::Value;
fn frame(v: &Value) -> Result<Frame> {
    let size = |k| {
        v.get(k)
            .and_then(Value::as_u64)
            .filter(|n| *n > 0 && *n <= 200_000)
            .map(|n| n as usize)
            .ok_or_else(|| "invalid saved source dimensions".to_string())
    };
    let s:lightcraft_develop::DevelopSettings=serde_json::from_value(serde_json::json!({"crop":v["crop"],"geometry":v["geometry"],"orientation":v["orientation"],"optics":v["optics"],"disabled_sections":v["disabledGeometry"]})).map_err(|e|e.to_string())?;
    let lens: Option<lightcraft_develop::EmbeddedLens> = serde_json::from_value(v["embeddedLens"].clone()).map_err(|e| e.to_string())?;
    Ok(Frame::with_lens(size("width")?, size("height")?, &s, true, lens.as_ref()))
}
fn to_source(f: &Frame, p: [f64; 2]) -> Result<[f64; 2]> {
    let q = f.out_to_oriented(1, 1).apply(Point::new(p[0], p[1]));
    let q = if let Some(w) = &f.warp { w.to_source(q, 1) } else { q };
    let (x, y) = f.orient.inverse().map(q.x, q.y, f.ow, f.oh);
    let q = [x / f.src_w as f64, y / f.src_h as f64];
    if q.iter().any(|v| !v.is_finite()) {
        return Err("geometry has no finite mapping".into());
    }
    Ok(q)
}
fn from_source(f: &Frame, source: [f64; 2]) -> Result<[f64; 2]> {
    let mut p = [0.5, 0.5];
    for _ in 0..40 {
        let q = to_source(f, p)?;
        let e = [source[0] - q[0], source[1] - q[1]];
        if e[0].hypot(e[1]) < 1e-9 {
            if p.iter().any(|v| *v < -1e-7 || *v > 1.0000001) {
                return Err("a landmark lies outside the new crop; widen the crop or reset its landmarks".into());
            }
            return Ok(p.map(|v| v.clamp(0., 1.)));
        }
        let x = to_source(f, [p[0] + 1e-5, p[1]])?;
        let y = to_source(f, [p[0], p[1] + 1e-5])?;
        let j = [(x[0] - q[0]) / 1e-5, (y[0] - q[0]) / 1e-5, (x[1] - q[1]) / 1e-5, (y[1] - q[1]) / 1e-5];
        let d = j[0] * j[3] - j[1] * j[2];
        if d.abs() < 1e-12 {
            return Err("new geometry is singular".into());
        }
        let step = [(j[3] * e[0] - j[1] * e[1]) / d, (-j[2] * e[0] + j[0] * e[1]) / d];
        let scale = (step[0].abs().max(step[1].abs()) / 0.5).max(1.);
        p[0] += step[0] / scale;
        p[1] += step[1] / scale;
    }
    Err("could not map a landmark through changed geometry".into())
}
pub fn remap(project: &super::Project, s: &crate::Session) -> Result<super::Project> {
    project.validate()?;
    let mut next = project.clone();
    for (index, old) in project.members.iter().enumerate() {
        let photo = s.catalog.photo(old.photo).ok_or("missing source")?;
        if photo.source != old.source || photo.content_hash != old.content_hash {
            return Err("source changed; create a new set".into());
        }
        let new = geometry(photo);
        if old.geometry == new {
            continue;
        }
        if old.geometry["width"] != new["width"] || old.geometry["height"] != new["height"] {
            return Err("source dimensions changed; create a new set".into());
        }
        let (a, b) = (frame(&old.geometry)?, frame(&new)?);
        let map = |p| from_source(&b, to_source(&a, p)?);
        if index == 0 {
            for m in &mut next.members {
                for pair in &mut m.points {
                    pair.reference = map(pair.reference)?;
                }
            }
            next.crop = [0., 0., 1., 1.];
        } else if let Some(m) = next.members.get_mut(index) {
            for pair in &mut m.points {
                pair.target = map(pair.target)?;
            }
        }
        if let Some(m) = next.members.get_mut(index) {
            m.geometry = new;
            m.aspect = b.aspect();
        }
    }
    if let Some(m) = next.members.first() {
        next.height = (next.width as f64 * next.crop[3] / (m.aspect * next.crop[2])).round().max(1.) as usize;
    }
    next.validate()?;
    Ok(next)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn coordinate_roundtrips() {
        for rotation in [lightcraft_geom::Orientation::Normal, lightcraft_geom::Orientation::Rotate90, lightcraft_geom::Orientation::FlipH] {
            let mut s = lightcraft_develop::DevelopSettings::default();
            s.orientation = rotation;
            s.crop.geometry.rect = lightcraft_geom::Rect::new(0.1, 0.15, 0.9, 0.85);
            s.crop.geometry.angle = 4.;
            let f = Frame::new(1200, 800, &s, true);
            for p in [[0.2, 0.3], [0.7, 0.6]] {
                let q = from_source(&f, to_source(&f, p).unwrap()).unwrap();
                assert!((q[0] - p[0]).hypot(q[1] - p[1]) < 1e-7);
            }
        }
    }
}
