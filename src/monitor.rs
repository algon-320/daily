use x11rb::protocol::randr::MonitorInfo;

use crate::bar::BarHandle;
use crate::context::Context;

#[derive(Debug)]
pub struct Monitor {
    pub id: usize,
    pub info: MonitorInfo,
    pub bar: BarHandle,
}

impl Monitor {
    pub fn new(ctx: &Context, id: usize, info: MonitorInfo) -> Self {
        let mut bar = BarHandle::new(ctx, id);
        bar.show().expect("TODO: bar.show");

        Self { id, info, bar }
    }
}
