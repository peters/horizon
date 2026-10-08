use super::HorizonApp;

impl HorizonApp {
    /// Replaced by the Dependencies panel; see feat/dependencies-panel.
    #[expect(
        clippy::unused_self,
        reason = "the stub keeps the panel's receiver so call sites stay unchanged"
    )]
    pub(super) fn open_dependencies_panel(&mut self, _ctx: &egui::Context) {}
}
