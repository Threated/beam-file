mod config;
#[cfg(feature = "server")]
mod server;

use std::time::Duration;
use std::{path::Path, process::ExitCode, time::SystemTime};

use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use beam_lib::{AppId, BeamClient, BlockingOptions, MsgId, RawString, TaskRequest, TaskResult};
use clap::Parser;
use config::{Config, Mode, ReceiveMode, SendArgs};
use futures_util::{FutureExt, Stream, StreamExt, TryFutureExt, TryStreamExt};
use once_cell::sync::Lazy;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
        Mode::Send(send_args) if send_args.file.to_string_lossy() == "-" => async {
            let mut buf = Vec::new();
            tokio::io::stdin().read_to_end(&mut buf).await?;
            send_file(&buf, send_args).await
        }.boxed(),
        Mode::Send(send_args) => tokio::fs::File::open(&send_args.file)
            .err_into()
            .and_then(|mut f| {
                let send_args = send_args.clone();
                async move {
                    let mut buf = Vec::new();
                    f.read_to_end(&mut buf).await?;
                    send_file(&buf, &send_args).await
            }}).boxed(),
        Mode::Receive { count, mode } => stream_tasks()
            .inspect_ok(|task| eprintln!("Receiving file from: {}", task.from))
            .and_then(move |task| {
                let id = task.id;
                let mut task_res = TaskResult {
                    from: CONFIG.beam_id.clone(),
                    to: vec![task.from.clone()],
                    task: task.id,
                    status: beam_lib::WorkStatus::Succeeded,
                    body: "",
                    metadata: serde_json::Value::Null
                };
                async move {
                    let res = match mode {
                        ReceiveMode::Print => print_file(task).await,
                        ReceiveMode::Save { outdir, naming } => save_file(outdir, task, naming).await,
                        ReceiveMode::Callback { url } => forward_file(task, url).await,
                    };
                    if res.is_err() {
                        task_res.status = beam_lib::WorkStatus::PermFailed
                    }
                    BEAM_CLIENT.put_result(&task_res, &id).await?;
                    res
                }.boxed()
            })
            .take(*count as usize)
            .for_each(|v| async {
                if let Err(e) = v {
                    eprintln!("{e}");
                    tokio::time::sleep(Duration::from_secs(10)).await;
                }
            })
            .map(Ok)
            .boxed(),
        #[cfg(feature = "server")]
        Mode::Server { bind_addr, api_key } => server::serve(bind_addr, api_key).boxed(),
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

pub async fn save_file(dir: &Path, task: TaskRequest<Vec<u8>>, naming_scheme: &str) -> Result<()> {
    let ts = SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let from = task.from.as_ref().split('.').take(2).collect::<Vec<_>>().join(".");
    let meta: FileMeta = serde_json::from_value(task.metadata).context("Failed to deserialize metadata")?;
    let filename = naming_scheme
        .replace("%f", &from)
        .replace("%t", &ts.to_string())
        // Save because deserialize implementation of suggested_name does path traversal check
        .replace("%n", meta.suggested_name.as_deref().unwrap_or(""));
    let mut file = tokio::fs::File::create(dir.join(filename)).await?;
    file.write_all(&task.body).await?;
    Ok(())
}

async fn send_file(body: &[u8], meta @ SendArgs { to, .. }: &SendArgs) -> Result<()> {
    let to = AppId::new_unchecked(format!(
        "{to}.{}",
        CONFIG
            .beam_id
            .as_ref()
            .splitn(3, '.')
            .nth(2)
            .expect("Invalid app id")
    ));
    BEAM_CLIENT
        .post_task(&TaskRequest {
            id: MsgId::new(),
            from: CONFIG.beam_id.clone(),
            to: vec![to],
            body: RawString::from(STANDARD.encode(body)),
            ttl: "60s".to_string(),
            failure_strategy: beam_lib::FailureStrategy::Discard,
            metadata: serde_json::to_value(meta.to_file_meta())?,
        })
        .await?;
    Ok(())
}

pub fn stream_tasks() -> impl Stream<Item = Result<TaskRequest<Vec<u8>>>> {
    static BLOCK: Lazy<BlockingOptions> = Lazy::new(|| BlockingOptions::from_count(1));
    futures_util::stream::repeat_with(move || BEAM_CLIENT.poll_pending_tasks::<RawString>(&BLOCK)).filter_map(
        |v| async {
            match v.await {
                Ok(mut v) => Some(Ok(v.pop()?)),
                Err(e) => Some(
                    Err(anyhow::Error::from(e)).context("Failed to get socket tasks from beam"),
                ),
            }
        },
    ).and_then(|TaskRequest { id, from, to, body, ttl, failure_strategy, metadata }| async move {
        let body = STANDARD.decode(body.0)?;
        Ok(TaskRequest {
            id,
            from,
            to,
            body,
            ttl,
            failure_strategy,
            metadata,
        })
    })
}

pub async fn forward_file(task: TaskRequest<Vec<u8>>, cb: &Url) -> Result<()> {
    let FileMeta { suggested_name, meta } = serde_json::from_value(task.metadata).context("Failed to deserialize metadata")?;
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
        .body(task.body)
        .send()
        .await;
    match res {
        Ok(r) if !r.status().is_success() => bail!(
            "Got unsuccessful status code from callback server: {}",
            r.status()
        ),
        Err(e) => bail!("Failed to send file to {cb}: {e}"),
        _ => Ok(()),
    }
}

pub async fn print_file(task: TaskRequest<Vec<u8>>) -> Result<()> {
    eprintln!("Incoming file from {}", task.from);
    tokio::io::stdout()
        .write_all(&task.body)
        .await?;
    eprintln!("Done printing file from {}", task.from);
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
        validate_filename(f).map_err(serde::de::Error::custom)?;
    }
    Ok(s)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FileMeta {
    #[serde(deserialize_with = "deserialize_filename")]
    suggested_name: Option<String>,

    meta: Option<serde_json::Value>,
}
