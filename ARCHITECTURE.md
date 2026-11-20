# How en-Master works

A map of the code for contributors and reviewers. Each module is small and has one job.

## Flow of one correction

1. A GNOME shortcut runs `enmaster --action coach` (or the GlobalShortcuts portal tells the
   running instance, in Flatpak).
2. `ui::run` hands the command line to the single resident `adw::Application`.
3. `ui::coach::open` reads the selected text (`selection.rs` outside the sandbox, GTK's primary
   clipboard inside it) and shows the window.
4. The action runs on a worker thread: `engine::run` gets a token (`auth.rs`), picks a model,
   builds the instructions (`actions.rs`) and streams the reply (`api.rs`).
5. Back on the main thread the reply is parsed into `Feedback` (text, mistakes, tips), shown,
   and appended to the learning log (`learn.rs`).
6. Replace puts the result on the clipboard and, outside the sandbox, sends Ctrl+V with
   `ydotool`.

## Modules

| File | Job |
|---|---|
| `main.rs` | Command-line entry: terminal commands, otherwise the GTK app |
| `paths.rs` | App ID, folders, private file writes, sandbox detection, migration from old names |
| `config.rs` | `config.toml` (user settings) and `state.json` (things the app learns) |
| `i18n.rs` | gettext setup and the `tr`, `trf`, `ntrf` helpers |
| `actions.rs` | Built-in actions, the instructions sent to the model, the feedback JSON schema and parser |
| `auth.rs` | Sign in with ChatGPT: registration, PKCE, loopback callback, token refresh, sign-out |
| `keyring.rs` | Stores the connection in the Secret Service with `oo7` |
| `http.rs` | HTTP helpers and `HttpError`, which turns API errors into readable messages |
| `api.rs` | `/v1/models` and streaming `/v1/responses` (server-sent events) |
| `engine.rs` | Runs an action, retries without optional fields the endpoint rejects |
| `learn.rs` | Learning log, progress report, Anki export |
| `srs.rs` | Drill cards from logged mistakes, Leitner-box scheduling, answer checking |
| `selection.rs` | Reads PRIMARY through XWayland (`xsel`) and pastes with `ydotool` (host only) |
| `install.rs` | `enmaster install` without Flatpak: files, autostart, GNOME custom shortcuts |
| `ui/mod.rs` | Application start-up, app actions and accelerators, About and shortcuts dialogs, helpers |
| `ui/coach.rs` | The main window |
| `ui/drill.rs`, `ui/progress.rs`, `ui/preferences.rs` | The other windows and the preferences dialog |
| `ui/background.rs` | Daily reminder; in Flatpak, the GlobalShortcuts and Background portals |

## Decisions worth knowing

- **One resident instance.** The app keeps running after its last window closes
  (`ApplicationHoldGuard`), so shortcuts open windows quickly, the clipboard survives the window,
  and the reminder timer can run. `Ctrl+Q` or `enmaster quit` stops it.
- **Reading the selection.** A shortcut-launched window has no focus yet, and Wayland only gives
  the primary selection to the focused client. Outside the sandbox, Mutter mirrors selections to
  XWayland, where `xsel` can read them at once. Inside Flatpak the window reads GTK's primary
  clipboard when it becomes active.
- **ChatGPT plan access.** `auth.rs` follows OpenAI's documented flow for open-source apps:
  `dynamic_agent_client` plus a stable `ext_agent_host_id` on first sign-in, then the issued
  `oaiapp_…` client ID. Refresh tokens rotate, so refreshes are serialized with a file lock
  across processes. The ID token's claims are checked; its signature is not, because it comes
  straight from the token endpoint over TLS (OpenID Connect Core §3.1.3.7).
- **Requests.** Plan tokens require `store: false` and `stream: true` and reject fields such as
  `max_output_tokens` and `temperature`. Structured output (`text.format`) and
  `reasoning.effort` are not documented for plan tokens, so `engine.rs` tries them and remembers
  in `state.json` if the endpoint refuses.
- **Feedback as JSON.** Every action returns `{text, mistakes, tips}`. If the model answers with
  plain text anyway, the text is shown and nothing is logged.
- **Drills.** Cards are distinct `(original, fix)` pairs from the log. A right first answer moves a
  card up a box (1, 2, 4 … 64 days); a wrong one sends it back to box 0 and repeats it once at the
  end of the session.
- **Translations.** Strings are marked with `tr("…")`; `po/update.sh` extracts them with
  xgettext's C parser, so marked strings must be plain `"…"` literals.

## Tests

`cargo test` covers the parts with logic: SSE parsing and a mock HTTP server for `api.rs`, OAuth
callback and token checks, feedback parsing, reports and Anki export, drill scheduling and
answer checking, config round-trips, the installer helpers and reminder timing.

For the UI, debug builds accept `ENMASTER_DEMO=result|screenshot|signin|busy` and
`ENMASTER_SNAPSHOT=file.png` to render a window without the network; the store screenshots in
`data/screenshots` were made that way in a headless GNOME Shell.
