# notchd

A notification centre shown through a dynamic-island pill at the top-centre of the screen (Windows, Rust, gpui).
The island is ported from the tour-guide app: a spring-animated notch, clipped to its shape, never taking focus.

## Run

```
cargo run                      # the daemon: island + API on 127.0.0.1:7878 (NOTCHD_PORT to change)
notchd notify "Build passed" "42 tests" --app CI [--urgency low|normal|critical] [--timeout ms] [--id key]
notchd list | clear | state
notchd dnd on|off
```

- **Idle:** a small pill. Hover shows the count; click opens the history list.
- **Toast:** a notification grows the pill, then it shrinks back after its timeout. Click a toast to dismiss it.
  Critical stays until dismissed and jumps the queue. A same-`id` post replaces in place.
- **Do Not Disturb:** posts go to history without a toast; critical still shows.

## API

Every call needs `Authorization: Bearer <token>` (token file: `%APPDATA%\notchd\token`), a `Host` of
`127.0.0.1:<port>` or `localhost:<port>`, and no `Origin` header.

| Route | |
|---|---|
| `POST /v1/notifications` | `{title, body?, app?, id?, urgency?, timeout_ms?}` → `{id}` |
| `GET /v1/notifications` | history, newest first |
| `DELETE /v1/notifications/{id}` / `DELETE /v1/notifications` | remove one / clear |
| `PUT /v1/dnd` | `{enabled}` |
| `GET /v1/state` | `{dnd, count}` |

## Not yet

History is in memory only. No config file or themes, no plugins, no capture of Windows toasts, no action buttons.
