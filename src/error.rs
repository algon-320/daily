use thiserror::Error;
use x11rb::errors::ReplyOrIdError;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Terminated by user.")]
    Quit,
    #[error("Restarted by user.")]
    Restart,

    #[error("Cannot connect with the X server.")]
    ConnectionFailed,
    #[error("Another window manager already exists.")]
    WmAlreadyExists,
    #[error("Another client has already grabbed the key we want to use.")]
    KeyAlreadyGrabbed,
    #[error("Another client has already grabbed the button we want to use.")]
    ButtonAlreadyGrabbed,

    #[error("No screen available.")]
    NoScreen,
    #[error("No monitor available.")]
    NoMonitor,

    #[error(transparent)]
    X11(ReplyOrIdError),

    #[error("Invalid config: {reason}")]
    InvalidConfig { reason: String },
}

pub type Result<T> = std::result::Result<T, Error>;

impl<T: Into<ReplyOrIdError>> From<T> for Error {
    fn from(x: T) -> Error {
        Error::X11(Into::<ReplyOrIdError>::into(x))
    }
}
