use std::{convert::Infallible, path::PathBuf};

use beam_lib::{reqwest::Url, AppId};
use clap::{Parser, Subcommand, ValueHint};

/// Samply.Beam.File
#[derive(Debug, Parser)]
pub struct Config {
    /// Url of the local beam proxy which is required to have sockets enabled
    #[clap(env, long, default_value = "http://beam-proxy:8081")]
    pub beam_url: Url,

    /// Beam api key
    #[clap(env, long)]
    pub beam_secret: String,

    /// The app id of this application
    #[clap(long, env, value_parser=|id: &str| Ok::<_, Infallible>(AppId::new_unchecked(id)))]
    pub beam_id: AppId,

    #[clap(subcommand)]
    pub mode: Mode,
}

#[derive(Subcommand, Debug)]
pub enum Mode {
    Send {
        /// Name of the receiving proxy
        #[clap(long)]
        to: String,

        #[clap(value_hint = ValueHint::FilePath)]
        file: PathBuf
    },
    /// Receive files from other Samply.Beam.File instances
    Receive {
        #[clap(subcommand)]
        mode: ReceiveMode,

        /// Only receive count files
        #[clap(long, short = 'n', default_value_t = u32::MAX, hide_default_value = true)]
        count: u32
    },
    #[cfg(feature = "server")]
    Server {
        /// Address the server should bind to
        #[clap(env, long, default_value = "0.0.0.0:8080")]
        bind_addr: std::net::SocketAddr,

        /// Api key required for uploading files
        #[clap(env, long)]
        api_key: String,
    }
}

#[derive(Subcommand, Debug)]
pub enum ReceiveMode {
    Print,
    Save {
        /// Directory files will be written to
        #[clap(long, short = 'o', value_hint = ValueHint::DirPath)]
        outdir: PathBuf,
    },
    Callback {
        /// A url to an endpoint that will be called when we are receiving a new file
        #[clap(value_hint = ValueHint::Url)]
        url: Url,
    }
}
