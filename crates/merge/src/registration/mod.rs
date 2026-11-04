//! Landmark registration, independent of panorama stitching. Points are normalized;
//! solvers use aspect-correct coordinates in image-height units. Transforms map
//! reference to target for inverse resampling. Manual pairs are never silently rejected.
use serde::{Deserialize, Serialize};
mod solve;
mod warp;
pub use warp::{common_crop, warp};
pub type Point = [f64; 2];
pub type Result<T> = std::result::Result<T, String>;
pub const MAX_POINTS: usize = 128;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Model {
    Translation,
    #[default]
    Similarity,
    Affine,
    Perspective,
    ThinPlate,
}
impl Model {
    pub fn minimum(self) -> usize {
        match self {
            Self::Translation => 1,
            Self::Similarity => 2,
            Self::Affine | Self::ThinPlate => 3,
            Self::Perspective => 4,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pair {
    pub reference: Point,
    pub target: Point,
    pub weight: f64,
    pub enabled: bool,
}
impl Pair {
    pub fn new(reference: Point, target: Point) -> Self {
        Self { reference, target, weight: 1., enabled: true }
    }
    pub fn validate(&self) -> Result<()> {
        if self.reference.iter().chain(&self.target).any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
            || !self.weight.is_finite()
            || !(0.01..=100.).contains(&self.weight)
        {
            return Err("points must be inside the image; weights must be 0.01–100".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug)]
pub struct Transform {
    matrix: [f64; 9],
    radial: Vec<(Point, Point)>,
    reference_aspect: f64,
    target_aspect: f64,
    pub residuals: Vec<Option<f64>>,
    pub rms: f64,
}
impl Transform {
    pub fn identity(aspect: f64) -> Self {
        Self {
            matrix: [1., 0., 0., 0., 1., 0., 0., 0., 1.],
            radial: vec![],
            reference_aspect: aspect,
            target_aspect: aspect,
            residuals: vec![],
            rms: 0.,
        }
    }
    pub fn map(&self, p: Point) -> Option<Point> {
        let [x, y] = [p[0] * self.reference_aspect, p[1]];
        let m = self.matrix;
        let d = m[6] * x + m[7] * y + m[8];
        if !d.is_finite() || d.abs() < 1e-9 {
            return None;
        }
        let mut q = [(m[0] * x + m[1] * y + m[2]) / d, (m[3] * x + m[4] * y + m[5]) / d];
        for (at, w) in &self.radial {
            let u = kernel((x - at[0]).powi(2) + (y - at[1]).powi(2));
            q[0] += w[0] * u;
            q[1] += w[1] * u;
        }
        q[0] /= self.target_aspect;
        q.iter().all(|v| v.is_finite()).then_some(q)
    }
    pub fn valid(&self, p: Point) -> bool {
        self.map(p).is_some_and(|q| q.iter().all(|v| (0.0..=1.0).contains(v)))
    }
}
fn kernel(r2: f64) -> f64 {
    if r2 < 1e-20 { 0. } else { r2 * r2.ln() }
}
pub fn fit(model: Model, pairs: &[Pair], reference_aspect: f64, target_aspect: f64, smoothing: f64) -> Result<Transform> {
    if pairs.len() > MAX_POINTS
        || ![reference_aspect, target_aspect].iter().all(|a| a.is_finite() && (0.01..=100.).contains(a))
        || !smoothing.is_finite()
        || !(0.0..=1.0).contains(&smoothing)
    {
        return Err("invalid aspect, smoothing or too many landmarks (maximum 128)".into());
    }
    for p in pairs {
        p.validate()?;
    }
    let active: Vec<_> = pairs.iter().filter(|p| p.enabled).collect();
    if active.len() < model.minimum() {
        return Err(format!("this model needs at least {} enabled pairs", model.minimum()));
    }
    let a: Vec<Point> = active.iter().map(|p| [p.reference[0] * reference_aspect, p.reference[1]]).collect();
    let b: Vec<Point> = active.iter().map(|p| [p.target[0] * target_aspect, p.target[1]]).collect();
    let mut t = Transform { reference_aspect, target_aspect, ..Transform::identity(reference_aspect) };
    if model == Model::ThinPlate {
        let n = a.len();
        let mut mat = vec![vec![0.; n + 3]; n + 3];
        for i in 0..n {
            for j in 0..n {
                mat[i][j] = kernel((a[i][0] - a[j][0]).powi(2) + (a[i][1] - a[j][1]).powi(2));
            }
            mat[i][i] += smoothing / active[i].weight;
            for (k, v) in [1., a[i][0], a[i][1]].into_iter().enumerate() {
                mat[i][n + k] = v;
                mat[n + k][i] = v;
            }
        }
        let mut bx = vec![0.; n + 3];
        let mut by = bx.clone();
        for i in 0..n {
            bx[i] = b[i][0];
            by[i] = b[i][1];
        }
        let x = solve::linear(mat.clone(), bx)?;
        let y = solve::linear(mat, by)?;
        t.matrix = [x[n + 1], x[n + 2], x[n], y[n + 1], y[n + 2], y[n], 0., 0., 1.];
        t.radial = a.iter().enumerate().map(|(i, p)| (*p, [x[i], y[i]])).collect();
    } else {
        let n = match model {
            Model::Translation => 2,
            Model::Similarity => 4,
            Model::Affine => 6,
            _ => 8,
        };
        let mut rows = Vec::new();
        let mut rhs = Vec::new();
        for (i, ([x, y], [u, v])) in a.iter().zip(&b).enumerate() {
            let (r, s, bu, bv) = match model {
                Model::Translation => (vec![1., 0.], vec![0., 1.], u - x, v - y),
                Model::Similarity => (vec![*x, -y, 1., 0.], vec![*y, *x, 0., 1.], *u, *v),
                Model::Affine => (vec![*x, *y, 1., 0., 0., 0.], vec![0., 0., 0., *x, *y, 1.], *u, *v),
                _ => (vec![*x, *y, 1., 0., 0., 0., -u * x, -u * y], vec![0., 0., 0., *x, *y, 1., -v * x, -v * y], *u, *v),
            };
            let w = active[i].weight.sqrt();
            rows.push(r.into_iter().map(|v| v * w).collect());
            rows.push(s.into_iter().map(|v| v * w).collect());
            rhs.extend([bu * w, bv * w]);
        }
        let c = solve::least_squares(rows, rhs, n)?;
        t.matrix = match model {
            Model::Translation => [1., 0., c[0], 0., 1., c[1], 0., 0., 1.],
            Model::Similarity => [c[0], -c[1], c[2], c[1], c[0], c[3], 0., 0., 1.],
            Model::Affine => [c[0], c[1], c[2], c[3], c[4], c[5], 0., 0., 1.],
            _ => [c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7], 1.],
        };
    }
    // A projective denominator must keep one sign over the whole canvas (linear in x/y).
    let m = t.matrix;
    let ds = [m[8], m[6] * reference_aspect + m[8], m[7] + m[8], m[6] * reference_aspect + m[7] + m[8]];
    if ds.iter().any(|d| d.abs() < 1e-9 || d.signum() != ds[0].signum()) {
        return Err("perspective horizon crosses the image".into());
    }
    // Sampled fold guard for TPS; a safety diagnostic, not a mathematical global guarantee.
    for y in 0..=24 {
        for x in 0..=24 {
            let p = [x as f64 / 24., y as f64 / 24.];
            let q = t.map(p).ok_or("invalid transform")?;
            let qx = t.map([p[0] + 1e-5, p[1]]).ok_or("invalid transform")?;
            let qy = t.map([p[0], p[1] + 1e-5]).ok_or("invalid transform")?;
            let det = ((qx[0] - q[0]) * (qy[1] - q[1]) - (qx[1] - q[1]) * (qy[0] - q[0])) / 1e-10;
            if !(1e-6..=1e6).contains(&det) {
                return Err("transform folds, reflects or collapses; revise points or increase warp smoothing".into());
            }
        }
    }
    let (mut sum, mut weights) = (0., 0.);
    for p in pairs {
        let r = if p.enabled {
            let q = t.map(p.reference).ok_or("invalid projection")?;
            Some(((q[0] - p.target[0]) * target_aspect).hypot(q[1] - p.target[1]))
        } else {
            None
        };
        if let Some(r) = r {
            sum += r * r * p.weight;
            weights += p.weight;
        }
        t.residuals.push(r);
    }
    t.rms = (sum / weights).sqrt();
    Ok(t)
}
#[cfg(test)]
mod tests;
