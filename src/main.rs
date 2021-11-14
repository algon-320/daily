mod atom;
mod bar;
mod config;
mod context;
mod error;
mod event;
mod layout;
mod monitor;
mod screen;
mod window;
mod winman;

/// A wrapper for `std::thread::spawn` to give a name to the thread.
pub fn spawn_named_thread<F, T>(name: String, body: F) -> std::thread::JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    std::thread::Builder::new()
        .name(name)
        .spawn(body)
        .expect("failed to spawn thread")
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, serde::Deserialize)]
pub enum KeybindAction {
    Press,
    Release,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, serde::Deserialize)]
pub enum Command {
    Quit,
    Restart,
    ShowBorder,
    HideBorder,
    Close,
    Sink,
    FocusNext,
    FocusPrev,
    FocusNextMonitor,
    FocusPrevMonitor,
    NextLayout,
    Spawn(String),
    Screen(usize),
    MoveToScreen(usize),
    MovePointerRel(i16, i16), // (dx, dy)
    MouseClickLeft,
}

use error::{Error, Result};
use log::{debug, error, info};

pub fn start<S>(display_name: S) -> Result<()>
where
    S: Into<Option<&'static str>>,
{
    use event::EventHandler;
    use x11rb::connection::Connection;

    let ctx = context::init(display_name)?;
    let mut wm = winman::WinMan::new(ctx.clone())?;
    debug!("WinMan initialized");

    let (event_tx, event_rx) = crossbeam_channel::unbounded();

    // a thread to consume X11 events.
    spawn_named_thread("main-x11".to_owned(), {
        let ctx = ctx.clone();
        move || loop {
            let event = ctx.conn.wait_for_event();
            let res = event_tx.send(event);
            if res.is_err() {
                return;
            }
        }
    });

    let timer_rx = crossbeam_channel::tick(std::time::Duration::from_secs(10));

    // main thread: processes events gathered from the others.
    loop {
        crossbeam_channel::select! {
            recv(event_rx) -> event => {
                let event = event.expect("event_tx has been closed.")?;
                let res = wm.handle_event(event);

                // Ignore WINDOW errors ...
                //     because WINDOW errors occur during processing a event
                //     which was generated on a already destroyed window at the time.
                use x11rb::protocol::ErrorKind;
                if let Err(err) = res {
                    if err.x11_error_kind() == Some(ErrorKind::Window) {
                        debug!("Ignored WINDOW error: {:?}", err);
                    } else {
                        return Err(err);
                    }
                }

                ctx.conn.flush()?;
            }
            recv(timer_rx) -> _ => {
                wm.alarm()?;
                ctx.conn.flush()?;
            }
        }
    }
}

fn main() {
    env_logger::init();

    use std::process::exit;

    info!("hello");
    let status = match start(None) {
        Ok(()) | Err(Error::Quit) => {
            info!("goodbye");
            0
        }
        Err(Error::Restart) => {
            info!("try to restart");
            2
        }
        Err(err) => {
            error!("{}", err);
            1
        }
    };
    exit(status);
}
