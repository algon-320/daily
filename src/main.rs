mod atom;
mod config;
mod context;
mod error;
mod event;
mod layout;
mod screen;
mod window;
mod winman;

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
    ctx.conn.flush()?;

    enum DailyEvent {
        X11(x11rb::protocol::Event),
        Error(x11rb::errors::ConnectionError),
        Alarm,
    }
    let (event_tx, event_rx) = std::sync::mpsc::channel::<DailyEvent>();

    // a thread to generate alarm events periodically.
    {
        use std::time::Duration;
        const PERIOD: Duration = Duration::from_secs(10);

        let event_tx = event_tx.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(PERIOD);
            let res = event_tx.send(DailyEvent::Alarm);
            if res.is_err() {
                return;
            }
        });
    }

    // a thread to consume X11 events.
    {
        let ctx = ctx.clone();
        std::thread::spawn(move || loop {
            let daily_event = match ctx.conn.wait_for_event() {
                Ok(ev) => DailyEvent::X11(ev),
                Err(err) => DailyEvent::Error(err),
            };
            let res = event_tx.send(daily_event);
            if res.is_err() {
                return;
            }
        });
    }

    // main thread: processes events gathered from the others.
    while let Ok(ev) = event_rx.recv() {
        match ev {
            DailyEvent::X11(ev) => {
                let res = wm.handle_event(ev);

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
            DailyEvent::Alarm => {
                wm.alarm()?;
                ctx.conn.flush()?;
            }
            DailyEvent::Error(e) => {
                return Err(e.into());
            }
        }
    }

    Ok(())
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
