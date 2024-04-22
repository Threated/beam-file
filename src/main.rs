mod config;
#[cfg(feature = "server")]
mod server;

use std::{path::Path, process::ExitCode, time::SystemTime};

use beam_lib::{AppId, BeamClient, BlockingOptions, SocketTask};
use clap::Parser;
use config::{Config, SendArgs, Mode, ReceiveMode};
use futures_util::{FutureExt, Stream, StreamExt, TryFutureExt, TryStreamExt};
use once_cell::sync::Lazy;
use reqwest::{Client, Upgraded, Url};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncRead;
use tokio_util::io::ReaderStream;
use reqwest::header::{HeaderName, HeaderValue, HeaderMap};
use anyhow::{anyhow, bail, Context, Result};

pub static CONFIG: Lazy<Config> = Lazy::new(Config::parse);

pub static BEAM_CLIENT: Lazy<BeamClient> = Lazy::new(|| {
    BeamClient::new(
        &CONFIG.beam_id,
        &CONFIG.beam_secret,
        CONFIG.beam_url.clone(),
    )
});
pub static CLIENT: Lazy<Client> = Lazy::new(Client::new);

#[tokio::main]
async fn main() -> ExitCode {
    let work = match &CONFIG.mode {
        Mode::Send(send_args) if send_args.file.to_string_lossy() == "-" => send_file(tokio::io::stdin(), send_args).boxed(),
        Mode::Send(send_args) => tokio::fs::File::open(&send_args.file)
            .err_into()
            .and_then(|f| send_file(f, send_args))
            .boxed(),
        Mode::Receive { count, mode } => stream_tasks()
            .and_then(connect_socket)
            .inspect_ok(|(task, _)| eprintln!("Receiving file from: {}", task.from))
            .and_then(move |(task, inc)| match mode {
                ReceiveMode::Print => print_file(task, inc).boxed(),
                ReceiveMode::Save { outdir, naming } => save_file(outdir, task, inc, naming).boxed(),
                ReceiveMode::Callback { url } => forward_file(task, inc, url).boxed(),
            })
            .take(*count as usize)
            .for_each(|v| {
                if let Err(e) = v {
                    eprintln!("{e}");
                }
                futures_util::future::ready(())
            })
            .map(Ok)
            .boxed(),
        #[cfg(feature = "server")]
        Mode::Server { bind_addr, api_key} => server::serve(bind_addr, api_key).boxed(),
    };
    let result = tokio::select! {
        res = work => res,
        _ = tokio::signal::ctrl_c() => {
            eprintln!("Shutting down");
            return ExitCode::from(130);
        }
    };
    if let Err(e) = result {
        eprintln!("Failure: {e}");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

pub async fn save_file(dir: &Path, socket_task: SocketTask, mut incoming: impl AsyncRead + Unpin, naming_scheme: &str) -> Result<()> {
    let ts = SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let from = socket_task.from.as_ref().split('.').take(2).collect::<Vec<_>>().join(".");
    let meta: FileMeta = serde_json::from_value(socket_task.metadata).context("Failed to deserialize metadata")?;
    let filename = naming_scheme
        .replace("%f", &from)
        .replace("%t", &ts.to_string())
        // Save because deserialize implementation of suggested_name does path traversal check
        .replace("%n", meta.suggested_name.as_ref().map(String::as_str).unwrap_or(""));
    let mut file = tokio::fs::File::create(dir.join(filename)).await?;
    tokio::io::copy(&mut incoming, &mut file).await?;
    Ok(())
}

async fn send_file(mut stream: impl AsyncRead + Unpin, meta @ SendArgs { to, .. }: &SendArgs) -> Result<()> {
    let to = AppId::new_unchecked(format!(
        "{to}.{}",
        CONFIG.beam_id.as_ref().splitn(3, '.').nth(2).expect("Invalid app id")
    ));
    let mut conn = BEAM_CLIENT
        .create_socket_with_metadata(&to, meta.to_file_meta())
        .await?;
    tokio::io::copy(&mut stream, &mut conn).await?;
    Ok(())
}

pub fn stream_tasks() -> impl Stream<Item = Result<SocketTask>> {
    static BLOCK: Lazy<BlockingOptions> = Lazy::new(|| BlockingOptions::from_count(1));
    futures_util::stream::repeat_with(move || {
        BEAM_CLIENT.get_socket_tasks(&BLOCK)
    }).filter_map(|v| async {
        match v.await {
            Ok(mut v) => Some(Ok(v.pop()?)),
            Err(e) => Some(Err(anyhow::Error::from(e)).context("Failed to get socket tasks from beam")),
        }
    })
}

pub async fn connect_socket(socket_task: SocketTask) -> Result<(SocketTask, Upgraded)> {
    let id = socket_task.id;
    Ok((socket_task, BEAM_CLIENT.connect_socket(&id).await.with_context(|| format!("Failed to connect to socket {id}"))?))
}

pub async fn forward_file(socket_task: SocketTask, incoming: impl AsyncRead + Unpin + Send + 'static, cb: &Url) -> Result<()> {
    let FileMeta { suggested_name, meta } = serde_json::from_value(socket_task.metadata).context("Failed to deserialize metadata")?;
    let mut headers = HeaderMap::with_capacity(2);
    if let Some(meta) = meta {
        headers.append(HeaderName::from_static("metadata"), HeaderValue::from_bytes(&serde_json::to_vec(&meta)?)?);
    }
    if let Some(name) = suggested_name {
        headers.append(HeaderName::from_static("filename"), HeaderValue::from_str(&name)?);
    }
    let res = CLIENT
        .post(cb.clone())
        .headers(headers)
        .body(reqwest::Body::wrap_stream(ReaderStream::new(incoming)))
        .send()
        .await;
    match res {
        Ok(r) if !r.status().is_success() => bail!("Got unsuccessful status code from callback server: {}", r.status()),
        Err(e) => bail!("Failed to send file to {cb}: {e}"),
        _ => Ok(())
    }
}

pub async fn print_file(socket_task: SocketTask, mut incoming: impl AsyncRead + Unpin) -> Result<()> {
    eprintln!("Incoming file from {}", socket_task.from);
    tokio::io::copy(&mut incoming, &mut tokio::io::stdout()).await?;
    eprintln!("Done printing file from {}", socket_task.from);
    Ok(())
}


fn validate_filename(name: &str) -> Result<&str> {
    if name.chars().all(|c| c.is_alphanumeric() || ['_', '.', '-'].contains(&c)) {
        Ok(name)
    } else {
        Err(anyhow!("Invalid filename: {name}"))
    }
}

fn deserialize_filename<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    let s = Option::<String>::deserialize(deserializer)?;
    if let Some(ref f) = s {
        validate_filename(&f).map_err(serde::de::Error::custom)?;
    }
    Ok(s)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FileMeta {
    #[serde(deserialize_with = "deserialize_filename")]
    suggested_name: Option<String>,

    meta: Option<serde_json::Value>,
}
