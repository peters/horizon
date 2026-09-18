use crate::{
    ActRequest, Action, ActionReceipt, Backend, Capability, CaptureOptions, DeviceError, Endpoint, Geometry, Key,
    Modifier, Observation, Readiness, Result, Target,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use enigo::{Direction, Enigo, Keyboard, Mouse, Settings};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use x11rb::{
    connection::Connection,
    protocol::randr::ConnectionExt as _,
    protocol::xproto::{ConnectionExt, ImageFormat, ImageOrder},
    rust_connection::RustConnection,
};

pub struct X11 {
    target: Target,
    connection: RustConnection,
    root: u32,
    visual: u32,
    randr: bool,
}
fn unavailable(e: impl std::fmt::Display) -> DeviceError {
    DeviceError::Unavailable(e.to_string())
}
impl X11 {
    pub fn connect(target: &Target) -> Result<Self> {
        let Endpoint::LocalX11 { display } = &target.endpoint;
        let valid = display.strip_prefix(':').is_some_and(|s| {
            !s.is_empty()
                && s.split('.').count() <= 2
                && s.split('.')
                    .all(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
        });
        if !valid {
            return Err(DeviceError::Invalid(
                "expected explicit local display :N or :N.S".into(),
            ));
        }
        let (connection, screen) = x11rb::connect(Some(display)).map_err(unavailable)?;
        // Enigo's absolute motion targets the screen currently holding the pointer.
        // Refuse multi-screen servers until input is bound to the observed root.
        if connection.setup().roots.len() != 1 {
            return Err(DeviceError::Unsupported(
                "MVP input requires a single-screen X11 server".into(),
            ));
        }
        let s = &connection.setup().roots[screen];
        // Resize-enabled targets need a revision that survives a size round trip.
        let randr = connection
            .query_extension(b"RANDR")
            .map_err(unavailable)?
            .reply()
            .map_err(unavailable)?
            .present;
        let randr = if randr {
            let version = connection
                .randr_query_version(1, 3)
                .map_err(unavailable)?
                .reply()
                .map_err(unavailable)?;
            (version.major_version, version.minor_version) >= (1, 3)
        } else {
            false
        };
        Ok(Self {
            target: target.clone(),
            randr,
            root: s.root,
            visual: s.root_visual,
            connection,
        })
    }
    fn geometry(&self) -> Result<Geometry> {
        let g = self
            .connection
            .get_geometry(self.root)
            .map_err(unavailable)?
            .reply()
            .map_err(unavailable)?;
        if g.width > 32_768 || g.height > 32_768 {
            return Err(DeviceError::Unsupported(
                "X11 input requires dimensions at most 32768 pixels".into(),
            ));
        }
        let Endpoint::LocalX11 { display } = &self.target.endpoint;
        let revision = if self.randr && self.target.desktop_resize.policy.enabled {
            self.connection
                .randr_get_screen_resources_current(self.root)
                .map_err(unavailable)?
                .reply()
                .map_err(unavailable)?
                .config_timestamp
        } else {
            0
        };
        Ok(Geometry {
            target_id: self.target.id.clone(),
            surface_id: "display".into(),
            width: u32::from(g.width),
            height: u32::from(g.height),
            revision: format!(
                "{display}:{}:{}:{}:{}:{revision}",
                self.root, g.width, g.height, g.depth
            ),
        })
    }
}
impl Backend for X11 {
    fn supports_resize_revisions(&self) -> bool {
        self.randr
    }
    fn doctor(&self) -> Result<Readiness> {
        let Endpoint::LocalX11 { display } = &self.target.endpoint;
        Enigo::new(&Settings {
            x11_display: Some(display.clone()),
            ..Settings::default()
        })
        .map_err(unavailable)?;
        Ok(Readiness {
            geometry: self.screenshot(&CaptureOptions::default())?.geometry,
            capabilities: vec![Capability::Screenshot, Capability::Pointer, Capability::Keyboard],
            desktop_resize: crate::ResizeReadiness::default(),
        })
    }
    fn screenshot(&self, options: &CaptureOptions) -> Result<Observation> {
        let geometry = self.geometry()?;
        let plan = options.plan(&geometry)?;
        let image = self
            .connection
            .get_image(
                ImageFormat::Z_PIXMAP,
                self.root,
                i16::try_from(plan.region.x).map_err(unavailable)?,
                i16::try_from(plan.region.y).map_err(unavailable)?,
                u16::try_from(plan.region.width).map_err(unavailable)?,
                u16::try_from(plan.region.height).map_err(unavailable)?,
                u32::MAX,
            )
            .map_err(unavailable)?
            .reply()
            .map_err(unavailable)?;
        let captured_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(unavailable)?
            .as_millis();
        let setup = self.connection.setup();
        let format = setup
            .pixmap_formats
            .iter()
            .find(|f| f.depth == image.depth)
            .ok_or_else(|| DeviceError::Unsupported("unknown pixel format".into()))?;
        let visual = setup
            .roots
            .iter()
            .flat_map(|s| &s.allowed_depths)
            .flat_map(|d| &d.visuals)
            .find(|v| v.visual_id == self.visual)
            .ok_or_else(|| DeviceError::Unsupported("unknown visual".into()))?;
        if format.bits_per_pixel != 32
            || setup.image_byte_order != ImageOrder::LSB_FIRST
            || visual.red_mask != 0x00ff_0000
            || visual.green_mask != 0xff00
            || visual.blue_mask != 0xff
        {
            return Err(DeviceError::Unsupported(
                "MVP capture requires 32-bit BGRX pixels".into(),
            ));
        }
        let expected = plan.region.width as usize * plan.region.height as usize * 4;
        if image.data.len() != expected || self.geometry()? != geometry {
            return Err(DeviceError::StaleGeometry);
        }
        let rgb: Vec<u8> = image
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[2], p[1], p[0]])
            .collect();
        let (image, mime_type) = plan.encode(rgb)?;
        Ok(Observation {
            geometry,
            source_region: plan.region,
            image_dimensions: plan.output,
            captured_unix_ms,
            mime_type: mime_type.into(),
            image_base64: STANDARD.encode(image),
        })
    }
    fn act(&mut self, request: &ActRequest) -> Result<ActionReceipt> {
        if self.geometry()? != request.geometry {
            return Err(DeviceError::StaleGeometry);
        }
        if let Action::Type { text } = &request.action {
            let setup = self.connection.setup();
            let mapping = self
                .connection
                .get_keyboard_mapping(setup.min_keycode, setup.max_keycode - setup.min_keycode + 1)
                .map_err(unavailable)?
                .reply()
                .map_err(unavailable)?;
            let mut needed: std::collections::BTreeSet<_> = text.chars().collect();
            let mut available = 0;
            for (keycode, symbols) in (setup.min_keycode..=setup.max_keycode)
                .zip(mapping.keysyms.chunks(usize::from(mapping.keysyms_per_keycode)))
            {
                if keycode != 8 && symbols.iter().all(|symbol| *symbol == 0) {
                    available += 1;
                }
                // Enigo reuses exact first-level keysyms. Only Latin-1 has an
                // unambiguous mapping without duplicating its legacy symbol table.
                if let Some(&symbol) = symbols.first() {
                    let codepoint = match symbol {
                        0x20..=0x7e | 0xa0..=0xff => Some(symbol),
                        _ => None,
                    };
                    if let Some(character) = codepoint.and_then(char::from_u32) {
                        needed.remove(&character);
                    }
                }
            }
            if needed.len() > available {
                return Err(DeviceError::Invalid(
                    "text exceeds available X11 Unicode key mappings".into(),
                ));
            }
        }
        let Endpoint::LocalX11 { display } = &self.target.endpoint;
        let mut input = Enigo::new(&Settings {
            x11_display: Some(display.clone()),
            ..Settings::default()
        })
        .map_err(unavailable)?;
        inject(&mut input, &request.action).map_err(|e| DeviceError::Indeterminate(e.to_string()))?;
        Ok(ActionReceipt {
            state: "dispatched".into(),
            geometry: request.geometry.clone(),
        })
    }
}

fn inject(input: &mut Enigo, action: &Action) -> enigo::InputResult<()> {
    use enigo::{Axis, Button as B, Coordinate::Abs};
    match action {
        Action::Click { at, button } => {
            input.move_mouse(at.x, at.y, Abs)?;
            let button = match button {
                crate::Button::Left => B::Left,
                crate::Button::Middle => B::Middle,
                crate::Button::Right => B::Right,
            };
            let press = input.button(button, Direction::Press);
            let release = input.button(button, Direction::Release);
            press.and(release)
        }
        Action::Drag { from, to, duration_ms } => {
            input.move_mouse(from.x, from.y, Abs)?;
            let result = (|| {
                input.button(B::Left, Direction::Press)?;
                let started = std::time::Instant::now();
                let duration = Duration::from_millis(u64::from(*duration_ms));
                for step in 1..=20_u16 {
                    let deadline = duration * u32::from(step) / 20;
                    std::thread::sleep(deadline.saturating_sub(started.elapsed()));
                    input.move_mouse(
                        from.x + (to.x - from.x) * i32::from(step) / 20,
                        from.y + (to.y - from.y) * i32::from(step) / 20,
                        Abs,
                    )?;
                }
                Ok(())
            })();
            let release = input.button(B::Left, Direction::Release);
            result.and(release)
        }
        Action::Scroll {
            at,
            vertical_notches,
            horizontal_notches,
        } => {
            input.move_mouse(at.x, at.y, Abs)?;
            input.scroll(*vertical_notches, Axis::Vertical)?;
            input.scroll(*horizontal_notches, Axis::Horizontal)
        }
        Action::Type { text } => {
            // X11 clients consume mapping notifications asynchronously. Keep the
            // temporary Unicode mappings alive until queued key events can drain.
            let result = (|| {
                for character in text.chars() {
                    input.key(enigo::Key::Unicode(character), Direction::Click)?;
                    std::thread::sleep(Duration::from_millis(20));
                }
                Ok(())
            })();
            std::thread::sleep(Duration::from_millis(100));
            result
        }
        Action::Key { key, modifiers } => {
            let mods: Vec<_> = modifiers
                .iter()
                .map(|m| match m {
                    Modifier::Control => enigo::Key::Control,
                    Modifier::Shift => enigo::Key::Shift,
                    Modifier::Alt => enigo::Key::Alt,
                    Modifier::Meta => enigo::Key::Meta,
                })
                .collect();
            let result = (|| {
                for m in &mods {
                    input.key(*m, Direction::Press)?;
                }
                input.key(key_code(*key), Direction::Click)
            })();
            let mut cleanup = Ok(());
            for m in mods.iter().rev() {
                let release = input.key(*m, Direction::Release);
                if release.is_err() {
                    cleanup = release;
                }
            }
            result.and(cleanup)
        }
    }
}
fn key_code(key: Key) -> enigo::Key {
    match key {
        Key::Enter => enigo::Key::Return,
        Key::Escape => enigo::Key::Escape,
        Key::Tab => enigo::Key::Tab,
        Key::Backspace => enigo::Key::Backspace,
        Key::Delete => enigo::Key::Delete,
        Key::Left => enigo::Key::LeftArrow,
        Key::Right => enigo::Key::RightArrow,
        Key::Up => enigo::Key::UpArrow,
        Key::Down => enigo::Key::DownArrow,
        Key::Home => enigo::Key::Home,
        Key::End => enigo::Key::End,
        Key::Space => enigo::Key::Space,
    }
}
