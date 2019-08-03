#![feature(async_await)]

mod settings;

use crate::settings::{BinSettings, Name};
use colored::*;
use failure::Error;
use futures::compat::{Future01CompatExt, Stream01CompatExt};
use futures::StreamExt;
use futures_legacy::prelude::*;
use runtime::task::JoinHandle;
use std::collections::HashMap;
use std::env;
use std::io::BufReader;
use std::process::{Command, Stdio};
use tokio_process::{Child, ChildStdout, CommandExt};

async fn run_command(name: Name, bin: BinSettings) -> Result<(), Error> {
    log::info!("Starting '{}': {}", name, bin.path);
    let mut cmd = Command::new(bin.path);
    let mut filtered_env: HashMap<String, String> = env::vars()
        .filter(|&(ref k, _)| k == "TERM" || k == "TZ" || k == "LANG" || k == "PATH")
        .collect();
    let env_iter = bin.env.into_iter().map(|(k, v)| (k.to_uppercase(), v));
    filtered_env.extend(env_iter);
    cmd.env_clear();
    log::trace!("Set env for '{}': {:?}", name, filtered_env);
    cmd.envs(&filtered_env);
    cmd.stderr(Stdio::piped());
    match cmd.spawn_async() {
        Ok(mut child) => {
            log::debug!("Started: '{}'", name);
            if let Some(stderr) = child.stderr().take() {
                let mut lines = tokio_io::io::lines(BufReader::new(stderr)).compat();
                runtime::spawn(child.compat());
                while let Some(line) = lines.next().await {
                    match line {
                        Ok(line) => {
                            println!("{} | {}", name.green(), line);
                        }
                        Err(err) => {
                            log::warn!("Can't read line from stderr of '{}': {}", name, err);
                        }
                    }
                }
            } else {
                log::warn!("Can't get a stderr stream of '{}'", name);
            }
        }
        Err(err) => {
            log::error!("Can't start '{}': {}", name, err);
        }
    }
    Ok(())
}

/// This struct holds `JoinHandle` of a spawned routine that
/// reprints output and contains a channel to send management commands
/// to a process. But maybe use signals to end them?
struct RunContext {
    handle: JoinHandle<Result<(), Error>>,
}

impl RunContext {
    fn start(name: Name, bin: BinSettings) -> Self {
        let handle = runtime::spawn(run_command(name, bin));
        Self { handle }
    }

    async fn end(self) -> Result<(), Error> {
        self.handle.await
    }
}

#[runtime::main(runtime_tokio::Tokio)]
async fn main() -> Result<(), Error> {
    env_logger::try_init()?;
    let settings = settings::Settings::parse()?;
    let mut ctrl_c = tokio_signal::ctrl_c().flatten_stream().compat();
    let mut processes_map = HashMap::new();
    for (name, bin) in settings.bins {
        let context = RunContext::start(name.clone(), bin);
        processes_map.insert(name, context);
    }
    ctrl_c.next().await;
    log::debug!("Terminating...");
    for (name, proc) in processes_map {
        log::info!("Finishing the process '{}'", name);
        proc.end();
    }
    Ok(())
}
