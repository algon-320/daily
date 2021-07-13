use std::cell::RefCell;
use std::collections::HashMap;

use crate::{Command, KeyCode, KeybindAction};
use x11rb::protocol::xproto::ModMask;

#[derive(Debug)]
pub struct Config {
    pub keybind: RefCell<HashMap<(KeybindAction, u16, u8), Command>>,
    pub launcher: String,
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
