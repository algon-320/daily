use log::info;
use std::process::Command;

const DAILY_BIN_NAME: &str = "daily";

fn main() {
    loop {
        info!("Try to launch a Daily process.");
        let mut ch = Command::new(DAILY_BIN_NAME).spawn().unwrap();
        if ch.wait().unwrap().success() {
            info!("Daily has gracefully exited.");
            break;
        }
    }
}
