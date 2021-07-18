use serde::Deserialize;

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
