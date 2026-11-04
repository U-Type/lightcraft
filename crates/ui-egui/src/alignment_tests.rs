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
