use egui::{Context, Event, Id, Order, PointerButton, Pos2, Rect, Vec2};

#[derive(Clone, Copy)]
struct CanvasDrag {
    origin: Pos2,
    position: Pos2,
    dragging: bool,
}

impl CanvasDrag {
    fn move_to(&mut self, position: Pos2, max_click_dist: f32) -> Option<Vec2> {
        let movement = if self.dragging {
            Some(position - self.position)
        } else if position.distance(self.origin) > max_click_dist {
            self.dragging = true;
            Some(position - self.origin)
        } else {
            None
        };
        self.position = position;
        movement
    }
}

pub(super) fn canvas_drag_delta(
    ctx: &Context,
    canvas_rect: Rect,
    panels: &(impl Iterator<Item = Rect> + Clone),
    events: &[Event],
) -> Option<Vec2> {
    let id = Id::new(("canvas_primary_drag", ctx.viewport_id()));
    let mut drag = ctx.data(|data| data.get_temp::<CanvasDrag>(id));
    let max_click_dist = ctx.options(|options| options.input_options.max_click_dist);
    let mut movement = None;
    if ctx.input(|input| input.focused) {
        for event in events {
            let delta = match event {
                Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers,
                } => {
                    if *pressed {
                        let empty_canvas = modifiers.is_none()
                            && canvas_rect.contains(*pos)
                            && !panels.clone().any(|rect| rect.contains(*pos))
                            && ctx
                                .layer_id_at(*pos)
                                .is_none_or(|layer| layer.order == Order::Background);
                        drag = empty_canvas.then_some(CanvasDrag {
                            origin: *pos,
                            position: *pos,
                            dragging: false,
                        });
                        None
                    } else {
                        drag.take().and_then(|mut drag| drag.move_to(*pos, max_click_dist))
                    }
                }
                Event::PointerMoved(position) => drag.as_mut().and_then(|drag| drag.move_to(*position, max_click_dist)),
                Event::ModifiersChanged(modifiers) if !modifiers.is_none() => {
                    drag = None;
                    None
                }
                _ => None,
            };
            if let Some(delta) = delta {
                *movement.get_or_insert(Vec2::ZERO) += delta;
            }
        }
    }
    if ctx.input(|input| !input.focused || !input.pointer.primary_down() || !input.modifiers.is_none()) {
        drag = None;
    }
    if drag.is_some_and(|drag| drag.dragging) {
        movement.get_or_insert(Vec2::ZERO);
    }
    ctx.data_mut(|data| {
        if let Some(drag) = drag {
            data.insert_temp(id, drag);
        } else {
            data.remove::<CanvasDrag>(id);
        }
    });
    movement
}

#[cfg(test)]
mod tests;
