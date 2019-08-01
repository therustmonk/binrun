use config::{ConfigError, Config, File, Environment};
use serde::Deserialize;
use std::collections::HashMap;


#[derive(Debug, Deserialize, PartialEq, Eq, Hash)]
pub struct Name(String);

#[derive(Debug, Deserialize)]
pub struct BinSettings {
    path: String,
}

#[derive(Debug, Deserialize)]
pub struct Settings {
    bin: HashMap<Name, BinSettings>,
}

impl Settings {
    pub fn parse() -> Result<Self, ConfigError> {
        let crate_name = env!("CARGO_PKG_NAME");
        let mut s = Config::new();
        s.merge(File::with_name(crate_name).required(true))?;
        s.merge(Environment::with_prefix(crate_name))?;
        s.try_into()
    }
}
