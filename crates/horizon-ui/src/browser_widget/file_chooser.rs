use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use horizon_core::browser::file_chooser::{
    DirectoryListing, FileChooserAnswer, FileChooserHandle, FileChooserRequest, read_directory,
};

pub(super) struct FilePicker {
    id: u64,
    directory: PathBuf,
    path: String,
    listing: Option<DirectoryListing>,
    loading: Option<Receiver<Result<DirectoryListing, String>>>,
    selected: Vec<PathBuf>,
    error: Option<String>,
    submitted: bool,
}

impl FilePicker {
    fn new(request: &FileChooserRequest, ctx: &egui::Context) -> Self {
        let directory = horizon_core::user_home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let mut picker = Self {
            id: request.id,
            path: directory.display().to_string(),
            directory,
            listing: None,
            loading: None,
            selected: Vec::new(),
            error: None,
            submitted: false,
        };
        picker.load(ctx);
        picker
    }

    fn load(&mut self, ctx: &egui::Context) {
        self.path = self.directory.display().to_string();
        self.listing = None;
        self.selected.clear();
        self.error = None;
        let ctx = ctx.clone();
        self.loading = Some(read_directory(self.directory.clone(), move || ctx.request_repaint()));
    }

    fn content(&mut self, ui: &mut egui::Ui, request: &FileChooserRequest) -> Option<FileChooserAnswer> {
        ui.set_width(520.0_f32.min(ui.ctx().content_rect().width() - 40.0));
        ui.heading(if request.multiple {
            "Choose files"
        } else {
            "Choose a file"
        });
        ui.label(format!("Upload to {}", request.origin));
        if !request.accept.is_empty() {
            let accept: String = request.accept.chars().take(160).collect();
            ui.label(format!("Accepted types: {accept}"));
        }
        if let Some(result) = self.loading.as_ref().and_then(|loading| loading.try_recv().ok()) {
            self.loading = None;
            match result {
                Ok(listing) => self.listing = Some(listing),
                Err(error) => self.error = Some(error),
            }
        }
        let mut navigate = None;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.directory.parent().is_some(), egui::Button::new("Up"))
                .clicked()
            {
                navigate = self.directory.parent().map(std::path::Path::to_path_buf);
            }
            let field = ui.text_edit_singleline(&mut self.path);
            if ui.button("Go").clicked()
                || (field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)))
            {
                navigate = Some(PathBuf::from(self.path.trim()));
            }
        });
        if self.loading.is_some() {
            ui.spinner();
        }
        let list_height = (ui.ctx().content_rect().height() * 0.4).clamp(80.0, 260.0);
        egui::ScrollArea::vertical().max_height(list_height).show(ui, |ui| {
            if let Some(listing) = &self.listing {
                for entry in &listing.entries {
                    let name = entry.path.file_name().unwrap_or_default().to_string_lossy();
                    if entry.directory {
                        if ui.button(format!("{name}/")).clicked() {
                            navigate = Some(entry.path.clone());
                        }
                    } else {
                        let selected = self.selected.contains(&entry.path);
                        if ui.selectable_label(selected, name).clicked() {
                            if selected {
                                self.selected.retain(|path| path != &entry.path);
                            } else {
                                if !request.multiple {
                                    self.selected.clear();
                                }
                                self.selected.push(entry.path.clone());
                            }
                        }
                    }
                }
                if listing.truncated {
                    ui.label("Directory has more than 2,000 entries. Enter a narrower directory.");
                }
            }
        });
        if let Some(path) = navigate {
            self.directory = path;
            self.load(ui.ctx());
        }
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        ui.label(format!("{} selected", self.selected.len()));
        ui.horizontal(|ui| {
            if ui.button("Cancel").clicked() {
                return Some(FileChooserAnswer::Cancel);
            }
            if ui
                .add_enabled(!self.selected.is_empty(), egui::Button::new("Open"))
                .clicked()
            {
                return Some(FileChooserAnswer::Files(self.selected.clone()));
            }
            None
        })
        .inner
    }
}

pub(super) fn show(
    ctx: &egui::Context,
    panel: egui::Id,
    handle: &FileChooserHandle,
    state: &mut Option<FilePicker>,
) -> bool {
    let Some(request) = handle.request() else {
        *state = None;
        return false;
    };
    let picker = state.get_or_insert_with(|| FilePicker::new(&request, ctx));
    if picker.id != request.id {
        *picker = FilePicker::new(&request, ctx);
    }
    if let Some(error) = handle.take_error() {
        picker.error = Some(error);
        picker.submitted = false;
    }
    let id = panel.with("file-chooser");
    let response = egui::Modal::new(id)
        .area(egui::Modal::default_area(id).order(egui::Order::Tooltip))
        .show(ctx, |ui| {
            ui.add_enabled_ui(!picker.submitted, |ui| picker.content(ui, &request))
                .inner
        });
    ctx.move_to_top(response.response.layer_id);
    let escape = ctx.input(|input| input.key_pressed(egui::Key::Escape));
    let close = response.should_close();
    let answer = response.inner.or_else(|| close.then_some(FileChooserAnswer::Cancel));
    let cancelled_by_escape = !picker.submitted && answer.is_some() && escape;
    if !picker.submitted
        && let Some(answer) = answer
    {
        picker.submitted = handle.respond(request.id, answer);
    }
    cancelled_by_escape
}
