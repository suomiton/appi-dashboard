# appi-dashboard

A btop-style terminal dashboard for **Azure Application Insights**. Keep it on a
second screen: it shows near-real-time sessions, users, failed requests,
response times, exceptions, dependencies, availability and server health for
any Application Insights instance your `az login` can see. When something
breaks, drill down from the failure spike to the failing endpoint, its
exception types and failing dependencies, and finally a single request with
its stack trace and end-to-end transaction.

```
 ◆ my-app-prod  │  overview   failures ● 12  │ range 4h │ refresh 30s next 12s ⟳ │ ● 2 servers     15:06:42
╭ users ──────╮╭ sessions ───╮╭ requests ───╮╭ failed ─────╮╭ exceptions ─╮╭ response ───╮╭ availability ╮
│▀█  █▀▀ █▀▀  ││▀▀█ █▀▀ █▀█  ││█ █ ▀█  ▀▀█  ││▀█  ▀▀█      ││▀▀█ █▀█      ││▀█  ▀█  █▀█  ││▀█  █▀█ █▀█   │
│ █  █▀█ ▀▀█  ││ ▀█ █▀█ █ █  ││▀▀█  █  █▀▀  ││ █  █▀▀      ││ ▀█ █ █      ││ █   █  ▀▀█  ││ █  █ █ █ █   │
│▀▀▀ ▀▀▀ ▀▀▀  ││▀▀▀ ▀▀▀ ▀▀▀  ││  ▀ ▀▀▀ ▀▀▀  ││▀▀▀ ▀▀▀      ││▀▀▀ ▀▀▀      ││▀▀▀ ▀▀▀ ▀▀▀  ││▀▀▀ ▀▀▀ ▀▀▀   │
╰─────────────╯╰─────────────╯╰─────────────╯╰─────────────╯╰─────────────╯╰─────────────╯╰──────────────╯
```

Try it without Azure: `appi-dashboard --demo`.

## Requirements

- The [Azure CLI](https://aka.ms/azcli), signed in with `az login`. The app uses
  your CLI session for tokens; it stores no keys or secrets.
- Read access (for example **Reader**) on the Application Insights resources you
  want to watch.
- A terminal with Unicode support. 24-bit color is used when available, with an
  automatic fallback to 256 colors (for example in macOS Terminal.app).

## Install

Download the archive for your platform from
[Releases](https://github.com/suomiton/appi-dashboard/releases) (Linux x86_64 and
arm64, macOS Intel and Apple Silicon, Windows x86_64), or build from source:

```sh
cargo install --git https://github.com/suomiton/appi-dashboard
```

The release binaries are not code-signed yet:

- **macOS:** remove the download quarantine once:
  `xattr -d com.apple.quarantine ./appi-dashboard`
- **Windows:** SmartScreen may warn about an unknown publisher; choose
  **More info → Run anyway**.

## Usage

```sh
appi-dashboard                      # last used instance, or a picker on first run
appi-dashboard --app my-app-prod    # by name (or full resource ID)
appi-dashboard --range 24h --refresh 60
appi-dashboard --demo               # made-up data, no Azure needed
appi-dashboard probe --app my-app-prod   # check access and print what a refresh returns
```

Run it in several terminals at once to watch several instances. Each run
remembers the last instance you picked.

| Flag | Meaning |
|---|---|
| `--app <name or resource ID>` | Instance to open. A name shared by several instances opens the picker. |
| `--range 30m\|1h\|4h\|12h\|24h` | Time range (default 4h). |
| `--refresh 15\|30\|60\|120\|300` | Refresh interval in seconds (default 30). |
| `--color auto\|truecolor\|256` | Color depth (default auto). |
| `--config <path>` | Use a different config file. |
| `--demo` | Made-up data; nothing is read from or saved to Azure or your config. |

## Keys

| Key | Action |
|---|---|
| `Tab` / `Shift+Tab` | Switch view: Overview ↔ Failures |
| `1`–`9`, `0` | Select a panel by its number |
| arrow keys | Move the selection; switch sub-panels in drill-down screens |
| `Enter` | Drill in |
| `Esc` | Back |
| `r` | Refresh now (reload in drill-down screens) |
| `+` / `-` | Longer / shorter refresh interval |
| `t` | Cycle the time range |
| `i` | Pick another Application Insights instance |
| `c` | Copy the operation ID (request detail) |
| `o` | Open in the Azure portal |
| `s` | Sort lists by the next column |
| `?` | Help |
| `q` | Quit |

The **Failures** tab label pulses red while failed requests are above the alert
threshold, so you notice it from the Overview. The app never switches tabs by
itself.

## Configuration

The config file is created when you pick an instance or change a setting:

| OS | Location |
|---|---|
| Linux | `~/.config/appi-dashboard/config.toml` |
| macOS | `~/Library/Application Support/io.github.suomiton.appi-dashboard/config.toml` |
| Windows | `%APPDATA%\suomiton\appi-dashboard\config\config.toml` |

Example:

```toml
refresh_secs = 30
range = "4h"
color = "auto"          # auto, truecolor or 256

[alerts]
failed_requests_5m = 5          # failed requests in 5 min that turn failures red
server_exceptions_5m = 10
dependency_failures_5m = 10
availability_below_pct = 99.0
response_avg_above_ms = 1000.0

# Optional badge shown next to the instance name (keys are resource IDs).
[labels]
"/subscriptions/00000000-0000-0000-0000-000000000000/resourceGroups/my-rg/providers/microsoft.insights/components/my-app-prod" = "PROD"
```

Command-line flags override the file for that run only.

## About the data

- Charts use the same Azure Monitor metrics and aggregations as the Azure
  portal's Application Insights charts; users and sessions use Microsoft's
  log-based definitions. Drill-down screens use log (KQL) queries.
- Telemetry usually arrives 1–3 minutes after it happens, so "now" is a little
  behind.
- Azure bills Metrics API calls beyond its monthly free allowance. Each
  refresh makes a handful of calls; how much that costs is governed by your own
  Azure budget.
- The app is read-only: it never changes anything in Azure.

## Troubleshooting

- **"Azure CLI sign-in needed"**: run `az login` in another terminal; the app
  retries by itself.
- **Windows**: the CLI is invoked as `az.cmd`; make sure it is on `PATH`.
- **Colors look off**: try `--color 256`, or `--color truecolor` if your
  terminal supports it but does not advertise it.
- **Something is missing or shows an error**: `appi-dashboard probe --app <name>`
  checks every query the dashboard and its drill-down screens run and prints
  `[ok]` / `[FAIL]` per check.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
