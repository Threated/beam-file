use std::{convert::Infallible, path::PathBuf};

use beam_lib::{reqwest::Url, AppId};
use clap::{Parser, Subcommand, ValueHint, Args};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{validate_filename, FileMeta};

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
    /// Send files
    Send(SendArgs),
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

#[derive(Debug, Clone, Serialize, Deserialize, Default, Args)]
pub struct SendArgs {
    /// Name of the receiving beam app without broker id e.g. app1.proxy2
    #[clap(long)]
    #[serde(skip)]
    pub to: String,

    /// Name of the file to be read or '-' to read from stdin
    #[clap(value_hint = ValueHint::FilePath)]
    pub file: PathBuf,

    /// A suggestion for the new name the receiver should use. Will default to the uploaded files name if it is not read from stdin.
    #[clap(long)]
    #[serde(skip_serializing_if  = "Option::is_none")]
    name: Option<String>,

    /// Additional metadata for the file
    #[clap(long)]
    #[serde(skip_serializing_if  = "Option::is_none")]
    pub meta: Option<Value>,
}

impl SendArgs {
    fn get_suggested_name(&self) -> Option<&str> {
        self.name
            .as_deref()
            .or_else(|| (self.file.as_os_str() != "-").then_some(self.file.file_name()?.to_str()?))
            .and_then(|v| validate_filename(v).ok())
    }

    pub fn to_file_meta(&self) -> FileMeta {
        FileMeta {
            suggested_name: self.get_suggested_name().map(ToOwned::to_owned),
            meta: self.meta.clone()
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum ReceiveMode {
    Print,
    Save {
        /// Directory files will be written to
        #[clap(long, short = 'o', value_hint = ValueHint::DirPath)]
        outdir: PathBuf,

        /// Naming scheme used to save files. %t -> unix timestamp %f beam app id e.g. app1.proxy2 %n suggested name from upload see send arguments
        #[clap(long, short = 'p', default_value = "%f_%t")]
        naming: String
    },
    Callback {
        /// A url to an endpoint that will be called when we are receiving a new file
        #[clap(value_hint = ValueHint::Url)]
        url: Url,
    }
}
