use std::{net::SocketAddr, sync::Arc};

use axum::{
    extract::{Path, State}, http::{HeaderMap, StatusCode}, routing::post, Router
};
use axum_extra::{headers::{authorization, Authorization}, TypedHeader};
use beam_lib::{AppId, MsgId, RawString, TaskRequest};
use tokio::net::TcpListener;

use crate::{FileMeta, BEAM_CLIENT, CONFIG};

pub async fn serve(addr: &SocketAddr, api_key: &str) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/send/:to", post(send_file))
        .with_state(Arc::from(api_key));
    axum::serve(TcpListener::bind(&addr).await? ,app.into_make_service())
        .with_graceful_shutdown(async { tokio::signal::ctrl_c().await.unwrap() })
        .await?;
    Ok(())
}

type AppState = Arc<str>;

async fn send_file(
    Path(other_proxy_name): Path<String>,
    auth: TypedHeader<Authorization<authorization::Basic>>,
    headers: HeaderMap,
    State(api_key): State<AppState>,
    req: String,
) -> Result<(), StatusCode> {
    if auth.password() != api_key.as_ref() {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let to = AppId::new_unchecked(format!(
        "{other_proxy_name}.{}",
        CONFIG.beam_id.as_ref().splitn(3, '.').nth(2).expect("Invalid app id")
    ));
    let meta = FileMeta {
        meta: headers.get("metadata").and_then(|v| serde_json::from_slice(v.as_bytes()).map_err(|e| eprintln!("Failed to deserialize metadata: {e}. Skipping metadata")).ok()),
        suggested_name: headers.get("filename").and_then(|v| v.to_str().map(Into::into).ok()),
    };
    let task = TaskRequest {
        id: MsgId::new(),
        from: CONFIG.beam_id.clone(),
        to: vec![to],
        body: RawString(req),
        ttl: "30s".to_string(),
        failure_strategy: beam_lib::FailureStrategy::Discard,
        metadata: serde_json::to_value(meta).unwrap(),
    };
    BEAM_CLIENT
        .post_task(&task)
        .await
        .map_err(|e| {
            eprintln!("Failed to tunnel request: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(())
}
