//! Jobs detached from the session. UI runs them on workers; CLI/MCP use the same implementation.
use super::{Pair, Project, Result, Session};
use crate::export::{ExportFormat, ExportOptions, OutputDepth, OutputSpace};
use crate::media::RenderJob;
use lightcraft_merge::{features, ransac};
use lightcraft_raster::Rgba8;
pub struct PreviewJob {
    pub jobs: Vec<RenderJob>,
}
impl PreviewJob {
    pub fn run(self) -> Result<Vec<Rgba8>> {
        self.jobs.into_iter().map(|j| j.run().rendered.map(|r| r.image)).collect()
    }
}
pub fn previews(s: &mut Session, p: &Project) -> Result<PreviewJob> {
    p.check_sources(s)?;
    let jobs = p.members.iter().map(|m| s.export_job(m.photo, 1200, 1200, OutputSpace::Srgb, OutputDepth::U8)).collect::<Result<_>>()?;
    Ok(PreviewJob { jobs })
}
pub fn suggest(reference: &Rgba8, target: &Rgba8) -> Result<Vec<Pair>> {
    use lightcraft_geom::Point;
    let grey = |im: &Rgba8| im.map(|p| (0.25 * f32::from(p[0]) + 0.6 * f32::from(p[1]) + 0.15 * f32::from(p[2])) / 255.);
    let a = features::detect(&grey(reference), 600);
    let b = features::detect(&grey(target), 600);
    let matches = features::match_features(&a, &b, 0.75);
    let mut pa = Vec::new();
    let mut pb = Vec::new();
    for (i, j) in matches {
        if let (Some(a), Some(b)) = (a.points.get(i), b.points.get(j)) {
            pa.push(Point::new(f64::from(a.x), f64::from(a.y)));
            pb.push(Point::new(f64::from(b.x), f64::from(b.y)));
        }
    }
    let f = ransac::ransac(ransac::Model::Similarity, &pa, &pb, 3., 8, 0x1ca11).ok_or("no reliable automatic match; place manual pairs")?;
    if f.inliers.len() * 4 < pa.len() {
        return Err("automatic matches are ambiguous; use manual points".into());
    }
    Ok(f.inliers
        .iter()
        .take(64)
        .filter_map(|i| {
            Some(Pair::new(
                [pa.get(*i)?.x / reference.width as f64, pa.get(*i)?.y / reference.height as f64],
                [pb.get(*i)?.x / target.width as f64, pb.get(*i)?.y / target.height as f64],
            ))
        })
        .collect())
}
pub struct ExportJob {
    project: Project,
    jobs: Vec<(usize, RenderJob, String)>,
    format: ExportFormat,
}
impl ExportJob {
    pub fn run(self, progress: &dyn Fn(usize, usize) -> bool) -> Result<Vec<String>> {
        let mut written = Vec::new();
        let n = self.jobs.len();
        for (index, job, path) in self.jobs {
            if !progress(written.len(), n) {
                return Err(format!("cancelled; {} files already exported", written.len()));
            }
            let result = (|| {
                let image = job.run().rendered?.image;
                let t = self.project.transform(index)?;
                let image = lightcraft_merge::registration::warp(&image, &t, self.project.crop, self.project.width, self.project.height)?;
                if self.format == ExportFormat::Jpeg && image.data.iter().any(|p| p[3] != 255) {
                    return Err("JPEG crop includes uncovered pixels; use Common crop or PNG".into());
                }
                let opts = ExportOptions { format: self.format, quality: 95, ..Default::default() };
                let bytes = crate::export::encode_image(&image, &opts)?;
                crate::export::write_file_new(&path, &bytes)
            })();
            if let Err(e) = result {
                return Err(format!("{e}; {} files already exported (never overwritten)", written.len()));
            }
            written.push(path);
        }
        progress(n, n);
        Ok(written)
    }
}
pub fn export(s: &mut Session, p: &Project, dir: &str, jpeg: bool) -> Result<ExportJob> {
    p.check_sources(s)?;
    if dir.trim().is_empty() {
        return Err("choose an output folder".into());
    }
    let format = if jpeg { ExportFormat::Jpeg } else { ExportFormat::Png };
    let mut jobs = Vec::new();
    for (i, m) in p.members.iter().enumerate().filter(|(_, m)| m.included) {
        p.transform(i)?;
        let photo = s.catalog.photo(m.photo).ok_or("missing photo")?;
        let (w, h) = lightcraft_pipeline::native_output_size(photo.width.max(1) as usize, photo.height.max(1) as usize, &photo.develop);
        if w * h > 100_000_000. {
            return Err("full-resolution alignment render exceeds 100 MP".into());
        }
        let stem = std::path::Path::new(&m.name).file_stem().and_then(|v| v.to_str()).unwrap_or("photo");
        let name = format!("{:02}-{}-aligned.{}", i + 1, stem, format.extension());
        let path = std::path::Path::new(dir).join(name).to_string_lossy().to_string();
        s.check_write_target(&path)?;
        if std::path::Path::new(&path).exists() {
            return Err(format!("{path} already exists; choose another output folder"));
        }
        jobs.push((i, s.export_job(m.photo, w.round().max(1.) as usize, h.round().max(1.) as usize, OutputSpace::Srgb, OutputDepth::U8)?, path));
    }
    if jobs.is_empty() {
        return Err("include at least one image".into());
    }
    Ok(ExportJob { project: p.clone(), jobs, format })
}
