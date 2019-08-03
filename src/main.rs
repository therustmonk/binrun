#![feature(async_await)]

mod settings;

use crate::settings::{BinSettings, Name, Settings};
use colored::*;
use failure::{format_err, Error};
use futures::channel::oneshot;
use futures::compat::{Future01CompatExt, Stream01CompatExt};
use futures::{select, FutureExt, StreamExt, TryFutureExt};
use futures_legacy::prelude::*;
use nix::sys::signal;
use nix::unistd::Pid;
use runtime::task::JoinHandle;
use runtime::time::FutureExt as _;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::env;
use std::io::BufReader;
use std::process::{Command, ExitStatus, Stdio};
use std::time::Duration;
use tokio_process::{Child, ChildStdout, CommandExt};
use tokio_signal::unix::{Signal, SIGHUP, SIGINT};

async fn run_command(
    name: Name,
    bin: BinSettings,
    killer: oneshot::Receiver<()>,
) -> Result<ExitStatus, Error> {
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
                let mut lines = tokio_io::io::lines(BufReader::new(stderr)).compat().fuse();
                let mut killer = killer.fuse();
                //runtime::spawn(child.compat());
                loop {
                    select! {
                        line = lines.next() => {
                            match line {
                                Some(Ok(line)) => {
                                    println!("{} | {}", name.green(), line);
                                }
                                Some(Err(err)) => {
                                    log::warn!("Can't read line from stderr of '{}': {}", name, err);
                                }
                                None => {
                                    // If stderr closed by a process it doesn't mean the process terminated.
                                    break;
                                }
                            }
                        }
                        kill = killer => {
                            break;
                        }
                    }
                }
            } else {
                log::warn!("Can't get a stderr stream of '{}'", name);
            }
            let pid = Pid::from_raw(child.id() as i32);
            let end_strategy = vec![(signal::Signal::SIGINT, 5), (signal::Signal::SIGKILL, 15)];
            let mut end_fut = child.from_err().compat();
            for (sig, timeout) in end_strategy {
                match signal::kill(pid, sig) {
                    Ok(_) => {
                        // Wait for a process termination
                        let duration = Duration::from_secs(timeout);
                        let term: Result<Result<ExitStatus, Error>, std::io::Error> =
                            (&mut end_fut).timeout(duration).await;
                        if let Ok(exit_status) = term {
                            match exit_status {
                                Ok(exit_status) => {
                                    // exit_status.code() returns `None` for Unix. Use Debug print instead.
                                    log::info!(
                                        "Process '{}' terminated with code: {:?}",
                                        name,
                                        exit_status
                                    );
                                    return Ok(exit_status);
                                }
                                Err(err) => {
                                    log::error!("Can't get status code of '{}': {}", name, err);
                                }
                            }
                        }
                    }
                    Err(err) => {
                        // No wait and send the next signal immediately.
                        log::error!("Can't send kill signal {} to {}: {}", sig, pid, err);
                    }
                }
            }
            log::error!("Can't terminate a process with pid of '{}': {}", name, pid);
            Err(format_err!("Can't kill by pid: {}", pid))
        }
        Err(err) => {
            log::error!("Can't start '{}': {}", name, err);
            Err(Error::from(err))
        }
    }
}

/// This struct holds `JoinHandle` of a spawned routine that
/// reprints output and contains a channel to send management commands
/// to a process. But maybe use signals to end them?
struct RunContext {
    handle: JoinHandle<Result<ExitStatus, Error>>,
    name: Name,
    bin: BinSettings,
    killer: Option<oneshot::Sender<()>>,
}

impl RunContext {
    fn start(name: Name, bin: BinSettings) -> Self {
        let (tx, rx) = oneshot::channel();
        let handle = runtime::spawn(run_command(name.clone(), bin.clone(), rx));
        Self {
            handle,
            name,
            bin,
            killer: Some(tx),
        }
    }

    async fn end(&mut self) -> Result<(), Error> {
        if let Some(killer) = self.killer.take() {
            if let Err(_) = killer.send(()) {
                log::error!("Can't send termination signal to '{}'", self.name);
            }
            (&mut self.handle).await;
        } else {
            log::error!("Attempt to call end twice for '{}'", self.name);
        }
        Ok(())
    }
}

struct Supervisor {
    processes: HashMap<Name, RunContext>,
}

impl Supervisor {
    fn new() -> Self {
        Self {
            processes: HashMap::new(),
        }
    }

    async fn apply_config(&mut self, config: Settings) {
        for (name, bin) in config.bins {
            let entry = self.processes.entry(name.clone());
            match entry {
                Entry::Occupied(mut entry) => {
                    let context = entry.get_mut();
                    if context.bin != bin {
                        log::debug!("Restarting process '{}'...", name);
                        context.end().await;
                        let new_context = RunContext::start(name.clone(), bin);
                        *context = new_context;
                    } else {
                        log::debug!("Process '{}' already started", name);
                    }
                }
                Entry::Vacant(entry) => {
                    let context = RunContext::start(name.clone(), bin);
                    entry.insert(context);
                }
            }
        }
    }

    async fn terminate(&mut self) {
        for (name, mut proc) in self.processes.drain() {
            log::info!("Finishing the process '{}'", name);
            // TODO: Add timeout and kill force quit
            proc.end().await;
        }
    }
}

#[runtime::main(runtime_tokio::Tokio)]
async fn main() -> Result<(), Error> {
    env_logger::try_init()?;
    let mut ctrl_c = Signal::new(SIGINT).flatten_stream().compat().fuse();
    let mut hups = Signal::new(SIGHUP).flatten_stream().compat().fuse();

    let mut supervisor = Supervisor::new();
    let config = settings::Settings::parse()?;
    supervisor.apply_config(config).await;
    loop {
        select! {
            _sigint = ctrl_c.next() => {
                break;
            }
            _sighup = hups.next() => {
                log::info!("Reloading configuration...");
                let config = settings::Settings::parse();
                match config {
                    Ok(config) => {
                        supervisor.apply_config(config).await;
                    }
                    Err(err) => {
                        log::error!("Can't load or parse config: {}", err);
                    }
                }
            }
        }
    }
    log::debug!("Terminating...");
    supervisor.terminate().await;
    Ok(())
}
