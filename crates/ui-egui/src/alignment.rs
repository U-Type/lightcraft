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
/// Increment once for each prepared custom build: v0.1, v0.2, ...
pub const BUILD_VERSION: &str = "v0.1";
#[derive(Clone)]
enum AfterSave {
    Close,
    Quit,
    New,
    Open(String),
}
#[derive(Clone, Copy, PartialEq)]
enum PickAction {
    Folder,
    Open,
    Save,
}
#[derive(Clone, Copy)]
struct View {
    zoom: f32,
    center: [f32; 2],
}
impl Default for View {
    fn default() -> Self {
        Self { zoom: 1., center: [0.5, 0.5] }
    }
}
#[derive(Default)]
pub struct Workspace {
    pub open: bool,
    target: usize,
    images: Vec<Rgba8>,
    textures: Vec<egui::TextureHandle>,
    review: Option<egui::TextureHandle>,
    review_key: String,
    fit_key: String,
    fit_result: Option<Result<lightcraft_engine::alignment::Transform, String>>,
    pending: Option<[f64; 2]>,
    selected: Option<usize>,
    views: [View; 3],
    linked: bool,
    result_linked: bool,
    inverse_scroll: bool,
    focused: usize,
    result_target: usize,
    initialized: bool,
    needs_load: bool,
    close_action: Option<AfterSave>,
    add_open: bool,
    add_selected: std::collections::BTreeSet<u64>,
    add_search: String,
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
    pick: Option<(PickAction, Receiver<Vec<String>>)>,
}
enum Message {
    Previews(Result<Vec<Rgba8>, String>),
    Auto(Result<Vec<Pair>, String>),
    Export(Result<Vec<String>, String>),
}
impl Workspace {
    pub(crate) fn prompt_layer(&self) -> Option<egui::LayerId> {
        self.close_action.as_ref().map(|_| egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("alignment-close")))
    }
    pub fn busy(&self) -> bool {
        self.task.is_some()
    }
}
fn execute(app: &mut LightcraftApp, w: &mut Workspace, id: &str, p: serde_json::Value) -> bool {
    match app.session.execute(id, &p) {
        Ok(_) => {
            w.message.clear();
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
    if let Some((action, rx)) = &w.pick {
        match rx.try_recv() {
            Ok(paths) => {
                let action = *action;
                w.pick = None;
                if let Some(path) = paths.first() {
                    match action {
                        PickAction::Folder => w.directory = path.clone(),
                        PickAction::Open => request_action(app, w, AfterSave::Open(path.clone())),
                        PickAction::Save => {
                            w.path = if path.ends_with(".lcalign.json") { path.clone() } else { format!("{path}.lcalign.json") };
                            if save(app, w) {
                                finish_action(app, w, ctx);
                            }
                        }
                    }
                }
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
    let mut w = std::mem::take(&mut app.alignment);
    w.open = true;
    if w.path.is_empty() {
        w.path = app.session.alignment.path.clone().unwrap_or_default();
    }
    let ids = app.session.selected();
    let same =
        app.session.alignment.project.as_ref().is_some_and(|p| p.members.len() == ids.len() && p.members.iter().all(|m| ids.contains(&m.photo)));
    if ids.len() >= 2 && !same {
        request_action(app, &mut w, AfterSave::New);
    } else if app.session.alignment.project.is_some() && w.images.is_empty() {
        w.needs_load = true;
    }
    app.alignment = w;
    Ok(json!({}))
}
pub fn request_quit(app: &mut LightcraftApp) {
    app.alignment.open = true;
    app.alignment.close_action = Some(AfterSave::Quit);
}
fn request_action(app: &mut LightcraftApp, w: &mut Workspace, action: AfterSave) {
    w.close_action = Some(action);
    if !app.session.alignment.dirty {
        perform_action(app, w);
    }
}
fn save(app: &mut LightcraftApp, w: &mut Workspace) -> bool {
    let path = w.path.clone();
    execute(app, w, "alignment.save", json!({"path":path}))
}
fn finish_action(app: &mut LightcraftApp, w: &mut Workspace, ctx: &egui::Context) {
    let quit = matches!(w.close_action, Some(AfterSave::Quit));
    perform_action(app, w);
    if quit {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}
fn perform_action(app: &mut LightcraftApp, w: &mut Workspace) {
    let Some(action) = w.close_action.take() else { return };
    match action {
        AfterSave::Close | AfterSave::Quit => w.open = false,
        AfterSave::New => {
            let r = app.session.active().map(|id| id.0);
            if execute(app, w, "alignment.create", json!({"reference":r})) {
                reset_previews(app, w);
            }
        }
        AfterSave::Open(path) => {
            if execute(app, w, "alignment.open", json!({"path":path})) {
                reset_previews(app, w);
            }
        }
    }
}
fn reset_previews(app: &LightcraftApp, w: &mut Workspace) {
    w.images.clear();
    w.textures.clear();
    w.review_key.clear();
    w.pending = None;
    w.selected = None;
    w.target = 1;
    w.result_target = 1;
    w.needs_load = true;
    w.path = app.session.alignment.path.clone().unwrap_or_default();
    if w.directory.is_empty()
        && let Some(parent) = std::path::Path::new(&w.path).parent()
    {
        w.directory = parent.join("Aligned").to_string_lossy().into_owned();
    }
}
fn browse(app: &mut LightcraftApp, w: &mut Workspace, kind: &str) {
    if w.pick.is_some() {
        return;
    }
    let name = std::path::Path::new(&w.path).file_name().and_then(|s| s.to_str()).unwrap_or("comparison.lcalign.json");
    let (action, request) = match kind {
        "folder" => (PickAction::Folder, crate::pick::PickRequest::folder("Aligned output folder")),
        "open" => (PickAction::Open, crate::pick::PickRequest::file("Open alignment project or working backup", "Alignment project", &["json"])),
        _ => (PickAction::Save, crate::pick::PickRequest::save("Save alignment project", "Alignment project", &["json"], name)),
    };
    if let Some(picker) = app.services.picker.as_mut() {
        match picker(request) {
            Ok(rx) => w.pick = Some((action, rx)),
            Err(e) => w.message = e,
        }
    } else {
        w.message = "Type a project path and press Save.".into();
    }
}
fn close_prompt(app: &mut LightcraftApp, w: &mut Workspace, ctx: &egui::Context) {
    if w.close_action.is_none() {
        return;
    }
    egui::Modal::new(egui::Id::unique("alignment-close")).show(ctx, |ui| {
        ui.set_max_width(480.);
        ui.heading("Save alignment project?");
        ui.label("Save keeps a project checkpoint. Discard removes the working backup and leaves any previously saved project unchanged.");
        ui.label(&w.path);
        if !w.message.is_empty() {
            ui.colored_label(Color32::YELLOW, &w.message);
        }
        if w.pick.is_some() {
            ui.label("Waiting for file dialog…");
        }
        ui.horizontal(|ui| {
            if ui.add_enabled(w.pick.is_none(), egui::Button::new("Save")).clicked() {
                if w.path.is_empty() {
                    browse(app, w, "save");
                } else if save(app, w) {
                    finish_action(app, w, ctx);
                }
            }
            if ui.add_enabled(w.pick.is_none(), egui::Button::new("Discard")).clicked() && execute(app, w, "alignment.discard", json!({})) {
                finish_action(app, w, ctx);
            }
            if ui.add_enabled(w.pick.is_none(), egui::Button::new("Save As…")).clicked() {
                browse(app, w, "save");
            }
            let cancel = ui.button("Cancel");
            crate::widgets::register(ui, "alignment.close.cancel", cancel.rect);
            if cancel.clicked() {
                w.close_action = None;
                w.pick = None;
            }
        });
    });
}
pub fn show(app: &mut LightcraftApp, ctx: &egui::Context) {
    let mut w = std::mem::take(&mut app.alignment);
    poll(app, &mut w, ctx);
    if !w.initialized {
        w.initialized = true;
        w.linked = true;
        w.opacity = 0.5;
        w.target = 1;
        w.result_target = 1;
        w.mode = 1;
    }
    if w.needs_load && w.task.is_none() {
        w.needs_load = false;
        load(app, &mut w, ctx);
    }
    if !w.open || crate::panels::notices::blocks_input(app) || app.ui.dialog.is_some() {
        app.alignment = w;
        return;
    }
    let mut open = true;
    egui::Window::new(format!("Align Photos {BUILD_VERSION}"))
        .id(egui::Id::unique("alignment-workspace"))
        .open(&mut open)
        .default_size([1320., 900.])
        .min_size([800., 600.])
        .resizable(true)
        .show(ctx, |ui| {
            ui.set_min_size(Vec2::new(760., 540.));
            if w.close_action.is_some() {
                ui.disable();
            }
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
            if let Some(error) = &app.session.alignment.backup_error {
                ui.colored_label(Color32::YELLOW, format!("Backup not saved: {error}"));
            }
            let Some(mut p) = app.session.alignment.project.clone() else {
                project_bar(app, &mut w, ui);
                ui.label("Select at least two imported photos, then open Align Photos.");
                return;
            };
            if p.members.get(w.target).is_none() || w.target == 0 {
                w.target = 1;
            }
            if p.members.get(w.result_target).is_none() {
                w.result_target = w.target;
            }
            if !w.images.is_empty() && w.preview_settings != settings(app, &p) {
                w.images.clear();
                w.textures.clear();
                w.review_key.clear();
                w.needs_load = true;
            }
            let revision = app.session.alignment.revision;
            let mut changed = false;
            egui::Panel::left("alignment-settings").resizable(true).default_size(280.).size_range(220.0..=520.0).show(ui, |ui| {
                egui::ScrollArea::vertical().id_salt("alignment-settings-scroll").show(ui, |ui| {
                    egui::CollapsingHeader::new("Project").default_open(true).show(ui, |ui| {
                        project_bar(app, &mut w, ui);
                    });
                    egui::CollapsingHeader::new("Set").default_open(true).show(ui, |ui| {
                        changed |= set_panel(app, &mut w, &mut p, ui);
                    });
                    egui::CollapsingHeader::new("Alignment").default_open(true).show(ui, |ui| {
                        changed |= model_bar(app, &mut w, &mut p, ui, ctx);
                    });
                    egui::CollapsingHeader::new("Export").default_open(true).show(ui, |ui| {
                        changed |= export_bar(app, &mut w, &mut p, ui, ctx);
                    });
                });
            });
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
                display_bar(app, &mut w, ui, ctx);
                if let Err(e) = p.check_sources(&app.session) {
                    ui.colored_label(Color32::YELLOW, e);
                    for (id, label) in [
                        ("alignment.remapGeometry", "Remap landmarks through changed geometry"),
                        ("alignment.resetGeometry", "Reset affected landmarks"),
                    ] {
                        if ui.button(label).clicked() && execute(app, &mut w, id, json!({})) {
                            w.needs_load = true;
                        }
                    }
                }
                let width = ui.available_width();
                let height = ((ui.available_height() - 140.) / 2.).max(130.);
                ui.label(if w.pending.is_some() {
                    "Click the matching landmark in the target."
                } else {
                    "Click reference, then target. Wheel: zoom • Right-drag: pan"
                });
                ui.horizontal(|ui| {
                    for side in 0..2 {
                        let index = if side == 0 { 0 } else { w.target };
                        ui.vertical(|ui| {
                            ui.set_width((width - 12.) / 2.);
                            ui.label(format!(
                                "{}: {}",
                                if side == 0 { "Reference" } else { "Target" },
                                p.members.get(index).map(|m| m.name.as_str()).unwrap_or("")
                            ));
                            if let Some(tex) = w.textures.get(index).cloned() {
                                let v = w.views[side];
                                let (response, rect) = canvas(ui, &tex, Vec2::new((width - 12.) / 2., height), v.zoom, v.center);
                                crate::widgets::register(ui, if side == 0 { "alignment.reference" } else { "alignment.target" }, rect);
                                navigate(ui, &mut w, side, &response, rect);
                                if let Some(m) = p.members.get_mut(w.target) {
                                    changed |= points(ui, &response, rect, side, m, &mut w.pending, &mut w.selected);
                                }
                                magnifier(ui, &response, rect, &tex);
                            } else {
                                ui.allocate_space(Vec2::new((width - 12.) / 2., height));
                            }
                        });
                    }
                });
                ui.label(format!("Live result: {}", p.members.get(w.result_target).map(|m| m.name.as_str()).unwrap_or("")));
                review(ui, &mut w, &p, ctx, Vec2::new(width, height));
                match cached_fit(&mut w, &p) {
                    Ok(t) => {
                        ui.small(format!("Fit RMS: {:.2} target-height pixels", t.rms * p.height as f64));
                    }
                    Err(e) => {
                        ui.small(e);
                    }
                }
                egui::CollapsingHeader::new("Landmark coordinates and weights").show(ui, |ui| {
                    changed |= point_table(&mut w, &mut p, ui);
                });
            });
            if changed && revision == app.session.alignment.revision {
                execute(app, &mut w, "alignment.replace", json!({"project":p}));
            }
        });
    if !open {
        request_action(app, &mut w, AfterSave::Close);
    }
    add_dialog(app, &mut w, ctx);
    close_prompt(app, &mut w, ctx);
    app.alignment = w;
}
fn project_bar(app: &mut LightcraftApp, w: &mut Workspace, ui: &mut egui::Ui) {
    if ui.button("New from selection").clicked() {
        request_action(app, w, AfterSave::New);
    }
    ui.add(egui::TextEdit::singleline(&mut w.path).desired_width(ui.available_width()).hint_text("Project .lcalign.json path"));
    ui.horizontal_wrapped(|ui| {
        if ui.button("Open…").clicked() {
            browse(app, w, "open");
        }
        if ui.button("Save").clicked() {
            if w.path.is_empty() {
                browse(app, w, "save");
            } else {
                save(app, w);
            }
        }
        if ui.button("Save As…").clicked() {
            browse(app, w, "save");
        }
    });
    ui.small(if app.session.alignment.dirty { "Working copy • changes not yet saved to project" } else { "Project saved" });
    if let Some(path) = &app.session.alignment.path {
        ui.small("Working backup beside project").on_hover_text(lightcraft_engine::alignment::backup_path(path));
    }
    ui.horizontal_wrapped(|ui| {
        for (id, label) in [("alignment.undo", "Undo alignment"), ("alignment.redo", "Redo alignment")] {
            if ui.button(label).clicked() {
                execute(app, w, id, json!({}));
            }
        }
    });
}
fn display_bar(app: &mut LightcraftApp, w: &mut Workspace, ui: &mut egui::Ui, ctx: &egui::Context) {
    ui.horizontal_wrapped(|ui| {
        if ui.selectable_label(w.linked, "Linked zoom / pan").clicked() {
            w.linked = !w.linked;
            if w.linked {
                w.views[1] = w.views[0];
            }
        }
        if ui.toggle_value(&mut w.result_linked, "Lock result zoom / pan").changed() && w.result_linked {
            sync_views(w, w.focused);
        }
        ui.toggle_value(&mut w.inverse_scroll, "Invert wheel zoom");
        if ui.button("Fit active").clicked() {
            w.views[w.focused] = View::default();
            sync_views(w, w.focused);
        }
        if ui.add_enabled(w.task.is_none(), egui::Button::new("Refresh previews")).clicked() {
            load(app, w, ctx);
        }
    });
    ui.horizontal_wrapped(|ui| {
        for (mode, label) in [(1, "Overlay"), (2, "Split"), (3, "Difference"), (4, "Blink")] {
            ui.selectable_value(&mut w.mode, mode, label);
        }
        ui.add(egui::Slider::new(&mut w.opacity, 0.0..=1.0).text(if w.mode == 2 { "Split" } else { "Opacity" }));
    });
}
fn sync_views(w: &mut Workspace, side: usize) {
    let view = w.views[side];
    if side < 2 && w.linked {
        w.views[0] = view;
        w.views[1] = view;
    }
    if w.result_linked {
        w.views = [view; 3];
    }
}
fn navigate(ui: &mut egui::Ui, w: &mut Workspace, side: usize, response: &egui::Response, rect: Rect) {
    let wheel = if response.hovered() {
        ui.input_mut(|i| {
            let y = i.smooth_scroll_delta.y;
            i.smooth_scroll_delta = Vec2::ZERO;
            y
        })
    } else {
        0.
    };
    if response.is_pointer_button_down_on() || response.clicked() || response.dragged() || wheel != 0. {
        w.focused = side;
    }
    if wheel != 0. {
        let v = &mut w.views[side];
        let old = v.zoom;
        v.zoom = (old * (wheel * if w.inverse_scroll { -0.003 } else { 0.003 }).exp()).clamp(1., 32.);
        if let Some(pos) = response.hover_pos() {
            let q = (pos - rect.min) / rect.size();
            let offset = (pos - response.rect.center()) / (rect.size() * (v.zoom / old));
            v.center = [(q.x - offset.x).clamp(0., 1.), (q.y - offset.y).clamp(0., 1.)];
        }
        sync_views(w, side);
    }
    if response.dragged_by(egui::PointerButton::Secondary) {
        let d = ui.input(|i| i.pointer.delta());
        w.views[side].center[0] = (w.views[side].center[0] - d.x / rect.width()).clamp(0., 1.);
        w.views[side].center[1] = (w.views[side].center[1] - d.y / rect.height()).clamp(0., 1.);
        sync_views(w, side);
    }
    if w.focused == side {
        ui.painter().rect_stroke(response.rect, 0., egui::Stroke::new(1.5, Color32::from_rgb(65, 145, 235)), egui::StrokeKind::Inside);
    }
}
fn set_panel(app: &mut LightcraftApp, w: &mut Workspace, p: &mut Project, ui: &mut egui::Ui) -> bool {
    let changed = ui.text_edit_singleline(&mut p.name).changed();
    ui.small("Click a viewport, then choose its photo below.");
    let active = match w.focused {
        0 => 0,
        1 => w.target,
        _ => w.result_target,
    };
    egui::ScrollArea::vertical().id_salt("alignment-members").max_height(180.).show(ui, |ui| {
        for (i, m) in p.members.iter().enumerate() {
            if ui.selectable_label(active == i, format!("{}{}", if i == 0 { "Reference • " } else { "" }, m.name)).clicked() {
                if w.focused == 0 && i != 0 {
                    if execute(app, w, "alignment.reference", json!({"index":i})) {
                        reset_previews(app, w);
                        w.message = "Reference changed. Landmarks remapped; review the fit and shared crop.".into();
                    }
                } else if i > 0 {
                    if w.focused == 2 {
                        w.result_target = i;
                    } else {
                        let follow = w.result_target == w.target;
                        w.target = i;
                        if follow {
                            w.result_target = i;
                        }
                    }
                    w.pending = None;
                    w.selected = None;
                    w.review_key.clear();
                } else if w.focused == 2 {
                    w.result_target = 0;
                    w.review_key.clear();
                } else if w.focused == 1 && execute(app, w, "alignment.reference", json!({"index":w.target})) {
                    let old = w.target;
                    reset_previews(app, w);
                    w.target = old;
                    w.result_target = old;
                }
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        if ui.button("Add photos…").clicked() {
            w.add_open = true;
        }
        if ui.add_enabled(p.members.len() > 2, egui::Button::new("Remove selected photo")).clicked()
            && execute(app, w, "alignment.remove", json!({"index":active}))
        {
            reset_previews(app, w);
        }
    });
    changed
}
fn add_dialog(app: &mut LightcraftApp, w: &mut Workspace, ctx: &egui::Context) {
    if !w.add_open {
        return;
    }
    let mut open = true;
    egui::Window::new("Add photos to alignment").id(egui::Id::unique("alignment-add")).open(&mut open).default_size([800., 600.]).show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label("Search:");
            ui.text_edit_singleline(&mut w.add_search);
            if ui.button("Import new photos…").clicked() {
                let _ = app.run("file.addPhotos", json!({}));
            }
        });
        let search = w.add_search.to_lowercase();
        let photos: Vec<_> = app
            .session
            .catalog
            .photos()
            .filter(|p| !p.deleted && !p.local && p.file_name.to_lowercase().contains(&search))
            .map(|p| (p.id, p.file_name.clone()))
            .collect();
        let existing: Vec<_> = app.session.alignment.project.as_ref().map(|p| p.members.iter().map(|m| m.photo).collect()).unwrap_or_default();
        let columns = ((ui.available_width() / 150.).floor() as usize).max(1);
        egui::ScrollArea::vertical().id_salt("alignment-add-grid").max_height(440.).show_rows(
            ui,
            145.,
            photos.len().div_ceil(columns),
            |ui, rows| {
                for row in rows {
                    ui.horizontal(|ui| {
                        for (id, name) in photos.iter().skip(row * columns).take(columns) {
                            ui.vertical(|ui| {
                                ui.set_width(140.);
                                crate::panels::grid::request_thumb(app, *id, 256, 10);
                                let (rect, response) = ui.allocate_exact_size(Vec2::new(140., 105.), egui::Sense::click());
                                ui.painter().rect_filled(rect, 2., Color32::from_gray(25));
                                if let Some(tex) = app.renderer.thumb(*id) {
                                    let fit = crate::panels::detail::fit_texture_rect(rect, tex.size);
                                    ui.painter().image(tex.tex.id(), fit, Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)), Color32::WHITE);
                                }
                                let in_set = existing.contains(id);
                                if in_set || w.add_selected.contains(&id.0) {
                                    ui.painter().rect_stroke(rect, 2., egui::Stroke::new(2., Color32::LIGHT_BLUE), egui::StrokeKind::Inside);
                                }
                                if response.clicked() && !in_set && !w.add_selected.remove(&id.0) {
                                    w.add_selected.insert(id.0);
                                }
                                ui.add(egui::Label::new(name).truncate()).on_hover_text(name);
                                if in_set {
                                    ui.small("Already in set");
                                }
                            });
                        }
                    });
                }
            },
        );
        if ui.add_enabled(!w.add_selected.is_empty(), egui::Button::new(format!("Add {} selected", w.add_selected.len()))).clicked()
            && execute(app, w, "alignment.add", json!({"ids":w.add_selected.iter().copied().collect::<Vec<_>>()}))
        {
            w.add_selected.clear();
            w.add_open = false;
            reset_previews(app, w);
        }
    });
    w.add_open &= open;
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
        ui.add(egui::TextEdit::singleline(&mut w.directory).desired_width(ui.available_width().min(260.)).hint_text("C:\\Photos\\Aligned"));
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
            changed = true;
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
    let t = match p.transform(w.result_target) {
        Ok(t) => t,
        Err(e) => {
            let (area, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
            ui.painter().rect_filled(area, 0., Color32::from_gray(20));
            ui.painter().text(area.center(), egui::Align2::CENTER_CENTER, e, egui::FontId::proportional(14.), Color32::LIGHT_GRAY);
            navigate(ui, w, 2, &response, area);
            crate::widgets::register(ui, "alignment.result", area);
            return;
        }
    };
    let (Some(reference), Some(target)) = (w.images.first(), w.images.get(w.result_target)) else {
        ui.label("Refresh previews to compare.");
        return;
    };

    let Some(r) = p.members.first() else { return };
    let key = format!("{}:{}:{}", w.result_target, w.mode, serde_json::to_string(p).unwrap_or_default());
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
    let Some(base) = w.textures.first() else { return };
    let (response, rect) = canvas(ui, base, size, w.views[2].zoom, w.views[2].center);
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
    navigate(ui, w, 2, &response, rect);
    crate::widgets::register(ui, "alignment.result", response.rect);
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
    h.app.alignment.review_key.clear();
    h.app.alignment.images = vec![im.clone(), im];
    h.app.alignment.textures =
        h.app.alignment.images.iter().enumerate().map(|(i, im)| texture(&h.view.ctx, &format!("alignment-test-{i}"), im)).collect();
    if let Some(p) = &h.app.session.alignment.project {
        h.app.alignment.preview_settings = settings(&h.app, p);
    }
}

#[cfg(test)]
pub(crate) fn test_cancel_close(h: &mut crate::headless::Headless) {
    assert!(matches!(h.app.alignment.close_action, Some(AfterSave::Quit)));
    h.settle(std::time::Duration::from_secs(10));
    let rect = h.app.widgets.iter().find(|(id, _)| id == "alignment.close.cancel").map(|(_, r)| *r).unwrap();
    h.app.synthetic.push(egui::Event::PointerMoved(rect.center()));
    for pressed in [true, false] {
        h.app.synthetic.push(egui::Event::PointerButton {
            pos: rect.center(),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        });
    }
    h.settle(std::time::Duration::from_secs(10));
    assert!(h.app.alignment.close_action.is_none());
    assert!(h.app.alignment.open && h.app.session.alignment.dirty);
}

#[cfg(test)]
mod lifecycle_tests {
    use super::{AfterSave, View, Workspace, perform_action, request_action, save, sync_views};
    use crate::{LightcraftApp, Services};
    use lightcraft_engine::{Session, alignment::Project};
    use serde_json::json;
    #[test]
    fn alignment_close_cancel_save_and_discard() {
        let mut s = Session::with_demo();
        let ids = s.catalog.photos().take(2).map(|p| p.id).collect::<Vec<_>>();
        s.alignment.replace(Project::create(&s, &ids, "Close test".into()).unwrap());
        let dir = std::env::temp_dir().join(format!("lc-ui-close-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.lcalign.json");
        let mut app = LightcraftApp::new(s, Services::default());
        let mut w = Workspace { open: true, path: path.to_string_lossy().into(), ..Default::default() };
        request_action(&mut app, &mut w, AfterSave::Close);
        assert!(w.open && w.close_action.is_some());
        w.close_action = None; // Cancel preserves edits and window.
        assert!(app.session.alignment.dirty && w.open);
        request_action(&mut app, &mut w, AfterSave::Close);
        assert!(save(&mut app, &mut w));
        perform_action(&mut app, &mut w);
        assert!(!w.open && path.exists() && !app.session.alignment.dirty);
        let mut p = app.session.alignment.project.clone().unwrap();
        p.name = "Discard me".into();
        app.session.execute("alignment.replace", &json!({"project":p})).unwrap();
        w.open = true;
        request_action(&mut app, &mut w, AfterSave::Quit);
        app.session.execute("alignment.discard", &json!({})).unwrap();
        perform_action(&mut app, &mut w);
        assert!(!w.open);
        assert_eq!(Project::load(path.to_str().unwrap()).unwrap().name, "Close test");
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn alignment_save_as_cancel_and_completion() {
        let mut s = Session::with_demo();
        let ids = s.catalog.photos().take(2).map(|p| p.id).collect::<Vec<_>>();
        s.alignment.replace(Project::create(&s, &ids, "Save as".into()).unwrap());
        let mut app = LightcraftApp::new(s, Services::default());
        let mut w = Workspace { open: true, close_action: Some(AfterSave::Close), ..Default::default() };
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        w.pick = Some((super::PickAction::Save, rx));
        tx.send(vec![]).unwrap();
        super::poll(&mut app, &mut w, &ctx);
        assert!(w.open && w.close_action.is_some() && app.session.alignment.dirty);
        let dir = std::env::temp_dir().join(format!("lc-ui-saveas-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("chosen");
        let (tx, rx) = std::sync::mpsc::channel();
        w.pick = Some((super::PickAction::Save, rx));
        tx.send(vec![path.to_string_lossy().into()]).unwrap();
        super::poll(&mut app, &mut w, &ctx);
        assert!(!w.open && w.close_action.is_none() && !app.session.alignment.dirty);
        assert!(dir.join("chosen.lcalign.json").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn alignment_view_locks_are_independent() {
        let mut w = Workspace { linked: true, ..Default::default() };
        w.views[0] = View { zoom: 3., center: [0.4, 0.6] };
        sync_views(&mut w, 0);
        assert_eq!(w.views[1].zoom, 3.);
        assert_eq!(w.views[2].zoom, 1.);
        w.linked = false;
        w.views[1].zoom = 5.;
        sync_views(&mut w, 1);
        assert_eq!(w.views[0].zoom, 3.);
        w.result_linked = true;
        w.views[2].zoom = 7.;
        sync_views(&mut w, 2);
        assert_eq!(w.views[0].zoom, 7.);
        assert_eq!(w.views[1].zoom, 7.);
    }
    #[test]
    fn alignment_open_loads_selection() {
        let mut s = Session::with_demo();
        let ids = s.catalog.photos().take(3).map(|p| p.id.0).collect::<Vec<_>>();
        s.execute("library.select", &json!({"ids":ids,"active":ids[1]})).unwrap();
        let mut app = LightcraftApp::new(s, Services::default());
        super::open(&mut app).unwrap();
        let p = app.session.alignment.project.as_ref().unwrap();
        assert_eq!(p.members.len(), 3);
        assert_eq!(p.members[0].photo.0, ids[1]);
        assert!(app.alignment.open && app.alignment.needs_load);
    }
}

#[cfg(test)]
pub(crate) fn test_wheel(h: &mut crate::headless::Headless) {
    fn wheel(h: &mut crate::headless::Headless, name: &str) {
        let rect = h.app.widgets.iter().find(|(id, _)| id == name).map(|(_, r)| *r).unwrap();
        h.app.synthetic.push(egui::Event::PointerMoved(rect.center()));
        h.app.synthetic.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0., 100.),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
            modifiers: egui::Modifiers::NONE,
        });
        for _ in 0..12 {
            h.step();
        }
    }
    h.app.alignment.views = [View::default(); 3];
    h.app.alignment.linked = true;
    h.app.alignment.result_linked = false;
    wheel(h, "alignment.reference");
    assert!(h.app.alignment.views[0].zoom > 1.);
    assert_eq!(h.app.alignment.views[0].zoom, h.app.alignment.views[1].zoom);
    assert_eq!(h.app.alignment.views[2].zoom, 1.);
    h.app.alignment.linked = false;
    let left = h.app.alignment.views[0].zoom;
    wheel(h, "alignment.target");
    assert_eq!(h.app.alignment.views[0].zoom, left);
    assert!(h.app.alignment.views[1].zoom > left);
    h.app.alignment.inverse_scroll = true;
    let before = h.app.alignment.views[1].zoom;
    wheel(h, "alignment.target");
    assert!(h.app.alignment.views[1].zoom < before);
    assert_eq!(h.app.alignment.focused, 1);
}
