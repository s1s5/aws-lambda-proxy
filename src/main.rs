use axum::{
    Router,
    extract::Request,
    http::{HeaderName, Response, StatusCode},
    response::IntoResponse,
};
use base64::Engine;
use base64::engine::general_purpose;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::{collections::HashMap, env, net::SocketAddr};
use tokio::signal;
use tokio::signal::unix::{SignalKind, signal};
use tower::ServiceBuilder;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LambdaResponse {
    status_code: u16,
    headers: HashMap<String, String>,
    body: String,
    is_base64_encoded: Option<bool>,
}

impl LambdaResponse {
    fn body(&self) -> Vec<u8> {
        if self.is_base64_encoded.unwrap_or(false) {
            general_purpose::STANDARD.decode(self.body.clone()).unwrap()
        } else {
            self.body.clone().into_bytes()
        }
    }
}

async fn handle_all(req: Request) -> impl IntoResponse {
    let method = req.method().to_string();
    let path = req.uri().path().to_string();

    let headers: HashMap<_, _> = match req
        .headers()
        .iter()
        .map(|(name, value)| {
            String::from_utf8(value.as_bytes().to_vec()).map(|x| (name.to_string(), x))
        })
        .collect()
    {
        Ok(r) => r,
        Err(err) => {
            eprintln!("Error reading headers: {}", err);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Error reading request headers.".to_string(),
            )
                .into_response();
        }
    };

    let query_string = req.uri().query().unwrap_or("").to_string();
    let query: HashMap<_, _> =
        url::form_urlencoded::parse(req.uri().query().unwrap_or("").as_bytes())
            .into_owned()
            .collect();

    let body_bytes = match axum::body::to_bytes(req.into_body(), usize::MAX).await {
        Ok(bytes) => bytes.to_vec(),
        Err(e) => {
            eprintln!("Error reading body: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Error reading request body.".to_string(),
            )
                .into_response();
        }
    };

    let now: DateTime<Utc> = Utc::now();
    let formatted_time = now.format("%d/%b/%Y:%H:%M:%S %z").to_string();

    // TODO: cookie not supported
    let body = serde_json::json!({
      "version": "2.0",
      "routeKey": "$default",
      "rawPath": path.clone(),
      "rawQueryString": query_string,
      "cookies": [
        "cookie1=value1",
        "cookie2=value2"
      ],
      "headers": headers,
      "queryStringParameters": query,
      "requestContext": {
        "accountId": "anonymous",
        "apiId": "xxxxxxxxxx",
        "domainName": "xxxxxxxxxx.lambda-url.ap-northeast-1.on.aws",
        "domainPrefix": "xxxxxxxxxx",
        "http": {
          "method": method.clone(),
          "path": path,
          "protocol": "HTTP/1.1",
          "sourceIp": "1.2.3.4",
          "userAgent": "curl/7.81.0"
        },
        "requestId": "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx",
        "routeKey": "$default",
        "stage": "$default",
        "time": formatted_time,
        "timeEpoch": now.timestamp_millis(),
      },
      "body": general_purpose::STANDARD.encode(&body_bytes),
      "isBase64Encoded": true
    });

    // TODO: 最大サイズ確認
    let response = reqwest::Client::new()
        .request(
            reqwest::Method::POST,
            env::var("BACKEND").expect("BACKEND is not set"),
        )
        .body(serde_json::to_vec(&body).unwrap())
        .send()
        .await
        .unwrap();

    if response.status().is_server_error() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Error calling backend.".to_string(),
        )
            .into_response();
    }

    let lambda_response: LambdaResponse =
        serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();

    let mut r: Response<axum::body::Body> = Response::builder()
        .status(lambda_response.status_code)
        .body(lambda_response.body().into())
        .unwrap();
    lambda_response.headers.into_iter().for_each(|(k, v)| {
        r.headers_mut().insert(
            HeaderName::try_from(k.as_str()).unwrap(),
            v.parse().unwrap(),
        );
    });

    r
}

// --- メイン関数 ---

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "axum_all_paths=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // ルーティングを設定
    let app = Router::new()
        // ルーティングにマッチしなかったすべてを handle_all にフォールバックさせる
        .fallback(handle_all)
        // Tower ServiceBuilderを使用してミドルウェアを追加 (例: ロギング)
        .layer(ServiceBuilder::new().layer(tower_http::trace::TraceLayer::new_for_http()));

    let addr = SocketAddr::from(([0, 0, 0, 0], 8000));
    tracing::debug!("listening on {}", addr);

    // サーバーを起動
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal()) // ここでシャットダウンシグナルを渡す
        .await
        .unwrap();

    // グレースフルシャットダウンが完了すると、この下のコードが実行される
    println!("Server has shut down.");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)] // Linux/macOSなどのUnix系OSでSIGTERMを処理
    let terminate = async {
        signal(SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))] // Unix系OS以外では何もしない
    let terminate = std::future::pending::<()>();

    // `ctrl_c`または`terminate`のどちらかが先に完了するのを待つ
    tokio::select! {
        _ = ctrl_c => {
            eprintln!("SIGINT (Ctrl+C) received. Starting graceful shutdown...");
        },
        _ = terminate => {
            eprintln!("SIGTERM received. Starting graceful shutdown...");
        },
    }
}
