//! Headless inspection of the actual workspace, not an HTML mock-up.
use crate::{LightcraftApp, Services, headless::Headless};
use lightcraft_engine::{
    Session,
    alignment::{Pair, Project},
};
use std::time::Duration;
#[test]
fn alignment_workspace_renders_and_guards_unsaved_quit() {
    let mut session = Session::with_demo();
    let ids: Vec<_> = session.catalog.photos().take(2).map(|p| p.id).collect();
    let mut p = Project::create(&session, &ids, "Before / after".into()).unwrap();
    p.members[1].points = vec![Pair::new([0.25, 0.35], [0.28, 0.34]), Pair::new([0.7, 0.65], [0.73, 0.64])];
    session.alignment.replace(p.clone());
    let mut h = Headless::new(LightcraftApp::new(session, Services::default()), [1600., 1100.], 1.);
    crate::alignment::open(&mut h.app).unwrap();
    h.settle(Duration::from_secs(10));
    assert!(!crate::panels::notices::may_close(&mut h.app));
    assert!(h.app.quit_prompt.is_some());
    h.app.quit_prompt = None;
    // The synthetic textures make this screenshot deterministic and avoid dependence on a GPU or camera file.
    super::alignment::test_images(&mut h);
    let image = h.snapshot(Duration::from_secs(10));
    assert_eq!(image.size, [1600, 1100]);
    if let Ok(path) = std::env::var("LIGHTCRAFT_ALIGNMENT_SCREENSHOT") {
        let pixels =
            lightcraft_raster::Rgba8 { width: image.size[0], height: image.size[1], data: image.pixels.iter().map(|p| p.to_array()).collect() };
        let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&pixels), &Default::default()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    assert_eq!(h.app.session.alignment.project.as_ref().unwrap(), &p);
    // Drive real pointer input on the two canvases, then use alignment undo.
    for name in ["alignment.reference", "alignment.target"] {
        let rect = h.app.widgets.iter().find(|(id, _)| id == name).map(|(_, r)| *r).unwrap();
        let at = rect.min + egui::vec2(rect.width() * 0.45, rect.height() * 0.22);
        h.app.synthetic.push(egui::Event::PointerMoved(at));
        for pressed in [true, false] {
            h.app.synthetic.push(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            });
        }
        h.settle(Duration::from_secs(10));
    }
    assert_eq!(h.app.session.alignment.project.as_ref().unwrap().members[1].points.len(), 3);
    h.app.session.execute("alignment.undo", &serde_json::json!({})).unwrap();
    assert_eq!(h.app.session.alignment.project.as_ref().unwrap(), &p);
}

#[test]
fn dragging_landmark_persists_across_frames() {
    let session = Session::with_demo();
    let ids: Vec<_> = session.catalog.photos().take(2).map(|p| p.id).collect();
    let mut p = Project::create(&session, &ids, "Drag".into()).unwrap();
    p.members[1].points = vec![Pair::new([0.25, 0.35], [0.28, 0.34]), Pair::new([0.7, 0.65], [0.73, 0.64])];
    let mut h = Headless::new(LightcraftApp::new(session, Services::default()), [1600., 1100.], 1.);
    h.app.session.alignment.replace(p);
    crate::alignment::open(&mut h.app).unwrap();
    h.settle(Duration::from_secs(10));
    super::alignment::test_images(&mut h);
    h.settle(Duration::from_secs(10));
    let rect = h.app.widgets.iter().find(|(id, _)| id == "alignment.target").map(|(_, r)| *r).unwrap();
    let start = rect.min + egui::vec2(rect.width() * 0.28, rect.height() * 0.34);
    let end = start + egui::vec2(30., 20.);
    h.app.synthetic.push(egui::Event::PointerMoved(start));
    h.app.synthetic.push(egui::Event::PointerButton {
        pos: start,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Default::default(),
    });
    h.step();
    h.app.synthetic.push(egui::Event::PointerMoved(end));
    h.step();
    h.app.synthetic.push(egui::Event::PointerButton {
        pos: end,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Default::default(),
    });
    h.step();
    h.settle(Duration::from_secs(10));
    let target = h.app.session.alignment.project.as_ref().unwrap().members[1].points[0].target;
    assert!((target[0] - (0.28 + 30. / f64::from(rect.width()))).abs() < 0.001);
    assert!((target[1] - (0.34 + 20. / f64::from(rect.height()))).abs() < 0.001);
}
