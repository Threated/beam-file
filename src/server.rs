use std::{io, net::SocketAddr, sync::Arc};

use axum::{
    extract::{BodyStream, Path, State}, headers::{authorization, Authorization}, http::{header, HeaderMap, HeaderName, StatusCode}, routing::post, Router, TypedHeader
};
use beam_lib::AppId;
use futures_util::TryStreamExt as _;
use tokio_util::io::StreamReader;

use crate::{FileMetadata, BEAM_CLIENT, CONFIG};

pub async fn serve(addr: &SocketAddr, api_key: &str) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/send/:to", post(send_file))
        .with_state(Arc::from(api_key));
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
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
    body: BodyStream,
) -> Result<(), StatusCode> {
    if auth.password() != api_key.as_ref() {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let to = AppId::new_unchecked(format!(
        "{}.{other_proxy_name}.{}",
        CONFIG.beam_id.app_name(),
        CONFIG.beam_id.as_ref().splitn(3, '.').nth(2).expect("Invalid app id")
    ));
    const RELEVANT_HEADERS: [HeaderName; 5] = [
        header::CONTENT_LENGTH,
        header::CONTENT_DISPOSITION,
        header::CONTENT_ENCODING,
        header::CONTENT_TYPE,
        header::HeaderName::from_static("metadata")
    ];
    let related_headers = headers
        .into_iter()
        .filter_map(|(maybe_k, v)| {
            if let Some(k) = maybe_k {
                RELEVANT_HEADERS.contains(&k).then_some((k, v))
            } else {
                None
            }
        })
        .collect();
    let mut conn = BEAM_CLIENT
        .create_socket_with_metadata(&to, FileMetadata { related_headers })
        .await
        .map_err(|e| {
            eprintln!("Failed to tunnel request: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    tokio::spawn(async move {
        let mut reader = StreamReader::new(body.map_err(|err| io::Error::new(io::ErrorKind::Other, err)));
        if let Err(e) = tokio::io::copy(&mut reader, &mut conn).await {
            // TODO: Some of these are normal find out which
            eprintln!("Error sending file: {e}")
        }
    });
    Ok(())
}
