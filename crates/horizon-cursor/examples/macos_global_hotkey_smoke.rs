//! Main-thread macOS smoke harness for native global-hotkey delivery.

#[cfg(target_os = "macos")]
mod macos {
    use std::time::{Duration, Instant};

    use horizon_cursor::{GlobalHotkeys, Hotkey, HotkeyEvent, HotkeyKey};
    use winit::application::ApplicationHandler;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};

    const PROFILE: usize = 7;
    const TIMEOUT: Duration = Duration::from_secs(10);

    pub(super) fn run() -> bool {
        let event_loop = match EventLoop::<()>::with_user_event().build() {
            Ok(event_loop) => event_loop,
            Err(error) => {
                eprintln!("failed to create the macOS event loop: {error}");
                return false;
            }
        };
        let hotkeys = match GlobalHotkeys::listen(&[(
            PROFILE,
            Hotkey {
                ctrl: false,
                shift: false,
                alt: false,
                super_: false,
                key: HotkeyKey::Function(9),
            },
        )]) {
            Ok(hotkeys) => hotkeys,
            Err(error) => {
                eprintln!("failed to register F9: {error}");
                return false;
            }
        };
        let proxy = event_loop.create_proxy();
        hotkeys.set_wake(move || {
            let _ = proxy.send_event(());
        });

        let deadline = Instant::now() + TIMEOUT;
        event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
        let mut app = SmokeApp {
            hotkeys,
            deadline,
            pressed: false,
            released: false,
            disconnected: false,
        };
        println!("SMOKE READY: press and release F9");
        if let Err(error) = event_loop.run_app(&mut app) {
            eprintln!("macOS event loop failed: {error}");
            return false;
        }
        if app.disconnected {
            eprintln!("global hotkey listener disconnected");
            return false;
        }
        if !(app.pressed && app.released) {
            eprintln!("timed out before receiving both F9 transitions");
            return false;
        }
        println!("SMOKE PASS: received one F9 press and release");
        true
    }

    struct SmokeApp {
        hotkeys: GlobalHotkeys,
        deadline: Instant,
        pressed: bool,
        released: bool,
        disconnected: bool,
    }

    impl SmokeApp {
        fn poll(&mut self, event_loop: &ActiveEventLoop) {
            while let Some(event) = self.hotkeys.try_recv() {
                match event {
                    HotkeyEvent::Pressed(PROFILE) => self.pressed = true,
                    HotkeyEvent::Released(PROFILE) => self.released = true,
                    HotkeyEvent::Disconnected => self.disconnected = true,
                    HotkeyEvent::Pressed(_) | HotkeyEvent::Released(_) => {}
                }
            }
            if self.disconnected || (self.pressed && self.released) || Instant::now() >= self.deadline {
                event_loop.exit();
            } else {
                event_loop.set_control_flow(ControlFlow::WaitUntil(self.deadline));
            }
        }
    }

    impl ApplicationHandler<()> for SmokeApp {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            self.poll(event_loop);
        }

        fn user_event(&mut self, event_loop: &ActiveEventLoop, (): ()) {
            self.poll(event_loop);
        }

        fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
            self.poll(event_loop);
        }

        fn window_event(
            &mut self,
            _event_loop: &ActiveEventLoop,
            _window_id: winit::window::WindowId,
            _event: WindowEvent,
        ) {
        }
    }
}

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    if macos::run() {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}

#[cfg(not(target_os = "macos"))]
fn main() -> std::process::ExitCode {
    eprintln!("macos_global_hotkey_smoke is available only on macOS");
    std::process::ExitCode::FAILURE
}
