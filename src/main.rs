//! notchd: a notification centre shown through a dynamic-island pill. `notchd` runs the daemon; any other
//! subcommand talks to a running one over its localhost API.

mod cli;
mod config;
mod geom;
mod model;
mod monitors;
mod panel;
mod server;
mod spring;
#[allow(dead_code)]
mod winapi;

use std::net::TcpListener;
use std::sync::Arc;

use gpui::Application;

use model::Store;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = if args.is_empty() {
        daemon()
    } else {
        cli::run(&args)
    };
    if let Err(err) = result {
        eprintln!("{err:#}");
        std::process::exit(1);
    }
}

fn daemon() -> anyhow::Result<()> {
    let port = config::port();
    // Binding first doubles as the single-instance check.
    let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|err| {
        anyhow::anyhow!("cannot listen on 127.0.0.1:{port} (already running?): {err}")
    })?;
    let token = config::token()?;
    let store = Arc::new(Store::default());
    let (tx, rx) = flume::unbounded();

    let server_store = store.clone();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        if let Err(err) = runtime.block_on(server::serve(listener, server_store, tx, token)) {
            eprintln!("server stopped: {err:#}");
            std::process::exit(1);
        }
    });
    println!("notchd listening on 127.0.0.1:{port}");

    Application::new().run(move |cx| {
        let Some(monitor) = monitors::primary() else {
            eprintln!("no display found");
            cx.quit();
            return;
        };
        let Some(island) = panel::open_island(cx, &monitor, store) else {
            eprintln!("could not open the island window");
            cx.quit();
            return;
        };
        cx.spawn(async move |cx| {
            while let Ok(cmd) = rx.recv_async().await {
                let _ = cx.update(|cx| {
                    let _ = island.update(cx, |view, _, cx| view.handle(cmd, cx));
                });
            }
        })
        .detach();
    });
    Ok(())
}
