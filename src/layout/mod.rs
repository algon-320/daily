mod full;
mod horizontal;
mod vertical;

pub use full::*;
pub use horizontal::*;
pub use vertical::*;

use x11rb::protocol::randr::MonitorInfo;

use crate::error::Result;
use crate::window::Window;

pub trait Layout {
    fn layout(
        &mut self,
        mon: &MonitorInfo,
        windows: &[&Window],
        border_visible: bool,
    ) -> Result<()>;

    fn name(&self) -> &'static str;
}
