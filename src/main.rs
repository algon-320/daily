mod error;
mod event;
mod winman;

use log::{debug, error, info};
use std::collections::HashMap;
use std::rc::Rc;

use error::{Error, Result};
use event::{EventHandler, EventRouter};
use winman::WinMan;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;

#[repr(u8)]
enum KeyCode {
    Tab = 23,
    Q = 24,
    C = 54,
    P = 33,
    Super = 133,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum KeybindAction {
    Press,
    Release,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum Command {
    Exit,
    ShowBorder,
    HideBorder,
    Close,
    FocusNext,
    FocusPrev,
    OpenLauncher,
}

use std::cell::RefCell;

#[derive(Debug)]
pub struct Config {
    keybind: RefCell<HashMap<(KeybindAction, u16, u8), Command>>,
    launcher: String,
}

impl Config {
    pub fn add_keybind(&self, on: KeybindAction, modifier: u16, keycode: u8, cmd: Command) {
        let mut keybind = self.keybind.borrow_mut();
        keybind.insert((on, modifier, keycode), cmd);
    }

    pub fn get_keybind(&self, on: KeybindAction, modifier: u16, keycode: u8) -> Option<Command> {
        let keybind = self.keybind.borrow();
        keybind.get(&(on, modifier, keycode)).cloned()
    }

    pub fn bounded_keys(&self) -> Vec<(KeybindAction, u16, u8)> {
        let keybind = self.keybind.borrow();
        let mut keys = Vec::new();
        for &(on, m, c) in keybind.keys() {
            keys.push((on, m, c));
        }
        keys
    }
}

impl Default for Config {
    fn default() -> Self {
        let config = Config {
            keybind: Default::default(),
            launcher: "/usr/bin/dmenu_run".to_owned(),
        };

        // Default keybind
        {
            let win = ModMask::M4.into();
            let win_shift = (ModMask::M4 | ModMask::SHIFT).into();
            config.add_keybind(
                KeybindAction::Press,
                win,
                KeyCode::Tab as u8,
                Command::FocusNext,
            );
            config.add_keybind(
                KeybindAction::Press,
                win_shift,
                KeyCode::Tab as u8,
                Command::FocusPrev,
            );
            config.add_keybind(
                KeybindAction::Press,
                win_shift,
                KeyCode::Q as u8,
                Command::Exit,
            );
            config.add_keybind(KeybindAction::Press, win, KeyCode::C as u8, Command::Close);
            config.add_keybind(
                KeybindAction::Press,
                0,
                KeyCode::Super as u8,
                Command::ShowBorder,
            );
            config.add_keybind(
                KeybindAction::Release,
                win,
                KeyCode::Super as u8,
                Command::HideBorder,
            );
            config.add_keybind(
                KeybindAction::Press,
                win,
                KeyCode::P as u8,
                Command::OpenLauncher,
            );
        }

        config
    }
}

pub fn start<S>(display_name: S) -> Result<()>
where
    S: Into<Option<&'static str>>,
{
    let config: Rc<Config> = Config::default().into();

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
