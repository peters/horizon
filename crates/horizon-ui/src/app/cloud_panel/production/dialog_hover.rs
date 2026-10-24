//! Hover text of a control inside a cloud dialog.
//!
//! A cloud dialog shows on the tooltip order and is raised with `move_to_top`, because the
//! board and the cloud panel hold that order too. `egui` draws a tooltip on that same order,
//! so a dialog raised over its neighbours is also raised over its own hover text, and the
//! text stays invisible. Naming the tooltip of a control as a sublayer of the layer that
//! holds the control puts the hover text directly over that surface again.

use egui::{Context, Id, LayerId, Order, Response, WidgetText};

/// The hover text of `response`, drawn over the dialog that holds it.
pub(super) fn text(response: Response, text: impl Into<WidgetText>) -> Response {
    raise_over_surface(&response);
    response.on_hover_text(text)
}

/// The hover text of a disabled `response`, drawn over the dialog that holds it.
pub(super) fn disabled_text(response: Response, text: impl Into<WidgetText>) -> Response {
    raise_over_surface(&response);
    response.on_disabled_hover_text(text)
}

/// Raise the tooltip `response` opens directly over the layer that holds the widget.
///
/// The widget must sit in a layer on the tooltip order, as the contents of a cloud dialog
/// do: `egui` keeps a sublayer inside the order of its parent. A widget on any other order
/// draws under its tooltips already, so it keeps the default placement.
fn raise_over_surface(response: &Response) {
    if response.layer_id.order != Order::Tooltip {
        return;
    }
    let tooltip = tooltip_layer(&response.ctx, response.id);
    response.ctx.set_sublayer(response.layer_id, tooltip);
}

/// The layer `egui` shows the next tooltip of `widget` in.
///
/// `egui` gives the tooltip of a widget the id of the widget with the count of tooltips the
/// widget showed in this pass, which is zero until the first one shows.
fn tooltip_layer(ctx: &Context, widget: Id) -> LayerId {
    LayerId::new(Order::Tooltip, egui::Tooltip::next_tooltip_id(ctx, widget))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_egui::DiscardTextures;
    use egui::{Event, FullOutput, Id, Pos2, RawInput, Rect, Shape};

    const BODY: &str = "The wording of the dialog";
    const HOVER: &str = "What the control does";

    /// Where in the paint order the text `text` is drawn, the last shape being the top one.
    fn paint_position(output: &FullOutput, text: &str) -> Option<usize> {
        output
            .shapes
            .iter()
            .position(|shape| matches!(&shape.shape, Shape::Text(text_shape) if text_shape.galley.job.text == text))
    }

    fn input(pointer: Option<Pos2>) -> RawInput {
        let mut input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))),
            ..RawInput::default()
        };
        if let Some(pointer) = pointer {
            input.events.push(Event::PointerMoved(pointer));
        }
        input
    }

    /// A dialog on the tooltip order, raised like the cloud dialogs are, with one control
    /// whose hover text goes through this module. Returns the center of that control.
    fn dialog_frame(ctx: &Context, pointer: Option<Pos2>) -> (FullOutput, Pos2) {
        let mut control = Pos2::ZERO;
        let output = ctx
            .run_ui(input(pointer), |ui| {
                let id = Id::unique("dialog_hover_test");
                let dialog = egui::Modal::new(id)
                    .area(egui::Modal::default_area(id).order(Order::Tooltip))
                    .show(ui.ctx(), |ui| {
                        ui.label(BODY);
                        control = text(ui.button("Control"), HOVER).rect.center();
                    });
                ui.ctx().move_to_top(dialog.response.layer_id);
            })
            .discard_textures();
        (output, control)
    }

    #[test]
    fn the_hover_text_of_a_raised_dialog_control_is_drawn_over_the_dialog() {
        let ctx = Context::default();
        ctx.all_styles_mut(|style| style.interaction.tooltip_delay = 0.0);
        // The dialog takes its place on the first frame; the pointer reaches it after that,
        // and a tooltip shows on the frame after the one that reports the hover.
        let (_, mut center) = dialog_frame(&ctx, None);
        let mut output = None;
        for _ in 0..4 {
            let (frame, placed) = dialog_frame(&ctx, Some(center));
            center = placed;
            output = Some(frame);
        }
        let output = output.expect("a rendered dialog");

        let body = paint_position(&output, BODY).expect("the dialog body");
        let hover = paint_position(&output, HOVER).expect("the hover text of the control");
        assert!(
            hover > body,
            "the hover text draws over the dialog: hover at {hover}, body at {body}"
        );
    }
}
