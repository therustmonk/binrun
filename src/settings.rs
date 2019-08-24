use config::{Config, ConfigError, Environment, File};
use serde::Deserialize;
use std::collections::HashMap;

pub type Name = String;

#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct BinSettings {
    pub name: Name,
    pub active: Option<bool>,
    pub command: String,
    pub workdir: Option<String>,
    pub args: Option<String>,
    pub env: Option<HashMap<String, String>>,
    pub wait: Option<u64>,
    pub delay: Option<u64>,
}

#[derive(Deserialize, Debug)]
pub struct Settings {
    pub bins: Vec<BinSettings>,
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
