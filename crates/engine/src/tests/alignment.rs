use crate::{
    Session,
    alignment::{Model, Pair, Project},
};
use serde_json::json;
fn session() -> (Session, Vec<crate::catalog::PhotoId>) {
    let s = Session::with_demo();
    let ids = s.catalog.photos().take(2).map(|p| p.id).collect();
    (s, ids)
}
fn aligned(s: &Session, ids: &[crate::catalog::PhotoId]) -> Project {
    let mut p = Project::create(s, ids, "Comparison".into()).unwrap();
    p.width = 64;
    p.height = 48;
    p.members[1].model = Model::Affine;
    p.members[1].points = vec![Pair::new([0.1, 0.1], [0.1, 0.1]), Pair::new([0.9, 0.1], [0.9, 0.1]), Pair::new([0.1, 0.9], [0.1, 0.9])];
    p
}
#[test]
fn commands_history_roundtrip() {
    let (mut s, ids) = session();
    s.execute("alignment.create", &json!({"ids":ids.iter().map(|p|p.0).collect::<Vec<_>>()})).unwrap();
    let p = aligned(&s, &ids);
    s.execute("alignment.replace", &json!({"project":p})).unwrap();
    assert!(s.execute("alignment.fit", &json!({"index":1})).unwrap()["rms"].as_f64().unwrap() < 1e-8);
    s.execute("alignment.undo", &json!({})).unwrap();
    assert!(s.alignment.project.as_ref().unwrap().members[1].points.is_empty());
    s.execute("alignment.redo", &json!({})).unwrap();
    assert_eq!(s.alignment.project.as_ref().unwrap(), &p);
    let dir = std::env::temp_dir().join(format!("lc-alignment-project-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("set.lcalign.json");
    s.execute("alignment.save", &json!({"path":path})).unwrap();
    assert!(!s.alignment.dirty);
    assert_eq!(Project::load(path.to_str().unwrap()).unwrap(), p);
    s.execute("alignment.discard", &json!({})).unwrap();
    s.execute("alignment.open", &json!({"path":path})).unwrap();
    assert_eq!(s.alignment.project.as_ref().unwrap(), &p);
    let mut invalid = p.clone();
    invalid.version = 999;
    assert!(s.execute("alignment.replace", &json!({"project":invalid})).is_err());
    assert_eq!(s.alignment.project.as_ref().unwrap(), &p);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn independent_edits_and_geometry_remap() {
    let (mut s, ids) = session();
    let p = aligned(&s, &ids);
    s.execute("develop.set", &json!({"id":ids[1].0,"control":"light.exposure","value":1.0})).unwrap();
    p.check_sources(&s).unwrap();
    let mut settings = (*s.catalog.photo(ids[1]).unwrap().develop).clone();
    settings.orientation = lightcraft_geom::Orientation::Rotate180;
    s.apply_catalog(crate::catalog::Op::SetDevelop { id: ids[1], settings: std::sync::Arc::new(settings), label: "Rotate".into(), edited: None })
        .unwrap();
    assert!(p.check_sources(&s).is_err());
    let q = crate::alignment::coordinates::remap(&p, &s).unwrap();
    let at = q.members[1].points[0].target;
    assert!((at[0] - 0.9).abs() < 1e-7 && (at[1] - 0.9).abs() < 1e-7);
    q.check_sources(&s).unwrap();
}
#[test]
fn separate_exports_same_size_never_overwrite() {
    let (mut s, ids) = session();
    let p = aligned(&s, &ids);
    let dir = std::env::temp_dir().join(format!("lc-alignment-export-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let before: Vec<_> = ids.iter().map(|id| s.catalog.photo(*id).unwrap().develop.clone()).collect();
    let files = crate::alignment::jobs::export(&mut s, &p, dir.to_str().unwrap(), false).unwrap().run(&|_, _| true).unwrap();
    assert_eq!(files.len(), 2);
    for path in &files {
        let b = std::fs::read(path).unwrap();
        assert_eq!(u32::from_be_bytes(b[16..20].try_into().unwrap()), 64);
        assert_eq!(u32::from_be_bytes(b[20..24].try_into().unwrap()), 48);
    }
    assert!(crate::alignment::jobs::export(&mut s, &p, dir.to_str().unwrap(), false).is_err());
    for (id, settings) in ids.iter().zip(before) {
        assert_eq!(s.catalog.photo(*id).unwrap().develop, settings);
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn alignment_working_backup_and_membership() {
    let (mut s, ids) = session();
    let p = aligned(&s, &ids);
    s.alignment.replace(p.clone());
    let dir = std::env::temp_dir().join(format!("lc-backup-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("set.lcalign.json");
    s.execute("alignment.save", &json!({"path":path})).unwrap();
    let mut edited = p.clone();
    edited.name = "Edited".into();
    s.execute("alignment.replace", &json!({"project":edited})).unwrap();
    let backup = crate::alignment::backup_path(path.to_str().unwrap());
    assert_eq!(Project::load(&backup).unwrap(), edited);
    assert_eq!(Project::load(path.to_str().unwrap()).unwrap(), p);
    assert!(s.alignment.dirty);
    let third = s.catalog.photos().nth(2).unwrap().id;
    s.execute("alignment.add", &json!({"ids":[third.0]})).unwrap();
    assert_eq!(s.alignment.project.as_ref().unwrap().members.len(), 3);
    s.execute("alignment.reference", &json!({"index":1})).unwrap();
    assert_eq!(s.alignment.project.as_ref().unwrap().members[0].photo, ids[1]);
    assert!(s.alignment.project.as_ref().unwrap().transform(1).unwrap().rms < 1e-8);
    s.execute("alignment.remove", &json!({"index":2})).unwrap();
    assert_eq!(s.alignment.project.as_ref().unwrap().members.len(), 2);
    s.execute("alignment.discard", &json!({})).unwrap();
    assert!(!std::path::Path::new(&backup).exists());
    assert_eq!(Project::load(path.to_str().unwrap()).unwrap(), p);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn alignment_default_names_and_recovery() {
    use lightcraft_catalog::{Op, Photo, PhotoId, Source};
    let (mut s, _) = session();
    let dir = std::env::temp_dir().join(format!("lc-recovery-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut ids = Vec::new();
    for (i, name) in ["Before.jpg", "After.jpg"].iter().enumerate() {
        let id = PhotoId(9000 + i as u64);
        let photo = Photo::new(id, Source::File { path: dir.join(name).to_string_lossy().into() }, name, "JPEG", 100, 100, "");
        s.catalog.apply(Op::AddPhoto { photo: Box::new(photo) }).unwrap();
        ids.push(id.0);
    }
    s.execute("alignment.create", &json!({"ids":ids})).unwrap();
    let path = s.alignment.path.clone().unwrap();
    assert!(path.ends_with("Before_After.lcalign.json"));
    let mut p = s.alignment.project.clone().unwrap();
    p.name = "Recovered edits".into();
    s.execute("alignment.replace", &json!({"project":p})).unwrap();
    // Simulate losing only the in-memory workspace, leaving the durable working copy.
    s.alignment = Default::default();
    s.execute("alignment.create", &json!({"ids":ids})).unwrap();
    assert_eq!(s.alignment.project.as_ref().unwrap().name, "Recovered edits");
    let mut named = p.clone();
    named.members[0].name = "非常に長いファイル名abcdefghijklmnop.jpg".into();
    let name = named.default_path().unwrap();
    assert!(name.ends_with("+1.lcalign.json"));
    s.execute("alignment.discard", &json!({})).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
