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
