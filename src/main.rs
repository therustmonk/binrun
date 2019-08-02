#![feature(async_await)]

mod settings;

use failure::Error;
use futures::compat::Stream01CompatExt;
use futures::StreamExt;
use futures_legacy::prelude::*;
use std::io::BufReader;
use std::process::Command;
use tokio_process::{Child, ChildStdout, CommandExt};

#[runtime::main(runtime_tokio::Tokio)]
async fn main() -> Result<(), Error> {
    env_logger::try_init()?;
    let settings = settings::Settings::parse()?;
    let ctrl_c = tokio_signal::ctrl_c().flatten_stream();
    for (name, bin) in settings.bins {
        log::info!("Starting '{}': {}", name, bin.path);
        match Command::new(bin.path).spawn_async() {
            Ok(mut child) => {
                log::debug!("Started: '{}'", name);
                if let Some(stderr) = child.stderr().take() {
                    let mut lines = tokio_io::io::lines(BufReader::new(stderr)).compat();
                    while let Some(line) = lines.next().await {
                    }
                } else {
                    log::warn!("Can't get a stderr stream of '{}'", name);
                }
            }
            Err(err) => {
                log::error!("Can't start '{}': {}", name, err);
            }
        }
    }
    Ok(())
}
