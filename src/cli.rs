//! `notchd notify|list|clear|dnd|state`: a thin client over the HTTP API, on plain std sockets.

use std::io::{Read, Write};
use std::net::TcpStream;

use anyhow::{Context, bail};
use serde_json::{Value, json};

use crate::config;

const USAGE: &str = "usage:
  notchd                         run the daemon
  notchd notify <title> [body] [--urgency low|normal|critical] [--timeout <ms>] [--id <id>] [--app <name>]
  notchd list | clear | state
  notchd dnd on|off
  notchd autostart on|off        start at login (the notchd.exe it is run from)
  notchd quit                    stop the daemon";

fn request(method: &str, path: &str, body: Option<&Value>) -> anyhow::Result<(u16, String)> {
    let port = config::port();
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .with_context(|| format!("notchd is not running on 127.0.0.1:{port}"))?;
    let body = body.map(Value::to_string).unwrap_or_default();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        config::token()?,
        body.len()
    )?;
    let mut raw = String::new();
    stream.read_to_string(&mut raw)?;
    let (head, rest) = raw.split_once("\r\n\r\n").context("malformed response")?;
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .context("malformed status line")?;
    Ok((status, rest.to_string()))
}

fn show(result: anyhow::Result<(u16, String)>) -> anyhow::Result<()> {
    let (status, body) = result?;
    if !(200..300).contains(&status) {
        bail!("request failed: HTTP {status}");
    }
    if !body.trim().is_empty() {
        println!("{}", body.trim());
    }
    Ok(())
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    match args.first().map(String::as_str) {
        Some("notify") => {
            let mut positional = Vec::new();
            let mut note = json!({});
            let mut rest = args[1..].iter();
            while let Some(arg) = rest.next() {
                let key = match arg.as_str() {
                    "--urgency" => "urgency",
                    "--timeout" => "timeout_ms",
                    "--id" => "id",
                    "--app" => "app",
                    _ => {
                        positional.push(arg.clone());
                        continue;
                    }
                };
                let value = rest
                    .next()
                    .with_context(|| format!("{arg} needs a value"))?;
                note[key] = if key == "timeout_ms" {
                    json!(value.parse::<u64>().context("--timeout is milliseconds")?)
                } else {
                    json!(value)
                };
            }
            let Some(title) = positional.first() else {
                bail!("{USAGE}");
            };
            note["title"] = json!(title);
            note["body"] = json!(positional.get(1).cloned().unwrap_or_default());
            show(request("POST", "/v1/notifications", Some(&note)))
        }
        Some("list") => show(request("GET", "/v1/notifications", None)),
        Some("clear") => show(request("DELETE", "/v1/notifications", None)),
        Some("state") => show(request("GET", "/v1/state", None)),
        Some("dnd") => {
            let enabled = match args.get(1).map(String::as_str) {
                Some("on") => true,
                Some("off") => false,
                _ => bail!("{USAGE}"),
            };
            show(request(
                "PUT",
                "/v1/dnd",
                Some(&json!({ "enabled": enabled })),
            ))
        }
        Some("autostart") => match args.get(1).map(String::as_str) {
            Some("on") => crate::autostart::set(true),
            Some("off") => crate::autostart::set(false),
            _ => bail!("{USAGE}"),
        },
        Some("quit") => show(request("POST", "/v1/quit", None)),
        _ => bail!("{USAGE}"),
    }
}

/// The release exe has the GUI subsystem, so it starts without a console. A subcommand run from a terminal
/// attaches to that terminal and points any unset output handle at it, so its output still shows.
pub fn attach_console() {
    use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
        SetStdHandle,
    };
    use windows::core::w;

    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
            return;
        }
        for which in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            // A redirect (`> file`) already set the handle; keep it.
            if GetStdHandle(which).is_ok_and(|h| h != HANDLE::default() && !h.is_invalid()) {
                continue;
            }
            if let Ok(con) = CreateFileW(
                w!("CONOUT$"),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            ) {
                let _ = SetStdHandle(which, con);
            }
        }
    }
}
