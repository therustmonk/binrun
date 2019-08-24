#![feature(async_await)]

mod colorizer;
mod settings;

use crate::settings::{BinSettings, Name, Settings};
use colored::*;
use colorizer::Colorizer;
use failure::{format_err, Error};
use futures::channel::oneshot;
use futures::compat::{Future01CompatExt, Stream01CompatExt};
use futures::stream::select;
use futures::{select, FutureExt, StreamExt, TryFutureExt};
use futures_legacy::prelude::*;
use nix::sys::signal;
use nix::unistd::Pid;
use runtime::task::JoinHandle;
use runtime::time::{Delay, FutureExt as _};
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::env;
use std::io::BufReader;
use std::process::{Command, ExitStatus, Stdio};
use std::time::Duration;
use tokio_process::{Child, ChildStdout, CommandExt};
use tokio_signal::unix::{Signal, SIGHUP, SIGINT};

async fn run_command(
    color: Color,
    pale_name: Name,
    bin: BinSettings,
    killer: oneshot::Receiver<()>,
) -> Result<ExitStatus, Error> {
    // TODO: Consider to give colored name here!
    let name = pale_name.color(color);
    let active = bin.active.unwrap_or(true);
    if !active {
        killer.await;
        return Err(format_err!("{} not started", name));
    }
    log::info!("Starting '{}': {}", name, bin.command);
    if let Some(mut secs) = bin.delay {
        let one_sec = Duration::from_secs(1);
        while secs > 0 {
            log::info!("{} secs remain before starting: {}", secs, name);
            Delay::new(one_sec).await;
            secs -= 1;
        }
    }
    let mut cmd = Command::new(bin.command);
    let mut filtered_env: HashMap<String, String> = env::vars()
        .filter(|&(ref k, _)| k == "TERM" || k == "TZ" || k == "LANG" || k == "PATH")
        .collect();
    if let Some(env) = bin.env {
        let env_iter = env.into_iter().map(|(k, v)| (k.to_uppercase(), v));
        filtered_env.extend(env_iter);
        cmd.env_clear();
        log::trace!("Set env for '{}': {:?}", name, filtered_env);
        cmd.envs(&filtered_env);
    }
    if let Some(args) = bin.args {
        cmd.args(args.split_whitespace());
    }
    if let Some(dir) = bin.workdir {
        cmd.current_dir(dir);
    }
    cmd.stderr(Stdio::piped());
    cmd.stdout(Stdio::piped());
    match cmd.spawn_async() {
        Ok(mut child) => {
            log::debug!("Started: '{}'", name);
            let child_stderr = child.stderr().take();
            let child_stdout = child.stdout().take();
            match (child_stdout, child_stderr) {
                (Some(stdout), Some(stderr)) => {
                    let err_lines = tokio_io::io::lines(BufReader::new(stderr)).compat();
                    let out_lines = tokio_io::io::lines(BufReader::new(stdout)).compat();
                    let mut lines = select(out_lines, err_lines).fuse();
                    let mut killer = killer.fuse();
                    loop {
                        select! {
                            line = lines.next() => {
                                match line {
                                    Some(Ok(line)) => {
                                        let prefix = format!("{:<15}|", pale_name);
                                        println!("{} {}", prefix.color(color), line);
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
                }
                _ => {
                    log::warn!("Can't get a stdout/stderr stream of '{}'", name);
                }
            }
            let pid = Pid::from_raw(child.id() as i32);
            let end_strategy = vec![(signal::Signal::SIGINT, 15), (signal::Signal::SIGKILL, 5)];
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
    color: Color,
    name: Name,
    bin: BinSettings,
    killer: Option<oneshot::Sender<()>>,
}

impl RunContext {
    fn start(color: Color, name: Name, bin: BinSettings) -> Self {
        let (tx, rx) = oneshot::channel();
        let handle = runtime::spawn(run_command(color.clone(), name.clone(), bin.clone(), rx));
        Self {
            handle,
            color,
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
    colorizer: Colorizer,
    processes: HashMap<Name, RunContext>,
}

impl Supervisor {
    fn new() -> Self {
        Self {
            processes: HashMap::new(),
            colorizer: Colorizer::new(),
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
                        let new_context =
                            RunContext::start(context.color.clone(), name.clone(), bin);
                        *context = new_context;
                    } else {
                        log::debug!("Process '{}' already started", name);
                    }
                }
                Entry::Vacant(entry) => {
                    let color = self.colorizer.next();
                    let context = RunContext::start(color, name.clone(), bin);
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
