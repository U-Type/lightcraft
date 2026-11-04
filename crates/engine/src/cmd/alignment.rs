//! Alignment commands shared by UI, CLI and MCP.
use super::{CommandSpec, always, bad, cmd, str_param};
use crate::alignment::{Project, State, geometry, jobs};
use crate::{Result, Session};
use serde_json::{Value, json};
fn current(s: &Session) -> Result<Project> {
    s.alignment.project.clone().ok_or_else(|| bad("alignment", "create or open an alignment set first"))
}
fn text<'a>(p: &'a Value, key: &str) -> Result<&'a str> {
    str_param(p, key).ok_or_else(|| bad("alignment", format!("missing {key}")))
}
fn replace(s: &mut Session, p: Project) -> Result<Value> {
    p.validate().map_err(|e| bad("alignment", e))?;
    s.alignment.replace(p);
    Ok(json!({"revision":s.alignment.revision}))
}
fn run(s: &mut Session, id: &str, p: &Value) -> Result<Value> {
    let err = |e: String| bad(id, e);
    match id {
        "alignment.create" => {
            if s.alignment.dirty {
                return Err(bad(id, "save or discard the current set first"));
            }
            let mut ids = s.targets(p)?;
            if let Some(r) = p.get("reference").and_then(Value::as_u64) {
                let i = ids.iter().position(|v| v.0 == r).ok_or_else(|| bad(id, "reference must be selected"))?;
                ids.swap(0, i);
            }
            let project = Project::create(s, &ids, str_param(p, "name").unwrap_or("Alignment set").into()).map_err(err)?;
            let revision = s.alignment.revision;
            s.alignment = State { revision, ..Default::default() };
            replace(s, project)
        }
        "alignment.get" => {
            Ok(json!({"project":s.alignment.project,"path":s.alignment.path,"dirty":s.alignment.dirty,"revision":s.alignment.revision}))
        }
        "alignment.replace" => {
            let old = current(s)?;
            let next: Project =
                serde_json::from_value(p.get("project").cloned().ok_or_else(|| bad(id, "missing project"))?).map_err(|e| bad(id, e.to_string()))?;
            if old.members.len() != next.members.len()
                || old.members.iter().zip(&next.members).any(|(a, b)| {
                    a.photo != b.photo || a.source != b.source || a.content_hash != b.content_hash || a.geometry != b.geometry || a.aspect != b.aspect
                })
            {
                return Err(bad(id, "source identity/geometry cannot be replaced; use remapGeometry or create another set"));
            }
            replace(s, next)
        }
        "alignment.save" => {
            let path = str_param(p, "path")
                .map(str::to_owned)
                .or_else(|| s.alignment.path.clone())
                .ok_or_else(|| bad(id, "provide a .lcalign.json path"))?;
            current(s)?.save(s, &path).map_err(err)?;
            s.alignment.path = Some(path.clone());
            s.alignment.dirty = false;
            Ok(json!({"path":path}))
        }
        "alignment.open" => {
            if s.alignment.dirty {
                return Err(bad(id, "save or discard the current set first"));
            }
            let path = text(p, "path")?;
            let project = Project::load(path).map_err(err)?;
            for m in &project.members {
                let photo = s.catalog.photo(m.photo).ok_or_else(|| bad(id, "open the library containing this set"))?;
                if photo.source != m.source || photo.content_hash != m.content_hash {
                    return Err(bad(id, "saved photos do not match this library"));
                }
            }
            let revision = s.alignment.revision.wrapping_add(1);
            s.alignment = State { project: Some(project), path: Some(path.into()), revision, ..Default::default() };
            Ok(json!({"path":path}))
        }
        "alignment.discard" => {
            let revision = s.alignment.revision.wrapping_add(1);
            s.alignment = State { revision, ..Default::default() };
            Ok(json!({}))
        }
        "alignment.undo" | "alignment.redo" => {
            s.alignment.travel(id.ends_with("redo")).map_err(err)?;
            Ok(json!({}))
        }
        "alignment.remapGeometry" => {
            let next = crate::alignment::coordinates::remap(&current(s)?, s).map_err(err)?;
            replace(s, next)
        }
        "alignment.resetGeometry" => {
            let mut project = current(s)?;
            let mut ref_changed = false;
            for (i, m) in project.members.iter_mut().enumerate() {
                let photo = s.catalog.photo(m.photo).ok_or_else(|| bad(id, "missing photo"))?;
                if photo.source != m.source || photo.content_hash != m.content_hash {
                    return Err(bad(id, "source changed; create a new set"));
                }
                if geometry(photo) != m.geometry {
                    m.geometry = geometry(photo);
                    let (w, h) = lightcraft_pipeline::native_output_size(photo.width.max(1) as usize, photo.height.max(1) as usize, &photo.develop);
                    m.aspect = w / h;
                    m.points.clear();
                    ref_changed |= i == 0;
                }
            }
            if ref_changed {
                for m in &mut project.members {
                    m.points.clear();
                }
                project.crop = [0., 0., 1., 1.];
            }
            replace(s, project)
        }
        "alignment.fit" => {
            let project = current(s)?;
            project.check_sources(s).map_err(err)?;
            let t = project.transform(index(p)?).map_err(err)?;
            Ok(json!({"rms":t.rms,"residuals":t.residuals}))
        }
        "alignment.commonCrop" => {
            let mut project = current(s)?;
            project.check_sources(s).map_err(err)?;
            let ts = project
                .members
                .iter()
                .enumerate()
                .filter(|(_, m)| m.included)
                .map(|(i, _)| project.transform(i))
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(err)?;
            project.crop = lightcraft_merge::registration::common_crop(&ts).map_err(err)?;
            let a = project.members.first().ok_or_else(|| bad(id, "missing reference"))?.aspect;
            project.height = (project.width as f64 * project.crop[3] / (a * project.crop[2])).round().max(1.) as usize;
            replace(s, project)
        }
        "alignment.auto" => {
            let mut project = current(s)?;
            let i = index(p)?;
            if i == 0 || project.members.get(i).is_none() {
                return Err(bad(id, "choose a target"));
            }
            if project.members.get(i).is_some_and(|m| !m.points.is_empty()) {
                return Err(bad(id, "automatic alignment requires an empty target; existing points are preserved"));
            }
            let images = jobs::previews(s, &project).map_err(err)?.run().map_err(err)?;
            let points =
                jobs::suggest(images.first().ok_or_else(|| bad(id, "missing reference"))?, images.get(i).ok_or_else(|| bad(id, "missing target"))?)
                    .map_err(err)?;
            if let Some(m) = project.members.get_mut(i) {
                m.points = points;
            }
            project.transform(i).map_err(err)?;
            replace(s, project)
        }
        "alignment.export" => {
            let project = current(s)?;
            let format = str_param(p, "format").unwrap_or("png");
            if !matches!(format, "png" | "jpeg") {
                return Err(bad(id, "export supports png or jpeg"));
            }
            let job = jobs::export(s, &project, text(p, "directory")?, format == "jpeg").map_err(err)?;
            Ok(json!({"files":job.run(&|_,_|true).map_err(err)?}))
        }
        _ => Err(bad(id, "unknown alignment command")),
    }
}
fn index(p: &Value) -> Result<usize> {
    match p.get("index") {
        None => Ok(1),
        Some(v) => v.as_u64().and_then(|v| usize::try_from(v).ok()).ok_or_else(|| bad("alignment", "index must be a nonnegative integer")),
    }
}
pub fn specs() -> Vec<CommandSpec> {
    let mut out = Vec::new();
    macro_rules! add {
        ($id:literal,$label:literal,$params:literal) => {
            out.push(cmd!($id, $label, [], None, $params, always, |s, p| run(s, $id, p)));
        };
    }
    add!("alignment.create", "Create Alignment Set", "{ids?, reference?, name?}");
    add!("alignment.get", "Inspect Alignment Set", "{}");
    add!("alignment.replace", "Edit Alignment Set", "{project}");
    add!("alignment.save", "Save Alignment Set", "{path?: *.lcalign.json}");
    add!("alignment.open", "Open Alignment Set", "{path}");
    add!("alignment.discard", "Discard Alignment Set", "{}");
    add!("alignment.undo", "Undo Alignment Edit", "{}");
    add!("alignment.redo", "Redo Alignment Edit", "{}");
    add!("alignment.remapGeometry", "Remap Changed Geometry", "{}: preserve points through source coordinates; reset reference crop if changed");
    add!("alignment.resetGeometry", "Reset Changed Geometry", "{}: clear affected points");
    add!("alignment.fit", "Fit Landmarks", "{index=1}");
    add!("alignment.commonCrop", "Find Common Crop", "{}");
    add!("alignment.auto", "Suggest Alignment Points", "{index=1}: empty target only");
    add!("alignment.export", "Export Aligned Photos", "{directory, format=png|jpeg}: never overwrite");
    out
}
