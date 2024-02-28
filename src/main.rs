mod config;
#[cfg(feature = "server")]
mod server;

use std::{path::PathBuf, process::ExitCode, time::SystemTime};

use beam_lib::{AppId, BeamClient, BlockingOptions, SocketTask};
use clap::Parser;
use config::{Config, Mode, ReceiveMode};
use futures_util::{FutureExt, Stream, StreamExt, TryFutureExt, TryStreamExt};
use once_cell::sync::Lazy;
use reqwest::{Client, Upgraded, Url};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncRead;
use tokio_util::io::ReaderStream;
use reqwest::header::HeaderMap;
use anyhow::{bail, Context, Result};

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
        Mode::Send { to, file } if file.to_string_lossy() == "-" => send_file(to, tokio::io::stdin(), FileMetadata::default()).boxed(),
        Mode::Send { to, file } => tokio::fs::File::open(file)
            .err_into()
            .and_then(|f| send_file(to, f, FileMetadata::default()))
            .boxed(),
        Mode::Receive { count, mode } => stream_tasks()
            .and_then(connect_socket)
            .and_then(move |(task, inc)| match mode {
                ReceiveMode::Print => print_file(task, inc).boxed(),
                ReceiveMode::Save { outdir } => save_file(outdir, task, inc).boxed(),
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
            println!("Shutting down");
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

pub async fn save_file(dir: &PathBuf, socket_task: SocketTask, mut incoming: impl AsyncRead + Unpin) -> Result<()> {
    let ts = SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let fname = dir.join(&format!("{ts}_{}", socket_task.from));
    let mut file = tokio::fs::File::create(fname).await?;
    tokio::io::copy(&mut incoming, &mut file).await?;
    Ok(())
}

async fn send_file(to: &str, mut stream: impl AsyncRead + Unpin, meta: FileMetadata) -> Result<()> {
    // TODO: rethink this
    let to = AppId::new_unchecked(format!(
        "{}.{to}.{}",
        CONFIG.beam_id.app_name(),
        CONFIG.beam_id.as_ref().splitn(3, '.').nth(2).expect("Invalid app id")
    ));
    let mut conn = BEAM_CLIENT
        .create_socket_with_metadata(&to, meta)
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
    Ok((socket_task, BEAM_CLIENT.connect_socket(&id).await.context("Failed to connect to socket")?))
}

pub async fn forward_file(socket_task: SocketTask, incoming: impl AsyncRead + Unpin + Send + 'static, cb: &Url) -> Result<()> {
    let FileMetadata { related_headers } = serde_json::from_value(socket_task.metadata).context("Failed to deserialize metadata")?;
    let res = CLIENT
        .post(cb.clone())
        .headers(related_headers)
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
    println!("Printing file from {}", socket_task.from);
    tokio::io::copy(&mut incoming, &mut tokio::io::stdout()).await?;
    println!("Done printing file from {}", socket_task.from);
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct FileMetadata {
    #[serde(with = "http_serde::header_map")]
    related_headers: HeaderMap,
}

