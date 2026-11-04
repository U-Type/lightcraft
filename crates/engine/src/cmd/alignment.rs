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
    let revision = s.alignment.revision;
    let result = run_inner(s, id, p);
    if result.is_ok() && s.alignment.dirty && s.alignment.revision != revision {
        crate::alignment::backup(s);
    }
    result
}
fn run_inner(s: &mut Session, id: &str, p: &Value) -> Result<Value> {
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
            let mut path = project.default_path();
            let mut recovered = None;
            if let Some(base) = path.clone() {
                for suffix in 1..=1000 {
                    let candidate =
                        if suffix == 1 { base.clone() } else { format!("{}-{suffix}.lcalign.json", base.trim_end_matches(".lcalign.json")) };
                    let working = crate::alignment::backup_path(&candidate);
                    let existing = if std::path::Path::new(&working).exists() { &working } else { &candidate };
                    if !std::path::Path::new(existing).exists() {
                        path = Some(candidate);
                        break;
                    }
                    let saved = Project::load(existing).map_err(err)?;
                    let same = saved.members.len() == project.members.len()
                        && saved
                            .members
                            .iter()
                            .zip(&project.members)
                            .all(|(a, b)| a.photo == b.photo && a.source == b.source && a.content_hash == b.content_hash);
                    if same {
                        recovered = Some(saved);
                        path = Some(candidate);
                        break;
                    }
                    if suffix == 1000 {
                        return Err(bad(id, "Too many similarly named alignment projects."));
                    }
                }
            }
            s.alignment = State { revision, path, ..Default::default() };
            replace(s, recovered.unwrap_or(project))
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
            crate::alignment::remove_backup(s).map_err(err)?;
            s.alignment.path = Some(path.clone());
            s.alignment.dirty = false;
            s.alignment.backup_error = None;
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
            let recovered = path.ends_with(".working.lcalign.json");
            let project_path = if recovered { format!("{}.lcalign.json", path.trim_end_matches(".working.lcalign.json")) } else { path.to_string() };
            s.alignment = State { project: Some(project), path: Some(project_path), revision, dirty: recovered, ..Default::default() };
            Ok(json!({"path":path}))
        }
        "alignment.discard" => {
            crate::alignment::remove_backup(s).map_err(err)?;
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
        "alignment.add" => {
            let mut project = current(s)?;
            let mut ids: Vec<_> = project.members.iter().map(|m| m.photo).collect();
            for id in s.targets(p)? {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            let all = Project::create(s, &ids, project.name.clone()).map_err(err)?;
            project.members.extend(all.members.into_iter().skip(project.members.len()));
            replace(s, project)
        }
        "alignment.remove" => {
            let mut project = current(s)?;
            let i = index(p)?;
            if project.members.len() <= 2 {
                return Err(bad(id, "Keep at least two photos in the set."));
            }
            let i = if i == 0 {
                let next = if project.members.iter().all(|m| m.points.is_empty()) {
                    1
                } else {
                    (1..project.members.len())
                        .find(|&j| project.transform(j).is_ok())
                        .ok_or_else(|| bad(id, "Align a replacement reference before removing this one."))?
                };
                run_inner(s, "alignment.reference", &json!({"index":next}))?;
                project = current(s)?;
                next
            } else {
                i
            };
            if i >= project.members.len() {
                return Err(bad(id, "No such photo."));
            }
            project.members.remove(i);
            replace(s, project)
        }
        "alignment.reference" => {
            let mut project = current(s)?;
            project.check_sources(s).map_err(err)?;
            let i = index(p)?;
            if i == 0 {
                return Ok(json!({}));
            }
            let new_ref = project.members.get(i).cloned().ok_or_else(|| bad(id, "No such photo."))?;
            if project.members.iter().any(|m| !m.points.is_empty()) {
                let map = project.transform(i).map_err(err)?;
                for (j, m) in project.members.iter_mut().enumerate() {
                    if j != i {
                        for point in &mut m.points {
                            point.reference =
                                map.map(point.reference).ok_or_else(|| bad(id, "Reference change maps a landmark outside the image."))?;
                            point.validate().map_err(err)?;
                        }
                    }
                }
                if let Some(old) = project.members.first_mut() {
                    old.model = new_ref.model;
                    old.smoothing = new_ref.smoothing;
                    old.points =
                        new_ref.points.iter().map(|p| crate::alignment::Pair { reference: p.target, target: p.reference, ..p.clone() }).collect();
                }
            }
            project.members.swap(0, i);
            if let Some(m) = project.members.first_mut() {
                m.points.clear();
            }
            project.crop = [0., 0., 1., 1.];
            project.height = (project.width as f64 / new_ref.aspect).round().max(1.) as usize;
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
    add!("alignment.add", "Add Alignment Photos", "{ids}");
    add!("alignment.remove", "Remove Alignment Photo", "{index}: remap to a fitted replacement when removing the reference");
    add!("alignment.reference", "Change Alignment Reference", "{index}: remap landmarks and reset crop");
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
