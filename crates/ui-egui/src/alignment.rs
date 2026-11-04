//! Dedicated alignment workspace; upstream Develop and Photo Merge stay independent.
use crate::LightcraftApp;
use egui::{Color32, Pos2, Rect, Vec2};
use lightcraft_engine::alignment::{Model, Pair, Project, jobs};
use lightcraft_raster::Rgba8;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
};
pub const CUSTOM_BUILD: bool = true;
#[derive(Default)]
pub struct Workspace {
    pub open: bool,
    draft: Option<Project>,
    revision: u64,
    target: usize,
    images: Vec<Rgba8>,
    textures: Vec<egui::TextureHandle>,
    review: Option<egui::TextureHandle>,
    review_key: String,
    fit_key: String,
    fit_result: Option<Result<lightcraft_engine::alignment::Transform, String>>,
    pending: Option<[f64; 2]>,
    selected: Option<usize>,
    zoom: f32,
    center: [f32; 2],
    mode: usize,
    opacity: f32,
    path: String,
    directory: String,
    jpeg: bool,
    message: String,
    task: Option<Receiver<Message>>,
    cancel: Option<Arc<AtomicBool>>,
    task_project: Option<Project>,
    task_target: usize,
    preview_settings: Vec<u64>,
    task_settings: Vec<u64>,
    pick: Option<(bool, Receiver<Vec<String>>)>,
}
enum Message {
    Previews(Result<Vec<Rgba8>, String>),
    Auto(Result<Vec<Pair>, String>),
    Export(Result<Vec<String>, String>),
}
impl Workspace {
    pub fn busy(&self) -> bool {
        self.task.is_some()
    }
}
fn execute(app: &mut LightcraftApp, w: &mut Workspace, id: &str, p: serde_json::Value) -> bool {
    match app.session.execute(id, &p) {
        Ok(_) => {
            w.message.clear();
            w.draft = None;
            true
        }
        Err(e) => {
            w.message = e.to_string();
            false
        }
    }
}
fn cached_fit(w: &mut Workspace, p: &Project) -> Result<lightcraft_engine::alignment::Transform, String> {
    let key = serde_json::to_string(&(p.members.first().map(|m| m.aspect), w.target, p.members.get(w.target))).map_err(|e| e.to_string())?;
    if key != w.fit_key || w.fit_result.is_none() {
        w.fit_result = Some(p.transform(w.target));
        w.fit_key = key;
    }
    w.fit_result.clone().unwrap_or_else(|| Err("no fit".into()))
}
fn settings(app: &LightcraftApp, p: &Project) -> Vec<u64> {
    p.members.iter().map(|m| app.session.catalog.photo(m.photo).map(|p| p.develop.hash64()).unwrap_or(0)).collect()
}
fn spawn(ctx: &egui::Context, work: impl FnOnce() -> Message + Send + 'static) -> Result<Receiver<Message>, String> {
    let (tx, rx) = mpsc::channel();
    let ctx = ctx.clone();
    std::thread::Builder::new()
        .name("alignment".into())
        .spawn(move || {
            let _ = tx.send(work());
            ctx.request_repaint();
        })
        .map_err(|e| e.to_string())?;
    Ok(rx)
}
fn texture(ctx: &egui::Context, name: &str, img: &Rgba8) -> egui::TextureHandle {
    let bytes: Vec<u8> = img.data.iter().flat_map(|p| p.iter().copied()).collect();
    ctx.load_texture(name, egui::ColorImage::from_rgba_unmultiplied([img.width, img.height], &bytes), egui::TextureOptions::LINEAR)
}
fn load(app: &mut LightcraftApp, w: &mut Workspace, ctx: &egui::Context) {
    let Some(p) = app.session.alignment.project.clone() else { return };
    match jobs::previews(&mut app.session, &p).and_then(|job| spawn(ctx, move || Message::Previews(job.run()))) {
        Ok(rx) => {
            w.task_settings = settings(app, &p);
            w.task = Some(rx);
            w.task_project = Some(p);
            w.message = "Rendering current edits…".into();
        }
        Err(e) => w.message = e,
    }
}
fn poll(app: &mut LightcraftApp, w: &mut Workspace, ctx: &egui::Context) {
    if let Some((folder, rx)) = &w.pick {
        match rx.try_recv() {
            Ok(paths) => {
                if let Some(path) = paths.first() {
                    if *folder {
                        w.directory = path.clone();
                    } else {
                        w.path = path.clone();
                    }
                }
                w.pick = None;
            }
            Err(mpsc::TryRecvError::Disconnected) => w.pick = None,
            Err(_) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
        }
    }
    let Some(rx) = w.task.as_ref() else { return };
    let message = match rx.try_recv() {
        Ok(m) => m,
        Err(mpsc::TryRecvError::Empty) => return,
        Err(_) => {
            w.task = None;
            w.message = "Worker stopped unexpectedly; the alignment set is unchanged.".into();
            return;
        }
    };
    w.task = None;
    w.cancel = None;
    let same = app.session.alignment.project == w.task_project && w.task_project.as_ref().is_some_and(|p| settings(app, p) == w.task_settings);
    match message {
        Message::Previews(Ok(images)) if same => {
            w.textures = images.iter().enumerate().map(|(i, im)| texture(ctx, &format!("alignment-{i}"), im)).collect();
            w.images = images;
            w.preview_settings = w.task_settings.clone();
            w.review_key.clear();
            w.message.clear();
        }
        Message::Auto(Ok(points)) if same => {
            if let Some(mut p) = app.session.alignment.project.clone() {
                if let Some(m) = p.members.get_mut(w.task_target) {
                    m.points = points;
                }
                match p.transform(w.task_target) {
                    Ok(_) => {
                        execute(app, w, "alignment.replace", json!({"project":p}));
                    }
                    Err(e) => w.message = e,
                }
            }
        }
        Message::Export(Ok(files)) => w.message = format!("Exported {} aligned images.", files.len()),
        Message::Previews(Err(e)) | Message::Auto(Err(e)) | Message::Export(Err(e)) => w.message = e,
        _ => w.message = "Set or edits changed during the job; refresh previews.".into(),
    }
}
pub fn open(app: &mut LightcraftApp) -> Result<serde_json::Value, String> {
    if cfg!(target_arch = "wasm32") {
        return Err("Alignment workspace currently requires the desktop app".into());
    }
    app.alignment.open = true;
    Ok(json!({}))
}
fn browse(app: &mut LightcraftApp, w: &mut Workspace, kind: &str) {
    if w.pick.is_some() {
        return;
    }
    let request = match kind {
        "folder" => crate::pick::PickRequest::folder("Aligned output folder"),
        "open" => crate::pick::PickRequest::file("Choose alignment project", "Alignment project", &["json"]),
        _ => crate::pick::PickRequest::save("Save alignment project", "Alignment project", &["json"], "comparison.lcalign.json"),
    };
    if let Some(picker) = app.services.picker.as_mut() {
        match picker(request) {
            Ok(rx) => w.pick = Some((kind == "folder", rx)),
            Err(e) => w.message = e,
        }
    } else {
        w.message = "Type the project or output path in the field.".into();
    }
}
pub fn show(app: &mut LightcraftApp, ctx: &egui::Context) {
    let mut w = std::mem::take(&mut app.alignment);
    poll(app, &mut w, ctx);
    if !w.open || crate::panels::notices::blocks_input(app) || app.ui.dialog.is_some() {
        app.alignment = w;
        return;
    }
    if w.zoom < 1. {
        w.zoom = 1.;
        w.center = [0.5, 0.5];
        w.opacity = 0.5;
        w.target = 1;
    }
    if w.draft.is_none() || w.revision != app.session.alignment.revision {
        w.draft = app.session.alignment.project.clone();
        w.revision = app.session.alignment.revision;
    }
    let mut open = true;
    egui::Window::new("Align photos — control points & comparison")
        .id(egui::Id::unique("alignment-workspace"))
        .open(&mut open)
        .default_size([1240., 820.])
        .resizable(true)
        .vscroll(true)
        .show(ctx, |ui| {
            project_bar(app, &mut w, ui, ctx);
            if w.task.is_some() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Working…");
                    if let Some(c) = &w.cancel
                        && ui.button("Cancel export").clicked()
                    {
                        c.store(true, Ordering::Relaxed);
                    }
                });
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            if !w.message.is_empty() {
                ui.label(&w.message);
            }
            let Some(mut p) = w.draft.take().or_else(|| app.session.alignment.project.clone()) else {
                ui.label("Import and select two or more photos, then choose New from selection. The active selected image becomes the reference.");
                return;
            };
            if p.members.get(w.target).is_none() || w.target == 0 {
                w.target = 1;
            }
            if !w.images.is_empty() && w.preview_settings != settings(app, &p) {
                w.images.clear();
                w.textures.clear();
                w.review_key.clear();
                w.message = "Photo edits changed. Refresh previews before placing points or comparing.".into();
            }
            let mut changed = false;
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                ui.label("Set:");
                changed |= ui.text_edit_singleline(&mut p.name).changed();
                for (i, m) in p.members.iter().enumerate() {
                    if i == 0 {
                        ui.label(format!("Reference: {}", m.name));
                    } else if ui.selectable_label(w.target == i, format!("{i}: {}", m.name)).clicked() {
                        w.target = i;
                        w.pending = None;
                        w.selected = None;
                        w.review_key.clear();
                    }
                }
            });
            if let Err(e) = p.check_sources(&app.session) {
                ui.colored_label(Color32::YELLOW, e);
                ui.horizontal_wrapped(|ui| {
                    for (id, label) in [
                        ("alignment.remapGeometry", "Remap landmarks through changed geometry"),
                        ("alignment.resetGeometry", "Reset affected landmarks"),
                    ] {
                        if ui.button(label).clicked() && execute(app, &mut w, id, json!({})) {
                            w.images.clear();
                            w.textures.clear();
                            load(app, &mut w, ctx);
                        }
                    }
                });
            }
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(w.task.is_none(), egui::Button::new("Refresh edits / previews")).clicked() {
                    load(app, &mut w, ctx);
                }
                for (id, label) in [("alignment.undo", "Undo alignment"), ("alignment.redo", "Redo alignment")] {
                    if ui.button(label).clicked() {
                        execute(app, &mut w, id, json!({}));
                    }
                }
                ui.add(egui::Slider::new(&mut w.zoom, 1.0..=12.0).text("Linked zoom"));
                if ui.button("Fit").clicked() {
                    w.zoom = 1.;
                    w.center = [0.5, 0.5];
                }
                for (i, label) in ["Point pairs", "Overlay", "Split", "Difference", "Blink"].iter().enumerate() {
                    ui.selectable_value(&mut w.mode, i, *label);
                }
            });
            changed |= model_bar(app, &mut w, &mut p, ui, ctx);
            let width = ui.available_width();
            let height = (ui.available_height() - 220.).clamp(200., 440.);
            if w.mode == 0 {
                ui.label(if w.pending.is_some() {
                    "Click the matching point on the target (right)."
                } else {
                    "Click reference (left), then target (right). Drag numbered points to refine. Right-drag pans both images."
                });
                if let Some(m) = p.members.get_mut(w.target) {
                    ui.horizontal(|ui| {
                        for side in 0..2 {
                            let index = if side == 0 { 0 } else { w.target };
                            if let Some(tex) = w.textures.get(index) {
                                let (response, rect) = canvas(ui, tex, Vec2::new((width - 12.) / 2., height), w.zoom, w.center);
                                crate::widgets::register(ui, if side == 0 { "alignment.reference" } else { "alignment.target" }, rect);
                                changed |= points(ui, &response, rect, side, m, &mut w.pending, &mut w.selected);
                                if response.dragged_by(egui::PointerButton::Secondary) {
                                    let d = response.drag_delta();
                                    w.center[0] = (w.center[0] - d.x / rect.width()).clamp(0., 1.);
                                    w.center[1] = (w.center[1] - d.y / rect.height()).clamp(0., 1.);
                                }
                                magnifier(ui, &response, rect, tex);
                            } else {
                                ui.allocate_space(Vec2::new((width - 12.) / 2., height));
                            }
                        }
                    });
                }
            } else {
                review(ui, &mut w, &p, ctx, Vec2::new(width, height));
            }
            match cached_fit(&mut w, &p) {
                Ok(t) => {
                    ui.label(format!(
                        "Fit RMS: {:.2} target-height pixels at {} px. Spread points across the feature of interest.",
                        t.rms * p.height as f64,
                        p.height
                    ));
                }
                Err(e) => {
                    ui.label(e);
                }
            }
            changed |= point_table(&mut w, &mut p, ui);
            changed |= export_bar(app, &mut w, &mut p, ui, ctx);
            if changed {
                execute(app, &mut w, "alignment.replace", json!({"project":p}));
            } else if w.draft.is_none() && w.revision == app.session.alignment.revision {
                w.draft = Some(p);
            }
        });
    w.open = open;
    app.alignment = w;
}
fn project_bar(app: &mut LightcraftApp, w: &mut Workspace, ui: &mut egui::Ui, ctx: &egui::Context) {
    ui.horizontal_wrapped(|ui| {
        if ui.add_enabled(w.task.is_none(), egui::Button::new("New from selection")).clicked() {
            let r = app.session.active().map(|id| id.0);
            if execute(app, w, "alignment.create", json!({"reference":r})) {
                w.images.clear();
                w.textures.clear();
                w.pending = None;
                load(app, w, ctx);
            }
        }
        ui.label("Project:");
        ui.add(egui::TextEdit::singleline(&mut w.path).desired_width(250.).hint_text("C:\\Photos\\comparison.lcalign.json"));
        if ui.button("Choose…").clicked() {
            browse(app, w, "open");
        }
        if ui.add_enabled(w.task.is_none(), egui::Button::new("Open")).clicked() {
            let path = w.path.clone();
            if execute(app, w, "alignment.open", json!({"path":path})) {
                w.images.clear();
                w.textures.clear();
                w.pending = None;
                load(app, w, ctx);
            }
        }
        if ui.button("Save as…").clicked() {
            browse(app, w, "save");
        }
        if ui.button("Save").clicked() {
            let path = w.path.clone();
            execute(app, w, "alignment.save", json!({"path":path}));
        }
        if app.session.alignment.dirty {
            ui.colored_label(Color32::YELLOW, "Unsaved changes");
        }
        ui.menu_button("Discard…", |ui| {
            ui.label("Discard this set's unsaved edits?");
            if ui.button("Discard set").clicked() {
                execute(app, w, "alignment.discard", json!({}));
                w.images.clear();
                w.textures.clear();
                ui.close();
            }
        });
    });
}
fn model_bar(app: &mut LightcraftApp, w: &mut Workspace, p: &mut Project, ui: &mut egui::Ui, ctx: &egui::Context) -> bool {
    let mut changed = false;
    let Some(m) = p.members.get_mut(w.target) else { return false };
    ui.horizontal_wrapped(|ui| {
        changed |= ui.checkbox(&mut m.included, "Include target").changed();
        craft_ui::ComboBox::from_id_salt("alignment-model").selected_text(format!("{:?}", m.model)).show_ui(ui, |ui| {
            for (model, label) in [
                (Model::Translation, "Position (1 pair)"),
                (Model::Similarity, "Rotation + scale (2 pairs)"),
                (Model::Affine, "Affine (3 pairs)"),
                (Model::Perspective, "Perspective (4 pairs)"),
                (Model::ThinPlate, "Local warp (3+ pairs)"),
            ] {
                changed |= ui.selectable_value(&mut m.model, model, label).changed();
            }
        });
        if m.model == Model::ThinPlate {
            changed |= ui.add(egui::Slider::new(&mut m.smoothing, 0.0..=0.1).text("Warp smoothing")).changed();
            ui.label("Local warp changes shape. Compare the unwarped photos too.");
        }
        if ui.add_enabled(w.task.is_none() && m.points.is_empty() && w.images.get(w.target).is_some(), egui::Button::new("Suggest points")).clicked()
            && let (Some(a), Some(b)) = (w.images.first(), w.images.get(w.target))
        {
            let (a, b) = (a.clone(), b.clone());
            match spawn(ctx, move || Message::Auto(jobs::suggest(&a, &b))) {
                Ok(rx) => {
                    w.task = Some(rx);
                    w.task_target = w.target;
                    w.task_project = app.session.alignment.project.clone();
                    w.task_settings = w.preview_settings.clone();
                }
                Err(e) => w.message = e,
            }
        }
        if ui.button("Remove selected pair").clicked()
            && let Some(i) = w.selected.take()
            && i < m.points.len()
        {
            m.points.remove(i);
            changed = true;
        }
        ui.menu_button("Clear pairs…", |ui| {
            if ui.button("Clear target landmarks").clicked() {
                m.points.clear();
                w.pending = None;
                changed = true;
                ui.close();
            }
        });
    });
    changed
}
fn point_table(w: &mut Workspace, p: &mut Project, ui: &mut egui::Ui) -> bool {
    let mut changed = false;
    let residuals = cached_fit(w, p).ok().map(|t| t.residuals).unwrap_or_default();
    egui::ScrollArea::vertical().id_salt("alignment-point-table").max_height(100.).show(ui, |ui| {
        if let Some(m) = p.members.get_mut(w.target) {
            egui::Grid::new("alignment-points").striped(true).show(ui, |ui| {
                for (i, pair) in m.points.iter_mut().enumerate() {
                    if ui.selectable_label(w.selected == Some(i), format!("{}", i + 1)).clicked() {
                        w.selected = Some(i);
                    }
                    changed |= ui.checkbox(&mut pair.enabled, "Use").changed();
                    changed |= ui.add(egui::DragValue::new(&mut pair.weight).range(0.01..=100.).speed(0.1).prefix("Weight ")).changed();
                    for v in pair.reference.iter_mut().chain(&mut pair.target) {
                        changed |= ui.add(egui::DragValue::new(v).range(0.0..=1.0).speed(0.0001).max_decimals(5)).changed();
                    }
                    if let Some(Some(e)) = residuals.get(i) {
                        ui.label(format!("{:.2} px", e * p.height as f64));
                    }
                    ui.end_row();
                }
            });
        }
    });
    changed
}
fn export_bar(app: &mut LightcraftApp, w: &mut Workspace, p: &mut Project, ui: &mut egui::Ui, ctx: &egui::Context) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        if let Some(m) = p.members.first_mut() {
            changed |= ui.checkbox(&mut m.included, "Include reference").changed();
        }
        ui.label("Crop x/y/w/h:");
        for v in &mut p.crop {
            changed |= ui.add(egui::DragValue::new(v).range(0.0..=1.0).speed(0.001).max_decimals(4)).changed();
        }
        if ui.button("Common crop").clicked() {
            execute(app, w, "alignment.commonCrop", json!({}));
        }
        if ui.button("Full reference").clicked() {
            p.crop = [0., 0., 1., 1.];
            changed = true;
        }
        changed |= ui.add(egui::DragValue::new(&mut p.width).range(1..=16000).prefix("Width ")).changed();
        if ui.button("Match crop aspect").clicked()
            && let Some(m) = p.members.first()
        {
            p.height = (p.width as f64 * p.crop[3] / (m.aspect * p.crop[2].max(0.0001))).round().max(1.) as usize;
            changed = true;
        }
        changed |= ui.add(egui::DragValue::new(&mut p.height).range(1..=16000).prefix("Height ")).changed();
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Output folder:");
        ui.add(egui::TextEdit::singleline(&mut w.directory).desired_width(260.).hint_text("C:\\Photos\\Aligned"));
        if ui.button("Browse…").clicked() {
            browse(app, w, "folder");
        }
        ui.checkbox(&mut w.jpeg, "JPEG (otherwise PNG)");
        if ui.add_enabled(w.task.is_none(), egui::Button::new("Export separate images")).clicked() {
            match jobs::export(&mut app.session, p, &w.directory, w.jpeg) {
                Ok(job) => {
                    let cancel = Arc::new(AtomicBool::new(false));
                    let c = cancel.clone();
                    match spawn(ctx, move || Message::Export(job.run(&|_, _| !c.load(Ordering::Relaxed)))) {
                        Ok(rx) => {
                            w.task = Some(rx);
                            w.cancel = Some(cancel);
                            w.task_project = Some(p.clone());
                            w.message = "Exporting current edits, one photo at a time…".into();
                        }
                        Err(e) => w.message = e,
                    }
                }
                Err(e) => w.message = e,
            }
        }
    });
    ui.small("sRGB • 8-bit • original photos and edits preserved • existing exports never replaced");
    changed
}
fn canvas(ui: &mut egui::Ui, tex: &egui::TextureHandle, size: Vec2, zoom: f32, center: [f32; 2]) -> (egui::Response, Rect) {
    let (area, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    let ts = tex.size_vec2();
    let scale = (area.width() / ts.x).min(area.height() / ts.y) * zoom;
    let sz = ts * scale;
    let rect = Rect::from_min_size(area.center() - Vec2::new(center[0] * sz.x, center[1] * sz.y), sz);
    let painter = ui.painter().with_clip_rect(area);
    painter.rect_filled(area, 0., Color32::from_gray(20));
    painter.image(tex.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)), Color32::WHITE);
    (response, rect)
}
fn points(
    ui: &egui::Ui,
    response: &egui::Response,
    rect: Rect,
    side: usize,
    m: &mut lightcraft_engine::alignment::Member,
    pending: &mut Option<[f64; 2]>,
    selected: &mut Option<usize>,
) -> bool {
    let mut changed = false;
    let painter = ui.painter().with_clip_rect(response.rect);
    let screen = |p: [f64; 2]| rect.min + Vec2::new(p[0] as f32 * rect.width(), p[1] as f32 * rect.height());
    let nearest = response.interact_pointer_pos().and_then(|pos| {
        m.points
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                let d = screen(if side == 0 { p.reference } else { p.target }).distance(pos);
                (d < 12.).then_some((i, d))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|v| v.0)
    });
    if (response.is_pointer_button_down_on() && ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Primary))) || response.clicked() {
        *selected = nearest;
    }
    if let Some(pos) = response.interact_pointer_pos().filter(|p| rect.contains(*p)) {
        let q = [f64::from((pos.x - rect.min.x) / rect.width()).clamp(0., 1.), f64::from((pos.y - rect.min.y) / rect.height()).clamp(0., 1.)];
        if response.dragged_by(egui::PointerButton::Primary)
            && let Some(p) = selected.and_then(|i| m.points.get_mut(i))
        {
            if side == 0 {
                p.reference = q;
            } else {
                p.target = q;
            }
        }
        if response.clicked() && nearest.is_none() {
            if side == 0 {
                *pending = Some(q);
            } else if let Some(a) = pending.take()
                && m.points.len() < 128
            {
                m.points.push(Pair::new(a, q));
                changed = true;
            }
        }
    }
    if response.drag_stopped_by(egui::PointerButton::Primary) {
        changed = true;
    }
    for (i, p) in m.points.iter().enumerate() {
        let at = screen(if side == 0 { p.reference } else { p.target });
        let color = if *selected == Some(i) {
            Color32::YELLOW
        } else if p.enabled {
            Color32::LIGHT_GREEN
        } else {
            Color32::GRAY
        };
        painter.circle_stroke(at, 6., egui::Stroke::new(2., color));
        painter.text(at + Vec2::new(8., -8.), egui::Align2::LEFT_BOTTOM, (i + 1).to_string(), egui::FontId::monospace(14.), color);
    }
    if side == 0
        && let Some(p) = pending
    {
        painter.circle_stroke(screen(*p), 8., egui::Stroke::new(2., Color32::YELLOW));
    }
    changed
}
fn magnifier(ui: &egui::Ui, response: &egui::Response, rect: Rect, tex: &egui::TextureHandle) {
    let Some(pos) = response.hover_pos().filter(|p| rect.contains(*p)) else { return };
    let uv = Pos2::new((pos.x - rect.min.x) / rect.width(), (pos.y - rect.min.y) / rect.height());
    let half = Vec2::new(18. / tex.size_vec2().x, 18. / tex.size_vec2().y);
    let lens = Rect::from_min_size(response.rect.right_top() + Vec2::new(-130., 8.), Vec2::splat(120.));
    let painter = ui.painter().with_clip_rect(response.rect);
    painter.image(tex.id(), lens, Rect::from_min_max(uv - half, uv + half), Color32::WHITE);
    painter.rect_stroke(lens, 0., egui::Stroke::new(1., Color32::YELLOW), egui::StrokeKind::Inside);
    let c = lens.center();
    painter.line_segment([c - Vec2::new(8., 0.), c + Vec2::new(8., 0.)], egui::Stroke::new(1., Color32::YELLOW));
    painter.line_segment([c - Vec2::new(0., 8.), c + Vec2::new(0., 8.)], egui::Stroke::new(1., Color32::YELLOW));
}
fn review(ui: &mut egui::Ui, w: &mut Workspace, p: &Project, ctx: &egui::Context, size: Vec2) {
    let t = match cached_fit(w, p) {
        Ok(t) => t,
        Err(e) => {
            ui.label(e);
            return;
        }
    };
    let (Some(reference), Some(target)) = (w.images.first(), w.images.get(w.target)) else {
        ui.label("Refresh previews to compare.");
        return;
    };

    let Some(r) = p.members.first() else { return };
    let key = format!("{}:{}:{}", w.target, w.mode, serde_json::to_string(p).unwrap_or_default());
    let height = (720. / r.aspect).round().clamp(1., 4096.) as usize;
    if w.review_key != key {
        match lightcraft_engine::alignment::warp_preview(target, &t, 720, height) {
            Ok(mut image) => {
                if w.mode == 3 {
                    let rt = lightcraft_engine::alignment::Transform::identity(r.aspect);
                    if let Ok(ref_im) = lightcraft_engine::alignment::warp_preview(reference, &rt, 720, height) {
                        for (a, b) in image.data.iter_mut().zip(ref_im.data) {
                            for k in 0..3 {
                                a[k] = a[k].abs_diff(b[k]);
                            }
                            a[3] = 255;
                        }
                    }
                }
                w.review = Some(texture(ctx, "alignment-review", &image));
                w.review_key = key;
            }
            Err(e) => {
                ui.label(e);
                return;
            }
        }
    }
    ui.add(egui::Slider::new(&mut w.opacity, 0.0..=1.0).text(if w.mode == 2 { "Split position" } else { "Target opacity" }));
    let Some(base) = w.textures.first() else { return };
    let (response, rect) = canvas(ui, base, size, w.zoom, w.center);
    if let Some(review) = &w.review {
        let mut clip = response.rect;
        let mut alpha = if w.mode == 1 { w.opacity } else { 1. };
        if w.mode == 2 {
            clip.min.x += clip.width() * w.opacity;
        }
        if w.mode == 4 {
            alpha = if ctx.input(|i| ((i.time * 2.) as u64).is_multiple_of(2)) { 1. } else { 0. };
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        ui.painter().with_clip_rect(clip).image(
            review.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
            Color32::from_white_alpha((alpha * 255.) as u8),
        );
    }
    let c = p.crop;
    let crop = Rect::from_min_size(
        rect.min + Vec2::new(c[0] as f32 * rect.width(), c[1] as f32 * rect.height()),
        Vec2::new(c[2] as f32 * rect.width(), c[3] as f32 * rect.height()),
    );
    ui.painter().with_clip_rect(response.rect).rect_stroke(crop, 0., egui::Stroke::new(2., Color32::YELLOW), egui::StrokeKind::Inside);
}

#[cfg(test)]
pub(crate) fn test_images(h: &mut crate::headless::Headless) {
    let im = Rgba8::from_fn(600, 400, |x, y| {
        let dx = x as f64 - 300.;
        let dy = y as f64 - 200.;
        if (dx / 210.).powi(2) + (dy / 110.).powi(2) < 1. {
            if x % 60 < 45 && y > 140 && y < 250 { [230, 224, 208, 255] } else { [150, 74, 76, 255] }
        } else {
            [38, 44, 49, 255]
        }
    });
    h.app.alignment.images = vec![im.clone(), im];
    h.app.alignment.textures =
        h.app.alignment.images.iter().enumerate().map(|(i, im)| texture(&h.view.ctx, &format!("alignment-test-{i}"), im)).collect();
    if let Some(p) = &h.app.session.alignment.project {
        h.app.alignment.preview_settings = settings(&h.app, p);
    }
}
