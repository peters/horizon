//! The clipboard read the toolkit answers on a later pass.
//!
//! `ViewportCommand::RequestPaste` is not a clipboard read. The integration
//! answers it at the end of the pass that carries the command and hands the
//! clipboard event to a later pass. An idle window runs no pass by itself, so a
//! paste would wait for unrelated input or panel output. This module asks for
//! the passes the request still needs.
const FRAMES_KEY: &str = "terminal_clipboard_paste_frames";
/// Passes the request and its clipboard event need, with one spare for a pass
/// that arrives before the integration drained the command.
const FRAMES: u8 = 6;

fn frames_id() -> egui::Id {
    egui::Id::unique(FRAMES_KEY)
}

/// Ask the toolkit to read the clipboard for the focused terminal.
pub(crate) fn request(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
    ctx.data_mut(|data| data.insert_temp(frames_id(), FRAMES));
    ctx.request_repaint();
}

/// Keep painting while a clipboard request waits for its event.
pub(crate) fn keep_frames(ctx: &egui::Context) {
    let id = frames_id();
    let remaining: u8 = ctx.data(|data| data.get_temp(id).unwrap_or(0));
    if remaining == 0 {
        return;
    }
    ctx.data_mut(|data| data.insert_temp(id, remaining - 1));
    ctx.request_repaint();
}

/// The frames a pending paste request still asks for.
#[cfg(test)]
pub(crate) fn pending_frames(ctx: &egui::Context) -> u8 {
    ctx.data(|data| data.get_temp(frames_id()).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::{FRAMES, keep_frames, pending_frames, request};

    #[test]
    fn a_paste_request_paints_until_its_frames_are_spent() {
        let ctx = egui::Context::default();
        assert_eq!(pending_frames(&ctx), 0);
        request(&ctx);
        assert_eq!(pending_frames(&ctx), FRAMES);

        for expected in (0..FRAMES).rev() {
            keep_frames(&ctx);
            assert_eq!(pending_frames(&ctx), expected);
        }
        keep_frames(&ctx);
        assert_eq!(pending_frames(&ctx), 0, "the pump stops on its own");
    }

    #[test]
    fn an_idle_window_asks_for_no_paste_frames() {
        let ctx = egui::Context::default();
        keep_frames(&ctx);
        assert_eq!(pending_frames(&ctx), 0, "nothing is pending");
    }
}
