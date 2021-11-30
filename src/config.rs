use crate::error::{Error, Result};
use crate::{Command, KeybindAction};
use log::info;
use std::collections::HashMap;
use std::convert::TryInto;

//  KeyCode:
//      1 = 10,
//      2 = 11,
//      3 = 12,
//      ...
//      Tab = 23,
//      Q = 24,
//      T = 28,
//      P = 33,
//      Enter = 36,
//      S = 39,
//      H = 43,
//      J = 44,
//      K = 45,
//      L = 46,
//      C = 54,
//      Space = 65,
//      SuperL = 133,
//      SuperR = 134,
//      AltL = 64,
//      AltR = 108,
//      Up = 111,
//      Down = 116,
//      Left = 113,
//      Right = 114,

const DEFAULT_CONFIG: &str = r###"
background_color: '#343255'
border:
    width: 1
    color_focused: '#00f080'
    color_regular: '#00003e'

keybind:
    - { action: Press,   mod: [Super],        key: 33,  command: {Spawn: /usr/bin/dmenu_run} }
    - { action: Press,   mod: [Super],        key: 28,  command: {Spawn: /usr/bin/xterm} }

    - { action: Press,   mod: [Super, Shift], key: 24,  command: Quit }
    - { action: Press,   mod: [Super],        key: 54,  command: Close }
    - { action: Press,   mod: [Super],        key: 23,  command: FocusNext }
    - { action: Press,   mod: [Super, Shift], key: 23,  command: FocusPrev }
    - { action: Press,   mod: [Super],        key: 44,  command: FocusNextMonitor }
    - { action: Press,   mod: [Super],        key: 45,  command: FocusPrevMonitor }
    - { action: Press,   mod: [Super],        key: 65,  command: NextLayout }
    - { action: Press,   mod: [Super],        key: 39,  command: Sink }

    - { action: Press,   mod: [],             key: 133, command: ShowBorder }
    - { action: Release, mod: [Super],        key: 133, command: HideBorder }

    - { action: Press,   mod: [Super],        key: 43,  command: {LayoutCommand: "-"} }
    - { action: Press,   mod: [Super],        key: 46,  command: {LayoutCommand: "+"} }

    - { action: Press,   mod: [Super],        key: 10,  command: {Screen: 0} }
    - { action: Press,   mod: [Super],        key: 11,  command: {Screen: 1} }
    - { action: Press,   mod: [Super],        key: 12,  command: {Screen: 2} }
    - { action: Press,   mod: [Super],        key: 13,  command: {Screen: 3} }
    - { action: Press,   mod: [Super],        key: 14,  command: {Screen: 4} }

    - { action: Press,   mod: [Super, Shift], key: 10,  command: {MoveToScreen: 0} }
    - { action: Press,   mod: [Super, Shift], key: 11,  command: {MoveToScreen: 1} }
    - { action: Press,   mod: [Super, Shift], key: 12,  command: {MoveToScreen: 2} }
    - { action: Press,   mod: [Super, Shift], key: 13,  command: {MoveToScreen: 3} }
    - { action: Press,   mod: [Super, Shift], key: 14,  command: {MoveToScreen: 4} }

    - { action: Press,   mod: [Super],        key: 111, command: {MovePointerRel: [  0, -32]} }
    - { action: Press,   mod: [Super],        key: 116, command: {MovePointerRel: [  0,  32]} }
    - { action: Press,   mod: [Super],        key: 113, command: {MovePointerRel: [-32,   0]} }
    - { action: Press,   mod: [Super],        key: 114, command: {MovePointerRel: [ 32,   0]} }
    - { action: Press,   mod: [Super, Shift], key: 111, command: {MovePointerRel: [  0,  -1]} }
    - { action: Press,   mod: [Super, Shift], key: 116, command: {MovePointerRel: [  0,   1]} }
    - { action: Press,   mod: [Super, Shift], key: 113, command: {MovePointerRel: [ -1,   0]} }
    - { action: Press,   mod: [Super, Shift], key: 114, command: {MovePointerRel: [  1,   0]} }

    - { action: Press,   mod: [Super],        key: 36,  command: MouseClickLeft }
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
    pub struct ConfigYamlRepr {
        keybind: Vec<KeyBind>,
        border: BorderConfig,
        background_color: String,
    }

    fn parse_color(hex: &str) -> Result<u32> {
        let hex = hex.trim_start_matches('#');
        u32::from_str_radix(hex, 16).map_err(|_| Error::InvalidConfig {
            reason: "expect a color of \"#RRGGBB\" (in hex)".to_owned(),
        })
    }

    impl std::convert::TryFrom<BorderConfig> for super::BorderConfig {
        type Error = Error;
        fn try_from(yaml_repr: BorderConfig) -> Result<Self> {
            Ok(super::BorderConfig {
                width: yaml_repr.width,
                color_focused: parse_color(&yaml_repr.color_focused)?,
                color_regular: parse_color(&yaml_repr.color_regular)?,
            })
        }
    }

    impl std::convert::TryFrom<ConfigYamlRepr> for Config {
        type Error = Error;
        fn try_from(yaml_repr: ConfigYamlRepr) -> Result<Self> {
            let mut keybind = HashMap::new();
            for kb in yaml_repr.keybind {
                let mut modmask: u16 = 0;
                for m in kb.r#mod {
                    modmask |= Into::<u16>::into(m);
                }
                keybind.insert((kb.action, modmask, kb.key), kb.command);
            }

            let background_color = parse_color(&yaml_repr.background_color)?;

            Ok(Config {
                keybind,
                border: yaml_repr.border.try_into()?,
                background_color,
            })
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BorderConfig {
    pub width: u32,
    pub color_focused: u32,
    pub color_regular: u32,
}

#[derive(Debug)]
pub struct Config {
    pub keybind: HashMap<(KeybindAction, u16, u8), Command>,
    pub border: BorderConfig,
    pub background_color: u32,
}

impl Config {
    pub fn load() -> Result<Self> {
        use ::config::{File, FileFormat};
        use std::{env, path::PathBuf};

        let mut conf = ::config::Config::new();

        // Default
        conf.merge(File::from_str(DEFAULT_CONFIG, FileFormat::Yaml).required(true))
            .expect("ill-formed DEFAULT_CONFIG");

        // config.yml localted on the current working directory.
        conf.merge(File::new("config.yml", FileFormat::Yaml).required(false))
            .map_err(|e| Error::InvalidConfig {
                reason: e.to_string(),
            })?;

        // config.yml localted on the xdg user config directory.
        let mut xdg_config = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let mut p = PathBuf::new();
                p.push(env::var_os("HOME").unwrap_or_else(|| "".into()));
                p.push(".config");
                p
            });
        xdg_config.push("daily");
        xdg_config.push("config.yml");
        conf.merge(
            File::new(
                xdg_config.to_str().expect("not UTF-8 path"),
                FileFormat::Yaml,
            )
            .required(false),
        )
        .map_err(|e| Error::InvalidConfig {
            reason: e.to_string(),
        })?;

        // Generate config
        let yaml_repr: parse::ConfigYamlRepr =
            conf.try_into().map_err(|e| Error::InvalidConfig {
                reason: e.to_string(),
            })?;
        yaml_repr.try_into()
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

        use ::config::{File, FileFormat};
        let mut conf = ::config::Config::new();
        conf.merge(File::from_str(DEFAULT_CONFIG, FileFormat::Yaml).required(true))
            .expect("ill-formed DEFAULT_CONFIG");

        let yaml_repr: parse::ConfigYamlRepr = conf.try_into().unwrap();
        yaml_repr.try_into().expect("ill-formed DEFAULT_CONFIG")
    }
}
