use super::Board;

impl Board {
    /// Drain pending context events from agent panels and route them to the
    /// appropriate workspace context.
    ///
    /// Follows the same collect-then-process pattern as `update_attention()` to
    /// avoid borrow conflicts (we need `&mut panel` to drain events, then
    /// `&mut workspace` to publish them).
    pub(super) fn process_context_events(&mut self) {
        let events: Vec<_> = self
            .panels
            .iter_mut()
            .filter(|p| p.had_recent_output && p.kind.is_agent())
            .flat_map(|p| {
                let ws_id = p.workspace_id;
                let panel_id = p.id;
                p.content
                    .terminal_mut()
                    .map(crate::terminal::Terminal::take_context_events)
                    .unwrap_or_default()
                    .into_iter()
                    .map(move |e| (ws_id, panel_id, e))
            })
            .collect();

        for (ws_id, panel_id, event) in events {
            if let Some(ws) = self.workspace_mut(ws_id) {
                ws.context.publish(event.key, event.value, Some(panel_id));
            }
        }
    }
}
