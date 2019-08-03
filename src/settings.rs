use config::{Config, ConfigError, Environment, File};
use serde::Deserialize;
use std::collections::HashMap;

pub type Name = String;

#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct BinSettings {
    pub path: String,
    pub env: HashMap<String, String>,
}

#[derive(Deserialize, Debug)]
pub struct Settings {
    pub bins: HashMap<Name, BinSettings>,
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
