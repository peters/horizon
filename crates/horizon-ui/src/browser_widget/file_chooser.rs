use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

use crate::dir_picker::{PickerEmptyState, PickerModalAction, PickerModalConfig, PickerModalState};
use crate::theme;
use egui::{Align2, FontId, RichText, Sense, Stroke, StrokeKind};
use horizon_core::browser::file_chooser::{
    DirectoryEntry, DirectoryListing, FileChooserAnswer, FileChooserHandle, FileChooserRequest, FilePickerState,
    read_directory,
};

pub(super) struct FilePicker {
    id: u64,
    modal: PickerModalState,
    files: FilePickerState,
    listing: Option<DirectoryListing>,
    loading: Option<Receiver<Result<DirectoryListing, String>>>,
    error: Option<String>,
    submitted: bool,
}

impl FilePicker {
    fn new(request: &FileChooserRequest, ctx: &egui::Context) -> Self {
        let directory = horizon_core::user_home_dir().unwrap_or_else(|| PathBuf::from(std::path::MAIN_SEPARATOR_STR));
        let mut picker = Self {
            id: request.id,
            modal: PickerModalState::new(directory_query(&directory)),
            files: FilePickerState::new(directory, request.multiple, request.accept.clone()),
            listing: None,
            loading: None,
            error: None,
            submitted: false,
        };
        picker.load(ctx);
        picker
    }

    fn load(&mut self, ctx: &egui::Context) {
        self.listing = None;
        self.error = None;
        let ctx = ctx.clone();
        self.loading = Some(read_directory(self.files.directory.clone(), move || {
            ctx.request_repaint();
        }));
    }

    fn navigate(&mut self, path: &Path) {
        self.modal.set_query(directory_query(path));
    }

    fn update(&mut self, ctx: &egui::Context) -> String {
        let (directory, filter) = self.files.query_location(self.modal.query());
        if directory != self.files.directory {
            self.files.directory = directory;
            self.load(ctx);
        }
        if let Some(result) = self.loading.as_ref().and_then(|loading| loading.try_recv().ok()) {
            self.loading = None;
            match result {
                Ok(listing) => self.listing = Some(listing),
                Err(error) => self.error = Some(error),
            }
        }
        filter
    }

    fn show(&mut self, ctx: &egui::Context, request: &FileChooserRequest, id: &str) -> Option<FileChooserAnswer> {
        let filter = self.update(ctx);
        if !self.submitted {
            let dropped = ctx.input_mut(|input| std::mem::take(&mut input.raw.dropped_files));
            let paths: Vec<_> = dropped.iter().map(|file| file.path().to_path_buf()).collect();
            if !paths.is_empty() {
                self.error = self.files.select_dropped(&paths).err().map(|error| error.to_string());
            }
        }
        let results = self
            .listing
            .as_ref()
            .map(|listing| self.files.visible_entries(listing, &filter))
            .unwrap_or_default();
        self.modal.clamp_selected(results.len());
        let status = if request.accept.is_empty() || request.accept == "*/*" {
            format!("Upload to {}", request.origin)
        } else {
            format!(
                "{}  •  {}",
                request.origin,
                request.accept.chars().take(80).collect::<String>()
            )
        };
        let empty = if self.loading.is_some() {
            "Loading files…"
        } else {
            "No matching files. Try another path or name."
        };

        let selected = &self.files.selected;
        let mut footer = FilePickerFooter {
            selected,
            hidden: &mut self.files.show_hidden,
            has_parent: self.files.directory.parent().is_some(),
            submitted: self.submitted,
            error: self.error.as_deref(),
            truncated: self.listing.as_ref().is_some_and(|listing| listing.truncated),
            up: false,
            cancel: false,
        };
        let action = self.modal.show_with_footer(
            ctx,
            &PickerModalConfig {
                id_source: id,
                heading: if request.multiple {
                    "Choose files to upload"
                } else {
                    "Choose a file to upload"
                },
                hint_text: "Search files or enter a path…",
                status_text: Some(&status),
                empty_state: PickerEmptyState {
                    message: empty,
                    color: theme::FG_DIM(),
                },
                footer_action_label: None,
            },
            &results,
            |ui, width, _, entry, focused| {
                ui.add_enabled_ui(!self.submitted, |ui| {
                    file_row(ui, width, entry, focused, selected.contains(&entry.path))
                })
                .inner
            },
            |ui| render_footer(ui, &mut footer),
            110.0,
        );
        let up = footer.up;
        let cancel = footer.cancel;
        if self.submitted {
            return None;
        }
        if cancel || matches!(action, PickerModalAction::Cancelled) {
            return Some(FileChooserAnswer::Cancel);
        }
        if up {
            if let Some(parent) = self.files.directory.parent().map(Path::to_path_buf) {
                self.navigate(&parent);
            }
            return None;
        }
        let entry = match action {
            PickerModalAction::ClickedRow(index) => results.get(index),
            PickerModalAction::Submit | PickerModalAction::CompleteSelection => {
                results.get(self.modal.selected_index())
            }
            PickerModalAction::FooterAction => return Some(FileChooserAnswer::Files(self.files.selected.clone())),
            _ => None,
        };
        if let Some(entry) = entry {
            let path = entry.path.clone();
            let directory = entry.directory;
            if directory {
                self.navigate(&path);
            } else {
                self.error = self.files.toggle(&path).err().map(|error| error.to_string());
            }
        }
        None
    }
}

struct FilePickerFooter<'a> {
    selected: &'a [PathBuf],
    hidden: &'a mut bool,
    has_parent: bool,
    submitted: bool,
    error: Option<&'a str>,
    truncated: bool,
    up: bool,
    cancel: bool,
}

fn render_footer(ui: &mut egui::Ui, footer: &mut FilePickerFooter<'_>) -> PickerModalAction {
    ui.add_space(8.0);
    ui.separator();
    if let Some(error) = footer.error {
        ui.label(RichText::new(error).size(11.0).color(theme::PALETTE_RED()));
    } else if footer.truncated {
        ui.label(
            RichText::new("First 2,000 entries shown. Enter a narrower path.")
                .size(11.0)
                .color(theme::FG_DIM()),
        );
    }
    ui.add_enabled_ui(!footer.submitted, |ui| {
        ui.horizontal(|ui| {
            footer.up = ui
                .add_enabled(footer.has_parent, egui::Button::new("↑ Parent folder").frame(false))
                .clicked();
            ui.checkbox(footer.hidden, "Hidden files");
        });
    });
    let upload = ui
        .horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let count = footer.selected.len();
                let label = if footer.submitted {
                    "Uploading…".into()
                } else if count > 1 {
                    format!("Upload {count} files")
                } else {
                    "Upload".into()
                };
                let upload = ui
                    .add_enabled(
                        !footer.submitted && count > 0,
                        egui::Button::new(RichText::new(label).color(theme::BG()))
                            .fill(theme::ACCENT())
                            .min_size(egui::vec2(90.0, 30.0)),
                    )
                    .clicked();
                footer.cancel = ui
                    .add_enabled(!footer.submitted, egui::Button::new("Cancel").frame(false))
                    .clicked();
                let summary = if count == 0 {
                    "Select files or drop them here".into()
                } else if count == 1 {
                    footer.selected[0]
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                } else {
                    format!("{count} files selected")
                };
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width().max(0.0), 30.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.add(egui::Label::new(RichText::new(summary).size(12.0).color(theme::FG_SOFT())).truncate());
                    },
                );
                upload
            })
            .inner
        })
        .inner;

    if upload {
        PickerModalAction::FooterAction
    } else {
        PickerModalAction::None
    }
}

fn directory_query(path: &Path) -> String {
    let mut query = horizon_core::dir_search::abbreviate_home(path);
    if !query.ends_with(std::path::is_separator) {
        query.push(std::path::MAIN_SEPARATOR);
    }
    query
}

fn file_row(ui: &mut egui::Ui, width: f32, entry: &DirectoryEntry, focused: bool, selected: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 34.0), Sense::click());
    let painter = ui.painter_at(rect);
    if focused || selected || response.hovered() {
        let fill = if focused || selected {
            theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.22)
        } else {
            theme::PANEL_BG_ALT()
        };
        painter.rect_filled(rect, 8, fill);
    }
    let icon = egui::Rect::from_center_size(egui::pos2(rect.left() + 15.0, rect.center().y), egui::vec2(14.0, 12.0));
    let color = if entry.directory {
        theme::ACCENT()
    } else {
        theme::FG_DIM()
    };
    painter.rect_stroke(icon, 2, Stroke::new(1.2, color), StrokeKind::Inside);
    if entry.directory {
        painter.rect_filled(
            egui::Rect::from_min_size(icon.min - egui::vec2(0.0, 3.0), egui::vec2(7.0, 4.0)),
            1,
            color,
        );
    } else if selected {
        painter.text(
            icon.center(),
            Align2::CENTER_CENTER,
            "✓",
            FontId::proportional(12.0),
            theme::ACCENT(),
        );
    }
    let name = entry.path.file_name().unwrap_or_default().to_string_lossy();
    response.widget_info(|| {
        if entry.directory {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, response.enabled(), name.as_ref())
        } else {
            egui::WidgetInfo::selected(egui::WidgetType::Checkbox, response.enabled(), selected, name.as_ref())
        }
    });
    let name_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 32.0, rect.top()),
        egui::pos2(rect.right() - 78.0, rect.bottom()),
    );
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(name_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            ui.add(
                egui::Label::new(RichText::new(name).size(12.5).color(theme::FG()))
                    .truncate()
                    .selectable(false)
                    .sense(Sense::hover()),
            );
        },
    );
    let detail = if entry.directory {
        "Folder".into()
    } else {
        file_size(entry.size)
    };
    painter.text(
        egui::pos2(rect.right() - 8.0, rect.center().y),
        Align2::RIGHT_CENTER,
        detail,
        FontId::proportional(11.0),
        theme::FG_DIM(),
    );
    response.on_hover_text(entry.path.display().to_string()).clicked() && ui.is_enabled()
}

fn file_size(size: u64) -> String {
    if size < 1_024 {
        format!("{size} B")
    } else if size < 1_048_576 {
        format!("{} KB", size.div_ceil(1_024))
    } else {
        format!("{} MB", size.div_ceil(1_048_576))
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
    let answer = picker.show(ctx, &request, &format!("file-picker-{panel:?}"));
    let cancelled_by_escape =
        matches!(answer, Some(FileChooserAnswer::Cancel)) && ctx.input(|input| input.key_pressed(egui::Key::Escape));
    if let Some(answer) = answer {
        picker.submitted = handle.respond(request.id, answer);
    }
    cancelled_by_escape
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_egui::DiscardTextures;

    #[test]
    fn clicking_the_file_name_selects_its_row() {
        let ctx = egui::Context::default();
        let entry = DirectoryEntry {
            path: PathBuf::from("/fixture/Notes.txt"),
            directory: false,
            size: 24,
        };
        let mut clicked = false;
        let mut position = egui::Pos2::ZERO;
        for step in 0..4 {
            let mut events = Vec::new();
            if step >= 2 {
                events.push(egui::Event::PointerMoved(position));
                events.push(egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed: step == 2,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            let _ = ctx
                .run_ui(
                    egui::RawInput {
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        position = ui.cursor().min + egui::vec2(70.0, 17.0);
                        clicked |= file_row(ui, 480.0, &entry, false, false);
                    },
                )
                .discard_textures();
        }
        assert!(clicked, "the file name must not intercept its row's click");
    }
}
