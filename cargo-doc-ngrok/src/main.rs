use std::{
    io,
    path::PathBuf,
    process::Stdio,
    sync::Arc,
};

use axum::BoxError;
use clap::{
    Args,
    Parser,
    Subcommand,
};
use hyper::service::service_fn;
use hyper_util::{
    rt::{
        TokioExecutor,
        TokioIo,
    },
    server,
};
use ngrok::{
    Agent,
    EndpointOptions,
};
use watchexec::{
    Watchexec,
    action::{
        Action,
        Outcome,
    },
    command::Command,
    config::{
        InitConfig,
        RuntimeConfig,
    },
    error::CriticalError,
    handler::PrintDebug,
    signal::source::MainSignal,
};

#[derive(Parser, Debug)]
struct Cargo {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    DocNgrok(DocNgrok),
}

#[derive(Debug, Args)]
struct DocNgrok {
    #[arg(short)]
    package: Option<String>,

    #[arg(long, short)]
    domain: Option<String>,

    /// Restrict access to only your current public IP address.
    #[arg(long)]
    restrict_ip: bool,

    #[arg(long, short)]
    watch: bool,

    #[arg(last = true)]
    doc_args: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let Cmd::DocNgrok(args) = Cargo::parse().cmd;

    std::process::Command::new("cargo")
        .arg("doc")
        .args(args.doc_args.iter())
        .stderr(Stdio::inherit())
        .stdout(Stdio::inherit())
        .spawn()?
        .wait()?;

    let meta = cargo_metadata::MetadataCommand::new().exec()?;

    let default_package = args
        .package
        .or_else(|| meta.root_package().map(|p| p.name.clone()))
        .or_else(|| {
            meta.workspace_packages()
                .first()
                .map(|p| p.name.clone())
        })
        .ok_or("No default package found. You must provide one with -p")?;
    let root_dir = meta.workspace_root;
    let target_dir = meta.target_directory;
    let doc_dir = target_dir.join("doc");

    let agent = Agent::builder().authtoken_from_env().build().await?;

    let mut opts_builder = EndpointOptions::builder();
    if let Some(domain) = args.domain {
        opts_builder = opts_builder.url(format!("https://{domain}"));
    }
    if args.restrict_ip {
        let ip = get_public_ip()?;
        println!("restricting access to IP: {ip}");
        let policy = format!(
            r#"on_http_request:
  - actions:
      - type: restrict-ips
        config:
          enforce: true
          allow:
            - "{ip}/32""#
        );
        opts_builder = opts_builder.traffic_policy(policy);
    }

    let mut listener = agent.listen(opts_builder.build()).await?;

    let doc_root = format!("/{}/", default_package.replace('-', "_"));
    let service = service_fn(move |req| {
        let doc_root = doc_root.clone();
        let stat = hyper_staticfile::Static::new(&doc_dir);
        async move {
            if req.uri().path() == "/" {
                Ok(hyper::Response::builder()
                    .status(301)
                    .header("Location", doc_root)
                    .body(hyper_staticfile::Body::Empty)
                    .unwrap())
            } else {
                stat.serve(req).await
            }
        }
    });

    let base_url = listener.url().as_str().trim_end_matches('/');
    println!(
        "serving docs on: {}/{}/",
        base_url,
        default_package.replace('-', "_")
    );

    let server = async move {
        let (dropref, waiter) = awaitdrop::awaitdrop();

        // Continuously accept new connections.
        while let Ok((conn, _addr)) = listener.accept().await {
            let service = service.clone();
            let dropref = dropref.clone();
            // Spawn a task to handle the connection. That way we can multiple connections
            // concurrently.
            tokio::spawn(async move {
                if let Err(err) = server::conn::auto::Builder::new(TokioExecutor::new())
                    .serve_connection(TokioIo::new(conn), service)
                    .await
                {
                    eprintln!("failed to serve connection: {err:#}");
                }
                drop(dropref);
            });
        }

        // Wait until all children have finished, not just the listener.
        drop(dropref);
        waiter.await;

        Ok::<(), BoxError>(())
    };

    if args.watch {
        let we = make_watcher(args.doc_args, root_dir, target_dir)?;

        we.main().await??;
    } else {
        server.await?;
    }

    Ok(())
}

/// Fetch our public IPv4 address from <https://ipv4.ngrok.com>.
fn get_public_ip() -> Result<String, BoxError> {
    let body: serde_json::Value = ureq::get("https://ipv4.ngrok.com")
        .call()?
        .body_mut()
        .read_json()?;
    let ip = body["remoteAddress"]
        .as_str()
        .ok_or("ipv4.ngrok.com response missing remoteAddress field")?
        .to_string();
    if ip.is_empty() {
        return Err("failed to determine public IP from ipv4.ngrok.com".into());
    }
    Ok(ip)
}

fn make_watcher(
    args: Vec<String>,
    root_dir: impl Into<PathBuf>,
    target_dir: impl Into<PathBuf>,
) -> Result<Arc<Watchexec>, Box<CriticalError>> {
    let target_dir = target_dir.into();
    let root_dir = root_dir.into();
    let mut init = InitConfig::default();
    init.on_error(PrintDebug(std::io::stderr()));

    let mut runtime = RuntimeConfig::default();
    runtime.pathset([root_dir]);
    runtime.command(Command::Exec {
        prog: "cargo".into(),
        args: [String::from("doc")].into_iter().chain(args).collect(),
    });
    runtime.on_action({
        move |action: Action| {
            let target_dir = target_dir.clone();
            async move {
                let sigs = action
                    .events
                    .iter()
                    .flat_map(|event| event.signals())
                    .collect::<Vec<_>>();
                if sigs.iter().any(|sig| sig == &MainSignal::Interrupt) {
                    action.outcome(Outcome::Exit);
                } else if action
                    .events
                    .iter()
                    .any(|e| e.paths().any(|(p, _)| !p.starts_with(&target_dir)))
                {
                    action.outcome(Outcome::if_running(
                        Outcome::both(Outcome::Stop, Outcome::Start),
                        Outcome::Start,
                    ));
                }

                Result::<_, io::Error>::Ok(())
            }
        }
    });
    Watchexec::new(init, runtime).map_err(Box::new)
}
