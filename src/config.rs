use crate::error::{Error, Result};
use crate::{Command, KeybindAction};
use log::info;
use std::collections::HashMap;
use std::convert::TryInto;

//  KeyCode:
//      Tab = 23,
//      Q = 24,
//      C = 54,
//      P = 33,
//      SuperL = 133,
//      SuperR = 134,
//      AltL = 64,
//      AltR = 108,

const DEFAULT: &str = r###"
launcher = "/usr/bin/dmenu_run"

keybind = [
    { action = "Press",   mod = ["Super", "Shift"], key = 24,  command = "Quit"},
    { action = "Press",   mod = ["Super"],          key = 54,  command = "Close"},
    { action = "Press",   mod = ["Super"],          key = 33,  command = "OpenLauncher"},
    { action = "Press",   mod = ["Super"],          key = 23,  command = "FocusNext"},
    { action = "Press",   mod = ["Super", "Shift"], key = 23,  command = "FocusPrev"},
    { action = "Press",   mod = [],                 key = 133, command = "ShowBorder"},
    { action = "Release", mod = ["Super"],          key = 133, command = "HideBorder"},
]

[border]
width = 1
color_focused = "#FF8882"
color_regular = "#505050"
"###;

mod parse {
    use crate::error::{Error, Result};
    use crate::{Command, KeybindAction};
    use serde::Deserialize;
    use std::collections::HashMap;
    use std::convert::TryInto;
    use x11rb::protocol::xproto::ModMask;

    use super::Config;

    #[derive(Debug, Deserialize)]
    enum Modifier {
        Shift,
        Control,
        Alt,
        Super,
    }

    impl From<Modifier> for u16 {
        fn from(m: Modifier) -> u16 {
            match m {
                Modifier::Shift => ModMask::SHIFT.into(),
                Modifier::Control => ModMask::CONTROL.into(),
                Modifier::Alt => ModMask::M1.into(),
                Modifier::Super => ModMask::M4.into(),
            }
        }
    }

    #[derive(Debug, Deserialize)]
    struct KeyBind {
        action: KeybindAction,
        r#mod: Vec<Modifier>,
        key: u8,
        command: Command,
    }

    #[derive(Debug, Deserialize)]
    struct BorderConfig {
        width: u32,
        color_focused: String,
        color_regular: String,
    }

    #[derive(Debug, Deserialize)]
    pub struct ConfigTomlRepr {
        keybind: Vec<KeyBind>,
        launcher: String,
        border: BorderConfig,
    }

    fn parse_color(hex: &str) -> Result<u32> {
        let hex = hex.trim_start_matches('#');
        u32::from_str_radix(hex, 16).map_err(|_| Error::InvalidConfig {
            reason: "expect a color of \"#RRGGBB\" (in hex)".to_owned(),
        })
    }

    impl std::convert::TryFrom<BorderConfig> for super::BorderConfig {
        type Error = Error;
        fn try_from(toml_repr: BorderConfig) -> Result<Self> {
            Ok(super::BorderConfig {
                width: toml_repr.width,
                color_focused: parse_color(&toml_repr.color_focused)?,
                color_regular: parse_color(&toml_repr.color_regular)?,
            })
        }
    }

    impl std::convert::TryFrom<ConfigTomlRepr> for Config {
        type Error = Error;
        fn try_from(toml_repr: ConfigTomlRepr) -> Result<Self> {
            let mut keybind = HashMap::new();
            for kb in toml_repr.keybind {
                let mut modmask: u16 = 0;
                for m in kb.r#mod {
                    modmask |= Into::<u16>::into(m);
                }
                keybind.insert((kb.action, modmask, kb.key), kb.command);
            }

            let launcher = toml_repr.launcher;

            Ok(Config {
                keybind,
                launcher,
                border: toml_repr.border.try_into()?,
            })
        }
    }
}

#[derive(Debug, Clone)]
pub struct BorderConfig {
    pub width: u32,
    pub color_focused: u32,
    pub color_regular: u32,
}

#[derive(Debug)]
pub struct Config {
    pub keybind: HashMap<(KeybindAction, u16, u8), Command>,
    pub launcher: String,
    pub border: BorderConfig,
}

impl Config {
    pub fn load() -> Result<Self> {
        const FILE: &str = "config.toml";
        match std::fs::read(FILE) {
            Ok(bytes) => {
                info!("configuration loaded from {}", FILE);
                let config_str = String::from_utf8(bytes).map_err(|_| Error::InvalidConfig {
                    reason: "ill-formed UTF-8".to_owned(),
                })?;
                let toml_repr: parse::ConfigTomlRepr =
                    toml::from_str(&config_str).map_err(|e| Error::InvalidConfig {
                        reason: format!("{}", e),
                    })?;
                toml_repr.try_into()
            }
            Err(_) => Ok(Self::default()),
        }
    }

    pub fn keybind_match(&self, on: KeybindAction, modifier: u16, keycode: u8) -> Option<Command> {
        self.keybind.get(&(on, modifier, keycode)).cloned()
    }

    pub fn keybind_iter(
        &self,
    ) -> impl Iterator<Item = (&'_ (KeybindAction, u16, u8), &'_ Command)> {
        self.keybind.iter()
    }
}

impl Default for Config {
    fn default() -> Self {
        info!("default config is used");
        let toml_repr: parse::ConfigTomlRepr =
            toml::from_str(DEFAULT).expect("Default config is wrong");
        toml_repr.try_into().expect("Default config is wrong")
    }
}
