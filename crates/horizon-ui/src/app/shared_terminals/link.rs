//! The live link as the join uses it, so a test joins an owner and a follower
//! without a server.

use horizon_sync::{LiveEvent, LiveHandle, LivePanel};

/// Declares [`LiveLink`] as the calls of [`LiveHandle`] that the join makes, and
/// gives the handle that trait. None of the calls waits or does I/O, so the
/// frame makes them.
macro_rules! live_link {
    ($($call:ident(&self $(, $argument:ident: $kind:ty)*) $(-> $answer:ty)?;)*) => {
        pub(in crate::app) trait LiveLink: Send {
            $(fn $call(&self $(, $argument: $kind)*) $(-> $answer)?;)*
        }

        impl LiveLink for LiveHandle {
            $(fn $call(&self $(, $argument: $kind)*) $(-> $answer)? {
                Self::$call(self $(, $argument)*)
            })*
        }
    };
}

live_link! {
    set_panels(&self, panels: Vec<LivePanel>);
    follow(&self, owner: &str, panel: &str);
    unfollow(&self, owner: &str, panel: &str);
    refresh(&self, owner: &str, panel: &str);
    resize(&self, owner: &str, panel: &str, cols: u16, rows: u16);
    screen(&self, panel: &str, cols: u16, rows: u16, fresh: bool, everyone: bool) -> bool;
    output(&self, panel: &str, bytes: &[u8], replay: bool) -> bool;
    ended(&self, panel: &str);
    typed(&self, owner: &str, panel: &str, bytes: &[u8]);
    take_events(&self) -> Vec<LiveEvent>;
    open_mirror(&self);
    follow_mirror(&self, owner: &str, program: &str, session: &str);
    unfollow_mirror(&self, owner: &str, program: &str, session: &str);
}
