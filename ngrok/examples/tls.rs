use std::{
    convert::Infallible,
    error::Error,
    net::SocketAddr,
};

use axum::{
    extract::ConnectInfo,
    routing::get,
    BoxError,
    Router,
};
use futures::TryStreamExt;
use hyper::{
    body::Incoming,
    Request,
};
use hyper_util::{
    rt::TokioExecutor,
    server,
};
use ngrok::{
    conn::ConnInfo,
    Endpoint,
};
use tower::{
    util::ServiceExt,
    Service,
};

// Note: TLS termination (formerly `.termination()`) is now configured via
// traffic policy rather than the listener builder. For example, you can add
// `.traffic_policy(policy)` to the listen builder with a policy that handles
// TLS termination. See https://ngrok.com/docs/http/traffic-policy/ for details.

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    // build our application with a single route
    let app = Router::new().route(
        "/",
        get(
            |ConnectInfo(remote_addr): ConnectInfo<SocketAddr>| async move {
                format!("Hello, {remote_addr:?}!\r\n")
            },
        ),
    );

    let agent = ngrok::Agent::builder().authtoken_from_env().build()?;

    let mut listener = agent
        .listen()
        .url("tls://example.ngrok.app")
        .metadata("example tunnel metadata from rust")
        .start()
        .await?;

    println!("TLS listener started on URL: {:?}", listener.url());

    let mut make_service = app.into_make_service_with_connect_info::<SocketAddr>();

    let server = async move {
        while let Some(conn) = listener.try_next().await? {
            let remote_addr = conn.remote_addr();
            let tower_service = unwrap_infallible(make_service.call(remote_addr).await);

            tokio::spawn(async move {
                let hyper_service =
                    hyper::service::service_fn(move |request: Request<Incoming>| {
                        tower_service.clone().oneshot(request)
                    });

                if let Err(err) = server::conn::auto::Builder::new(TokioExecutor::new())
                    .serve_connection_with_upgrades(conn, hyper_service)
                    .await
                {
                    eprintln!("failed to serve connection: {err:#}");
                }
            });
        }
        Ok::<(), BoxError>(())
    };

    server.await?;

    Ok(())
}

fn unwrap_infallible<T>(result: Result<T, Infallible>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => match err {},
    }
}
