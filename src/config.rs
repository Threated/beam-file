use std::{net::SocketAddr, convert::Infallible};

use beam_lib::{reqwest::Url, AppId};
use clap::Parser;

/// Samply.Beam.File
#[derive(Debug, Parser)]
pub struct Config {
    /// Address the server should bind to
    #[clap(env, long, default_value = "0.0.0.0:8080")]
    pub bind_addr: SocketAddr,

    /// Url of the local beam proxy which is required to have sockets enabled
    #[clap(env, long, default_value = "http://beam-proxy:8081")]
    pub beam_url: Url,

    /// Beam api key
    #[clap(env, long)]
    pub beam_secret: String,

    /// Api key required for uploading files
    #[clap(env, long)]
    pub api_key: String,

    /// The app id of this application
    #[clap(long, env, value_parser=|id: &str| Ok::<_, Infallible>(AppId::new_unchecked(id)))]
    pub beam_id: AppId,

    /// A url to an endpoint that will be called when we are receiving a new file
    #[clap(long, env)]
    pub callback: Option<Url>,
}
