# appi-dashboard: Application Insights monitor TUI (design)

Date: 2026-11-04
Status: approved by the user, 2026-11-04
Visual reference: `2026-11-04-appi-dashboard-mockup.html` in this folder. Open it in a browser and press Tab to switch views. The mockup data is fake, and the mockup's Service Bus and Redis panels are out of scope for v1 (see "Panels").

Notation: **[user]** means the user decided it. **[my call]** means a default chosen during design that the user has not explicitly confirmed. Unmarked items follow directly from the user's decisions.

---

## 1. Intent

The author's company's production SaaS (`my-app-prod`) is watched today through an Azure portal dashboard. The user wants a terminal app with the btop look that stays open **on a second screen all day** **[user]** and shows the same kind of information: sessions, users, failed requests, response time, exceptions, and so on.

The moment that matters: last time something broke, the user first noticed **a spike in failed requests**, then went to **server exceptions** and needed the **exception types and failing endpoints** **[user]**. So the app must:

1. make failures obvious at a glance, even from across the room, and
2. let you drill from "failures are up" down to the single failing request without leaving the terminal **[user]**.

It must work for **anyone** with Azure access and **any** Application Insights instance, not only my-app-prod **[user]**.

## 2. Goals

- Near-real-time ambient monitoring with an adjustable refresh rate **[user]**. Data that is 1–3 minutes behind is acceptable **[user]**.
- A striking btop-style TUI: rounded panels, braille charts with color gradients, large block-digit headline numbers, meters.
- Two views, switched with **Tab** **[user]**:
  - **Overview** (default at startup): the Azure dashboard redrawn as a terminal grid.
  - **Failures**: failures-first layout.
- Drill-down from panels: Enter goes deeper, Esc goes back, and a breadcrumb path is shown **[user]**.
- Auth through the user's existing `az login` session only **[user]**. No keys or secrets.
- Remember the chosen instance, allow changing it later, and allow different instances in separate terminals at the same time **[user]**.
- Runs on Linux, macOS and Windows **[user]**. Written in Rust **[user]**.

## 3. Non-goals for v1

- Panels from other Azure resources (Service Bus, Redis, Function apps, and so on). Deferred to v2 **[user]**, together with any way of adding or arranging panels (in-app picker, importing an Azure portal dashboard). **v1 panels are fixed [user].**
- Secure Score, the resource-group list and the Entra ID link tiles from the portal dashboard. These are not monitoring data.
- True 1-second live streaming. The portal's "Live Metrics" uses an undocumented protocol and must not be used.
- Several instances in one window. Use separate terminals.
- Desktop notifications, sounds, alert rules, or anything that writes to Azure. The app is read-only **[my call]**.
- A free-form KQL console.

## 4. Anti-goals (the user wants none of these) [user]

The user confirmed that each of these would make the app a failure even if it otherwise works:

1. **Numbers that don't match the Azure portal** for the same resource, metric, aggregation and time range.
2. **Noticeable CPU use or flicker** while idle on a second screen all day.
3. **Querying Azure so much that it gets throttled.** Cost is **not** a constraint: Azure governance and the pricing budget handle it, so the app must **not** add its own request caps, call budgets or counters **[user]**. Throttling still counts as a defect because it breaks the display.
4. **Looking right only in the author's terminal** and broken in Windows Terminal, macOS Terminal.app, iTerm2 or common Linux terminals.

Also implied by all-day use **[my call]**: it must never crash or show a blank screen because of a network blip, an expired token or a throttled request.

## 5. Screens

### 5.1 Header (every screen)

The header shows, from left to right: instance name, a badge (see below), the tab bar (`overview` | `failures ● 12`), the time range, the refresh interval with a countdown to the next refresh, the live server count, and on the right a clock with the date.

- The **Failures tab label pulses red** while the failure alert is active, so it can be noticed from the Overview **[user]**. The app **never switches tabs on its own** **[user]**.
- Badge **[my call]**: by default it shows nothing, or the instance name only. An optional per-instance label set in config (for example `PROD`) is shown as the colored badge. Do not guess the environment from the resource name.

### 5.2 Overview tab (default view)

A row of big-number tiles, then a 3×3 grid of chart panels. Each panel has a hotkey number in its title (btop style), a chart, and a legend line with current or aggregate values. A panel whose alert rule fires gets a red border and red title **[user]**.

**Headline tiles (v1):** active users, unique sessions, requests per minute, failed requests in the last 5 min, server exceptions, average server response time, availability %.

**Grid panels (v1, fixed) [my call within the user's "fixed panels" decision]:**

| # | Panel | Series |
|---|-------|--------|
| 1 | Sessions & users | unique sessions (up) and unique users (mirrored down) |
| 2 | Failed requests | failed request count |
| 3 | Server response time | avg (chart), p95 in legend |
| 4 | Page load time | browser page load; legend shows client / network / send breakdown |
| 5 | Exceptions / dependencies | server exceptions (up) and dependency failures (mirrored down) |
| 6 | CPU | process CPU %, processor time % |
| 7 | Availability | availability % and availability test result count |
| 8 | Browser exceptions | browser exception count |
| 9 | Memory & I/O | available memory, process I/O rate |

Panels 7–9 replace the mockup's Service Bus, Redis and memory panels, because v1 uses App Insights data only.

### 5.3 Failures tab

- **Large failures panel**, about two-thirds wide: a failed-request chart, server exceptions mirrored below it, a dependency-failure count in the legend, and next to the chart the **top failing endpoints** and **top exception types** for a short recent window (the mockup uses 15 min) **[user]**.
- **Health column:** btop-style meters for availability, avg and p95 response time, CPU, available memory and availability tests, plus sessions, users, requests per minute and server count.
- **Smaller panels below [my call]:** traffic, server response, page load, browser exceptions, then CPU, memory, dependency failures and availability tests.

### 5.4 Drill-down

Breadcrumb at the top, for example `overview › failed requests › POST /api/import/upload › 14:58:12`. **Enter** goes deeper and **Esc** goes back one level **[user]**.

**Must-have path [user]:**

1. **Failed requests panel** (either tab) →
2. **Failing endpoints:** operation name, fail count, result codes with counts, trend sparkline. Sortable. →
3. **One endpoint:** exception types with counts and bars, failing dependencies (type, target, count), list of recent failed requests (time, code, duration, server instance, exception type). →
4. **One request:** result, duration, instance, operation ID, session and user IDs, exception type, message and top stack frames, and an **end-to-end transaction** timeline (the request plus its dependency calls with durations and success or failure).

**Other drill-downs [my call, the user accepted these as guesses]:**

- Server exceptions → exception types → occurrences → the request.
- Dependency failures → failing targets → calls → the request.
- Server response → slowest endpoints (avg, p95) → slow requests → the request.
- Page load → slowest pages. Browser exceptions → types → occurrences.
- CPU and memory → breakdown per server instance.
- Availability → per test and location → failed results.

If time is short, the must-have path ships first and the others can follow in later v1 iterations.

### 5.5 Instance picker

- Shown at startup when there is no remembered instance and no `--app` flag **[user]**.
- A **searchable list of every Application Insights instance the `az login` identity can see**, across all subscriptions, showing name, resource group and subscription so same-named instances can be told apart **[user accepted guess]**.
- Opened later with **`i`** to switch instance **[user accepted guess]**.

### 5.6 Error and empty states [my call]

- **Not logged in or token expired:** a clear full-screen message telling the user to run `az login`, which retries automatically once a token is available. No stack traces.
- **Network failure, 429 or 5xx:** keep showing the last good data, mark affected panels as stale with their age, and retry with backoff (honor `Retry-After`).
- **No data**, for example an instance with no browser telemetry: the panel says "no data in range" instead of drawing an empty chart.
- **Terminal too small:** a compact layout or a "make the terminal bigger" message. Never corrupted drawing.

## 6. Keys

| Key | Action |
|-----|--------|
| `Tab` / `Shift+Tab` | switch view (Overview ↔ Failures) **[user]** |
| `1`–`9`, `0` | select panel by its number |
| arrow keys | move selection; inside drill-down screens, move between sub-panels **[user accepted guess]** |
| `Enter` | drill in **[user]** |
| `Esc` | back **[user]** |
| `r` | refresh now |
| `+` / `-` | change refresh interval **[user]** |
| `t` | cycle time range **[user accepted guess]** |
| `i` | instance picker **[user accepted guess]** |
| `c` | copy operation ID (request detail) **[user accepted guess]** |
| `o` | open the current item in the Azure portal **[user accepted guess]** |
| `s` | sort (list screens) |
| `?` | help overlay **[my call]** |
| `q` | quit |

## 7. Refresh, time range and alerts

- **Refresh interval [my call]:** default 30 s, adjustable in steps between 15 s and 5 min, remembered in config. The header shows a countdown. The 15 s minimum is there because Azure data only arrives about once a minute, not to save cost.
- **Time ranges [my call]:** 30 min, 1 h, 4 h (default, matching the current portal dashboard), 12 h, 24 h. Chart bucket size follows the range.
- **Alert rules [my call]:** per-panel thresholds in the config file with sensible defaults. The key one is "failed requests in the last 5 min ≥ N", with a small default N so a single stray failure on a busy app doesn't turn the screen red; the builder picks N and documents it. The failure alert drives the Failures-tab pulse and the red borders.
- The clock and all timestamps use the local time zone.

## 8. Data and Azure access

How data is fetched is mostly left to the builder, within these rules:

- **Auth:** use the token of the signed-in `az login` session (`az` CLI must be installed). This must work on Windows, where the CLI is `az.cmd`. Tokens expire, so re-acquire them as needed.
- **Instance discovery:** list App Insights resources across all accessible subscriptions. Azure Resource Graph is the obvious fast option.
- **Charts must match the portal (anti-goal 1):** the portal's dashboard tiles are Azure Monitor metric charts on the App Insights resource. Prefer the **same metrics, aggregations and time grain** through the Azure Monitor Metrics API, and use log (KQL) queries only where no metric exists. Any number shown in two places (tile and chart) must come from the same source.
- **Drill-down and top-N lists:** log queries (KQL) on the instance's telemetry (requests, exceptions, dependencies, page views and so on), joined on operation ID for the transaction view.
- **Azure calls (anti-goal 3):** cost is not a constraint and requests must not be capped or budgeted **[user]**. Paid Metrics API calls are fine. The only concern is **throttling**: batch where it is natural (several metrics per call), don't send the same query twice in one refresh, handle 429 and `Retry-After` gracefully, and stay well inside throttling limits even with several terminals open under the same user.
- **Reference dashboard:** the user's current Azure portal dashboard is a `Microsoft.Portal/dashboards` resource whose ID the user can provide. During development, read its definition with `az` to copy the exact metric IDs, aggregations, time grain and filters for the v1 panels it has in common with this design. That ID and any tenant or subscription details stay **out of the public repo**.
- **Verify early (risks to spike first):** which App Insights metrics (users, sessions, page-load breakdown, performance counters) are available through the Metrics API versus only through logs; the right query endpoint and token audience for log queries in a way that works for a user with plain Reader access on the App Insights resource; and the Live Metrics-free way of getting the "active servers" count.

## 9. Configuration and instances

- Per-user config file in the OS-standard config location (Linux `~/.config/…`, macOS `~/Library/Application Support/…`, Windows `%APPDATA%\…`) **[my call]**. TOML **[my call]**.
- It stores the last used instance, a recent-instances list, the refresh interval, the time range, alert thresholds and optional per-instance labels.
- `--app <name | resource ID>` starts directly on an instance **[user accepted guess]**. A name that matches several instances opens the picker filtered to those. CLI flags override config for that run only.
- **Separate terminals are independent [user].** Two running copies must never corrupt the config file. Writes are atomic, and "last used" is simply whoever wrote last **[my call]**.

## 10. Look and terminal compatibility (anti-goals 2 and 4)

- btop-inspired dark theme: rounded borders, panel titles in the top border with hotkey numbers, braille area charts with vertical color gradients, mirrored charts for paired series, block-digit headline numbers, `■` meters. The mockup is the reference for the look.
- **Must render correctly** in Windows Terminal, macOS Terminal.app and iTerm2, and common Linux terminals (Alacritty, Kitty, GNOME Terminal, Konsole, foot, Ghostty).
- Detect color support and fall back from 24-bit color to 256 colors where needed. macOS Terminal.app is the usual case to check.
- Make sure the whole app is readable on a light terminal background, or ship a light theme selectable in config **[my call; the builder chooses how]**.
- **Idle efficiency:** redraw only when data changes, on input, or for the once-per-second clock and countdown. Draw only what changed so there is no flicker. Idle CPU should be close to zero.
- Handle terminal resize live. Panels reflow into fewer columns on narrow terminals **[my call]**.

## 11. Technology [my call within the user's Rust choice]

Suggested: `ratatui` + `crossterm` (cross-platform TUI), `tokio` (async), `reqwest` with rustls (HTTP, so there's no OpenSSL dependency on Windows or macOS), `serde`, `clap` (CLI), `directories` (config paths). The builder may change any of these with a reason.

## 12. Repository and distribution

- **Public GitHub repository under https://github.com/suomiton [user].** Working repo name: `appi-dashboard` **[my call]**.
- **GitHub Actions** builds and tests on Linux, macOS and Windows. A version tag produces a GitHub Release with one self-contained binary per platform: Linux x86_64 and arm64, macOS Intel and Apple Silicon, Windows x86_64 **[my call]**. `cargo install` from source also works. The only runtime requirement is the Azure CLI.
- **License:** MIT OR Apache-2.0, the usual Rust dual license **[my call]**.
- **v1 binaries are unsigned [my call].** The README documents the one-time macOS "allow" step and the Windows "unknown publisher" warning. Code signing can come later.
- **Public-repo hygiene [my call]:** no tenant or subscription IDs, dashboard IDs, or company-internal resource names in code, tests, fixtures, screenshots or docs. Test fixtures use made-up data. Before the first push, this design document and the mockup get scrubbed (for example `my-app-prod` → `my-app-prod`) or are kept out of the public repo.

## 13. Development and testing environment

- **Linux (this machine):** the user will install the Azure CLI and log in, so development and the portal-matching checks run against the user's real instance, read-only **[user]**.
- **Windows:** the user will test the builds in Windows Terminal **[user]**.
- **macOS:** no manual tester is confirmed yet. CI builds and runs tests on macOS. Terminal.app safety relies on the color fallback in section 10. A Mac check by a colleague would be welcome before calling v1 done.

## 14. Left to the builder

Exact API endpoints and versions; KQL; how calls are batched and cached; internal architecture; testing approach; exact layout maths and reflow breakpoints; exact color values (follow the mockup); the alert default N; the binary name (working name: `appi-dashboard`).

## 15. Open questions

- Who checks macOS by hand (section 13).
- v2 topics (out of scope now): panels from other Azure resources, in-app panel picker, importing an Azure portal dashboard.
