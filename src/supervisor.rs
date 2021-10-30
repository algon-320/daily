use log::{error, info};
use std::process::Command;
use std::time::Instant;

const DAILY_BIN_NAME: &str = "daily";

fn main() {
    env_logger::init();

    let mut retries = 0;
    loop {
        info!("Try to launch a Daily process.");
        let mut ch = Command::new(DAILY_BIN_NAME).spawn().unwrap();
        let started_time = Instant::now();

        // Exit caused by user.
        if ch.wait().unwrap().success() {
            info!("Daily has gracefully exited.");
            break;
        }

        if started_time.elapsed().as_millis() < 3000 {
            retries += 1;

            // Maybe something is wrong in launching process.
            if retries > 10 {
                error!("Frequent failure");
                return;
            }
        } else {
            retries = 0;
        }

        info!("uptime: {} secs.", started_time.elapsed().as_secs());
    }
}
