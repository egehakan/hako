//! Quits the app the normal way on SIGTERM and SIGHUP, so that what is done as the app quits,
//! such as quitting the Mac browser's Chrome, is done then too.

use std::io;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use futures::StreamExt as _;
use signal_hook::consts::{SIGHUP, SIGTERM};
use signal_hook::flag;
use signal_hook_tokio::Signals;
use warpui::platform::TerminationMode;
use warpui::{Entity, ModelContext};

const SIGNALS: [libc::c_int; 2] = [SIGTERM, SIGHUP];

pub(super) fn quit_on_signals<T: Entity>(ctx: &mut ModelContext<T>) {
    ctx.spawn(
        async {
            // `Signals` registers with the Tokio reactor, which the background executor runs.
            let mut signals = Signals::new(SIGNALS)?;
            // A second delivery ends the app at once, should the normal quit hang.
            for signal in SIGNALS {
                let armed = Arc::new(AtomicBool::new(false));
                flag::register_conditional_default(signal, Arc::clone(&armed))?;
                flag::register(signal, armed)?;
            }
            Ok::<_, io::Error>(signals.next().await)
        },
        |_, received, ctx| match received {
            Ok(Some(signal)) => {
                log::info!("Quitting on signal {signal}");
                ctx.terminate_app(TerminationMode::ForceTerminate, None);
            }
            Ok(None) => {}
            Err(error) => log::warn!("Could not watch for SIGTERM and SIGHUP: {error}"),
        },
    );
}
