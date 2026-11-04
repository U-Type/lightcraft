//! Versioned alignment sets reference independent catalog photos and their geometry snapshots.
//! No original, catalog schema, or Develop setting is modified by alignment.
pub mod coordinates;
pub mod jobs;
use crate::Session;
use lightcraft_catalog::{PhotoId, Source};
pub use lightcraft_merge::registration::{Model, Pair, Transform};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
pub type Result<T> = std::result::Result<T, String>;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Member {
    pub photo: PhotoId,
    pub source: Source,
    pub content_hash: Option<String>,
    pub name: String,
    pub geometry: Value,
    pub aspect: f64,
    pub included: bool,
    pub model: Model,
    pub smoothing: f64,
    pub points: Vec<Pair>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Project {
    pub version: u32,
    pub name: String,
    pub members: Vec<Member>,
    pub crop: [f64; 4],
    pub width: usize,
    pub height: usize,
}
#[derive(Default)]
pub struct State {
    pub project: Option<Project>,
    pub path: Option<String>,
    pub dirty: bool,
    pub revision: u64,
    pub undo: Vec<Project>,
    pub redo: Vec<Project>,
    pub backup_error: Option<String>,
}
impl State {
    pub fn replace(&mut self, p: Project) {
        if self.project.as_ref() == Some(&p) {
            return;
        }
        if let Some(old) = self.project.replace(p) {
            self.undo.push(old);
            if self.undo.len() > 100 {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.dirty = true;
        self.revision = self.revision.wrapping_add(1);
    }
    pub fn travel(&mut self, redo: bool) -> Result<()> {
        let (from, to) = if redo { (&mut self.redo, &mut self.undo) } else { (&mut self.undo, &mut self.redo) };
        let p = from.pop().ok_or("no alignment history")?;
        if let Some(old) = self.project.replace(p) {
            to.push(old);
        }
        self.dirty = true;
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }
}
pub fn geometry(p: &lightcraft_catalog::Photo) -> Value {
    let s = &p.develop;
    json!({"width":p.width,"height":p.height,"crop":s.crop,"geometry":s.geometry,"orientation":s.orientation,"optics":s.optics,"embeddedLens":p.embedded_lens,"disabledGeometry":s.disabled_sections.iter().filter(|v|matches!(v.as_str(),"crop"|"geometry"|"optics")).collect::<Vec<_>>()})
}
impl Project {
    /// Suggested name is Unicode-safe and shared by the UI and command API.
    pub fn default_path(&self) -> Option<String> {
        let first = self.members.first()?;
        let Source::File { path } = &first.source else { return None };
        let names: Vec<_> =
            self.members.iter().map(|m| std::path::Path::new(&m.name).file_stem().and_then(|s| s.to_str()).unwrap_or("photo").to_string()).collect();
        let joined = names.join("_");
        let name = if joined.chars().count() <= 15 {
            joined
        } else {
            format!("{}+{}", names.first()?.chars().take(15).collect::<String>(), self.members.len().saturating_sub(1))
        };
        Some(std::path::Path::new(path).parent()?.join(format!("{name}.lcalign.json")).to_string_lossy().into_owned())
    }
    pub fn create(s: &Session, ids: &[PhotoId], name: String) -> Result<Self> {
        if !(2..=32).contains(&ids.len()) {
            return Err("select 2–32 imported photos".into());
        }
        let mut members = Vec::new();
        for id in ids {
            if members.iter().any(|m: &Member| m.photo == *id) {
                return Err("duplicate photo".into());
            }
            let p = s.catalog.photo(*id).ok_or("photo is not in this library")?;
            if p.deleted || p.local {
                return Err("import all photos before aligning".into());
            }
            let (w, h) = lightcraft_pipeline::native_output_size(p.width.max(1) as usize, p.height.max(1) as usize, &p.develop);
            members.push(Member {
                photo: *id,
                source: p.source.clone(),
                content_hash: p.content_hash.clone(),
                name: p.file_name.clone(),
                geometry: geometry(p),
                aspect: w / h,
                included: true,
                model: Model::Similarity,
                smoothing: 0.001,
                points: vec![],
            });
        }
        let a = members.first().ok_or("missing reference")?.aspect;
        let p = Self { version: 1, name, members, crop: [0., 0., 1., 1.], width: 2048, height: (2048. / a).round().max(1.) as usize };
        p.validate()?;
        Ok(p)
    }
    pub fn validate(&self) -> Result<()> {
        if self.version != 1 {
            return Err("unsupported alignment project version".into());
        }
        if !(2..=32).contains(&self.members.len()) || self.name.len() > 512 {
            return Err("invalid set size/name".into());
        }
        if self.width == 0 || self.height == 0 || self.width.checked_mul(self.height).is_none_or(|n| n > 100_000_000) {
            return Err("output must contain 1–100 million pixels".into());
        }
        let c = self.crop;
        if c.iter().any(|x| !x.is_finite())
            || c[0] < 0.
            || c[1] < 0.
            || c[2] <= 0.
            || c[3] <= 0.
            || c[0] + c[2] > 1.00000001
            || c[1] + c[3] > 1.00000001
        {
            return Err("crop must lie inside the reference".into());
        }
        let mut ids = std::collections::HashSet::new();
        for m in &self.members {
            if !ids.insert(m.photo)
                || !m.aspect.is_finite()
                || !(0.01..=100.).contains(&m.aspect)
                || !m.smoothing.is_finite()
                || !(0.0..=1.0).contains(&m.smoothing)
                || m.points.len() > 128
            {
                return Err("invalid alignment member".into());
            }
            for p in &m.points {
                p.validate()?;
            }
        }
        Ok(())
    }
    pub fn check_sources(&self, s: &Session) -> Result<()> {
        self.validate()?;
        for m in &self.members {
            let p = s.catalog.photo(m.photo).ok_or_else(|| format!("{} is missing from this library", m.name))?;
            if p.deleted || p.source != m.source || p.content_hash != m.content_hash {
                return Err(format!("{} no longer matches the saved source; open the correct library or create another set", m.name));
            }
            if geometry(p) != m.geometry {
                return Err(format!("{} geometry changed. Remap landmarks before export, or reset them.", m.name));
            }
        }
        Ok(())
    }
    pub fn transform(&self, index: usize) -> Result<Transform> {
        let r = self.members.first().ok_or("missing reference")?;
        let m = self.members.get(index).ok_or("no such alignment image")?;
        if index == 0 {
            return Ok(Transform::identity(r.aspect));
        }
        lightcraft_merge::registration::fit(m.model, &m.points, r.aspect, m.aspect, m.smoothing)
    }
    pub fn save(&self, s: &Session, path: &str) -> Result<()> {
        self.validate()?;
        if !path.ends_with(".lcalign.json") {
            return Err("project filenames must end in .lcalign.json".into());
        }
        s.check_write_target(path)?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        crate::export::write_file_durable(path, &bytes)
    }
    pub fn load(path: &str) -> Result<Self> {
        use std::io::Read;
        let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        f.take(4_000_001).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        if bytes.len() > 4_000_000 {
            return Err("sidecar exceeds 4 MB".into());
        }
        let p: Self = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        p.validate()?;
        Ok(p)
    }
}
pub fn warp_preview(image: &lightcraft_raster::Rgba8, t: &Transform, w: usize, h: usize) -> Result<lightcraft_raster::Rgba8> {
    lightcraft_merge::registration::warp(image, t, [0., 0., 1., 1.], w, h)
}

/// A working copy never changes the explicit saved-project checkpoint.
pub fn backup_path(path: &str) -> String {
    format!("{}.working.lcalign.json", path.strip_suffix(".lcalign.json").unwrap_or(path))
}
pub fn backup(s: &mut Session) {
    let result = match (&s.alignment.project, &s.alignment.path) {
        (Some(p), Some(path)) => p.save(s, &backup_path(path)),
        (Some(_), None) => Err("Choose Save As to enable working backups for this set.".into()),
        _ => Ok(()),
    };
    s.alignment.backup_error = result.err();
}
pub fn remove_backup(s: &Session) -> Result<()> {
    if let Some(path) = &s.alignment.path {
        let path = backup_path(path);
        s.check_write_target(&path)?;
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}
