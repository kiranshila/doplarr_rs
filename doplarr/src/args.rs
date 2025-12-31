use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
pub struct Cli {
    #[arg(value_name = "FILE", default_value = "config.toml")]
    pub config_file: Option<PathBuf>,
}
