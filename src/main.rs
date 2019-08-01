#![feature(async_await)]

mod settings;

use failure::Error;
use futures_legacy::prelude::*;

#[runtime::main(runtime_tokio::Tokio)]
async fn main() -> Result<(), Error> {
    let settings = settings::Settings::parse()?;
    let ctrl_c = tokio_signal::ctrl_c().flatten_stream();
    env_logger::try_init()?;
    Ok(())
}
