mod config;
mod error;
mod event;
mod winman;

use log::{debug, error, info};
use serde::Deserialize;
use std::rc::Rc;

use config::Config;
use error::{Error, Result};
use event::{EventHandler, EventRouter};
use winman::WinMan;

use x11rb::connection::Connection;

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Deserialize)]
pub enum KeybindAction {
    Press,
    Release,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Deserialize)]
pub enum Command {
    Quit,
    ShowBorder,
    HideBorder,
    Close,
    FocusNext,
    FocusPrev,
    OpenLauncher,
}

pub fn start<S>(display_name: S) -> Result<()>
where
    S: Into<Option<&'static str>>,
{
    let config: Rc<Config> = Config::load()?.into();

    // Connect with the X server (specified by $DISPLAY).
    let (conn, _) = x11rb::connect(display_name.into()).map_err(|_| Error::ConnectionFailed)?;
    let conn = Rc::new(conn);

    // Get a root window on the first screen.
    let screen = conn.setup().roots.get(0).expect("No screen");
    let root = screen.root;
    debug!("root = {}", root);

    let mut router = EventRouter::default();
    let wm = WinMan::new(conn.clone(), config, root)?;
    router.add_handler(Box::new(wm));

    loop {
        let x11_event = conn.wait_for_event()?;
        router.handle_event(x11_event.clone())?;
    }
}

fn main() {
    env_logger::init();

    info!("hello");
    match start(None) {
        Ok(()) | Err(Error::Quit) => {
            info!("goodbye");
        }
        Err(err) => {
            error!("{}", err);
        }
    }
}
