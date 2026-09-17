use super::*;
use std::cell::Cell;
use std::rc::Rc;

fn geometry() -> Geometry {
    Geometry {
        target_id: "fixture".into(),
        surface_id: "view".into(),
        width: 1280,
        height: 800,
        revision: "one".into(),
    }
}
struct Fake {
    actions: Rc<Cell<u32>>,
}
impl Backend for Fake {
    fn doctor(&self) -> Result<Readiness> {
        Err(DeviceError::Unsupported("fixture".into()))
    }
    fn screenshot(&self) -> Result<Observation> {
        Err(DeviceError::Unsupported("fixture".into()))
    }
    fn act(&mut self, request: &ActRequest) -> Result<ActionReceipt> {
        self.actions.set(self.actions.get() + 1);
        Ok(ActionReceipt {
            state: "dispatched".into(),
            geometry: request.geometry.clone(),
        })
    }
}
#[test]
fn invalid_inputs_never_reach_backend() {
    let calls = Rc::new(Cell::new(0));
    let mut device = Device {
        backend: Box::new(Fake { actions: calls.clone() }),
    };
    for action in [
        Action::Click {
            at: Point { x: -1, y: 0 },
            button: Button::Left,
        },
        Action::Click {
            at: Point { x: 1280, y: 0 },
            button: Button::Left,
        },
        Action::Drag {
            from: Point { x: 1, y: 1 },
            to: Point { x: 0, y: 800 },
            duration_ms: 100,
        },
        Action::Drag {
            from: Point { x: 1, y: 1 },
            to: Point { x: 2, y: 2 },
            duration_ms: 2001,
        },
        Action::Scroll {
            at: Point { x: 0, y: 0 },
            vertical_notches: i32::MIN,
            horizontal_notches: 0,
        },
        Action::Type { text: "a\0b".into() },
        Action::Type { text: "a".repeat(4097) },
        Action::Type { text: "a".repeat(257) },
        Action::Type {
            text: "🦀".repeat(257)
        },
        Action::Key {
            key: Key::Enter,
            modifiers: vec![Modifier::Shift; 5],
        },
    ] {
        assert!(matches!(
            device.act(&ActRequest {
                geometry: geometry(),
                action
            }),
            Err(DeviceError::Invalid(_))
        ));
    }
    assert_eq!(calls.get(), 0);
}
#[test]
fn unicode_is_text_not_a_keyboard_layout_assumption() {
    let calls = Rc::new(Cell::new(0));
    let mut device = Device {
        backend: Box::new(Fake { actions: calls.clone() }),
    };
    assert!(
        device
            .act(&ActRequest {
                geometry: geometry(),
                action: Action::Type {
                    text: "æøå🦀".repeat(64)
                }
            })
            .is_ok()
    );
    assert_eq!(calls.get(), 1);
}
#[test]
fn transport_rejects_unknown_fields_and_actions() {
    assert!(
        serde_json::from_str::<Action>(r#"{"kind":"click","at":{"x":1,"y":2},"button":"left","repeat":20}"#).is_err()
    );
    assert!(serde_json::from_str::<Action>(r#"{"kind":"tap","at":{"x":1,"y":2}}"#).is_err());
}
#[test]
#[cfg(target_os = "linux")]
fn only_local_display_endpoints_are_accepted() {
    for display in ["", "localhost:0", ":", ":0.", ":0.0.1", "unix:0"] {
        let target = Target {
            id: "fixture".into(),
            endpoint: Endpoint::LocalX11 {
                display: display.into(),
            },
        };
        assert!(matches!(Device::connect(&target), Err(DeviceError::Invalid(_))));
    }
}
#[test]
fn model_can_describe_mobile_surface_without_pid_or_window_handle() -> std::result::Result<(), serde_json::Error> {
    let geometry = Geometry {
        target_id: "ipad-lab".into(),
        surface_id: "application".into(),
        width: 1024,
        height: 768,
        revision: "landscape-2".into(),
    };
    let request = ActRequest {
        geometry,
        action: Action::Type { text: "hello".into() },
    };
    let encoded = serde_json::to_string(&request)?;
    let decoded: ActRequest = serde_json::from_str(&encoded)?;
    assert_eq!(decoded.geometry.target_id, "ipad-lab");
    Ok(())
}
