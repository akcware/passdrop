# passdrop — v1 design

> **Working name.** "passdrop" is a placeholder until the name is decided.
> **Status:** draft, 2026-11-11.

## 1. Problem

Coding agents (Claude Code, Codex) often need a value from the human that the agent itself should never see: an API key, a `.p8` signing key, a token, a phone number for an app review form. Today the agent asks the user to paste it into the chat, so the value lands in the transcript and the model context. The alternative is a list of manual steps for the user.

passdrop lets the agent describe **what** it needs and **where it goes**. The human supplies the value in a small native window. passdrop writes the value to its destination and reports back metadata only.

## 2. Goals and non-goals

**Goals**

- The value never enters the chat, the transcript or the model context.
- One small binary with no runtime. First frame in roughly 0.1–0.4 s (spike numbers, §3).
- Works the same from Claude Code and Codex: a CLI plus a `SKILL.md`.
- The agent writes the spec; the human controls the window; destinations come from a fixed allowlist.
- Format validation never blocks. It only warns.

**Non-goals for v1**

- Not a security boundary against an agent with shell access. Once a value is in `.env` or a file, the agent can read it. Pair passdrop with deny rules (§9.3).
- Not a vault, secret manager or runtime injector.
- No provider liveness checks. They would send keys over the network.
- No MCP server. It could be a thin wrapper later.
- No arbitrary-command sinks.

## 3. Findings this design rests on

| Finding | Source |
|---|---|
| The window takes focus by itself: 3 of 3 runs, 175–237 ms after start, no click. Launched from the Claude desktop app's shell; **not yet tested from a terminal or Codex.** | spike |
| A dropped file gives its real path (`ViewportBuilder::with_drag_and_drop(true)`). | spike |
| A value entered into a masked field arrived (15 chars). The spike can't tell paste from typing. | spike |
| Binary size: 7.2 MB with wgpu, 4.9 MB with glow (empty window, LTO + strip). The glow build was not run. | spike |
| egui replaces password-field text with bullets in its accessibility output and disables copy. | egui 0.35 source, not a runtime test |
| A timer driven by UI frames stalled in 2 of 3 runs after the user switched to another window. The likely cause is that eframe doesn't repaint a covered window; this was not confirmed. Either way, timers must not depend on the UI loop. | spike |
| Codex's default sandbox blocks port binding, writes outside the workspace and network access, and very likely GUI launch. passdrop must run outside it. | Codex research |
| Claude Code has no documented default that blocks reading `.env`. Plugins cannot ship permission rules. | Claude Code docs |
| `gh secret set` reads the value from stdin when `--body` is absent. `vercel env add` reads stdin when `--value` is absent and has `--force` to overwrite. | `--help` output (Vercel CLI 54.14.2) |
| Every prior-art project that validates values **blocks** on mismatch; none uses JSON Schema; none has a Vercel sink. | prior-art survey |
| tokenstash's `providers.json` is MIT-licensed with 79 entries, each with a url and steps; 46 also have a prefix pattern. | licence and count verified via GitHub API; field counts from the survey |
| Single quotes are the only dotenv quoting that `sh`, `zsh`, Node, Bun and Deno all read literally; Bun still expands `$VAR` inside them (§7.3). | loader test, 2026-11-11 |
| git prints localized messages (German on this machine), so passdrop runs git with `LC_ALL=C` before matching its output. | milestone 1 test failure |

## 4. Architecture

```
agent ── spec.json ──▶ passdrop request ──▶ egui window ◀── human
                              │                   │
                              │        values in memory (zeroized)
                              ▼
                 sinks: dotenv · file · keychain · gh-secret · vercel-env
                              │
agent ◀── one JSON line, metadata only ──┘
```

Cargo workspace:

- **`crates/core`** (library, no GUI dependencies): spec parsing and checks, warning engine, key registry, sinks, destination guards, result JSON. Everything here is unit-testable without a display.
- **`crates/passdrop`** (binary): argument parsing, the egui window, `init`.

Values travel as `secrecy::SecretString` from the input field to the sink. Buffers are zeroized after the write.

## 5. CLI

| Command | What it does | Opens a window |
|---|---|---|
| `passdrop request <spec.json \| ->` | Check the spec, look up existing values, open the window, write the sinks, print the result. | yes |
| `passdrop check <spec>` | For each field: is it already set at its sink? No values. | no |
| `passdrop validate <spec>` | Spec errors and warnings only. | no |
| `passdrop schema` | Print the JSON Schema of the spec format. | no |
| `passdrop init [--claude] [--codex] [--undo]` | Install or remove the skill (§9.3). | no |

`request` flags: `--timeout <secs>` (default 600), `--on-top`.

**Exit codes**

| Code | Status | Meaning |
|---|---|---|
| 0 | `ok` | Every field was written, kept, generated or echoed, or was optional and left empty (`skipped`). |
| 1 | `error` | Internal error. |
| 0 | `valid` | `validate` only: the spec and its destinations are fine. |
| 2 | `invalid_spec` / `usage` | Spec error, or a usage error such as an unreadable spec file. Nothing was shown. |
| 10 | `cancelled` | The human cancelled. |
| 11 | `timeout` | The window timed out. |
| 12 | `no_gui` | No display available. |
| 20 | `partial` | Some sinks failed. |
| 21 | `failed` | No sink succeeded. |

**Streams:** for `request`, `check` and `validate`, stdout always gets exactly one JSON line, errors included. `schema` prints the schema and `init` prints human-readable text. stderr gets human-readable diagnostics and never contains values.

## 6. Spec format

The spec is a subset of JSON Schema 2020-12. The top level is `"type": "object"`, and each property is one field.

### 6.1 Top-level keywords

| Keyword | Required | Meaning |
|---|---|---|
| `title` | yes | Window title. Max 80 chars. |
| `x-purpose` | yes | Why the agent needs these values. Shown in the header. Max 300 chars. |
| `description` | no | Shown under the title. Max 1000 chars. |
| `properties` | yes | The fields. |
| `required` | no | Fields that must be filled before Submit is enabled. |
| `x-timeout` | no | Seconds. Overrides the default 600. |

### 6.2 Field keywords

Standard JSON Schema keywords:

| Keyword | Meaning |
|---|---|
| `title` | Label. Required. |
| `description` | Help text. Plain text; `https://` URLs become links. Max 1000 chars. |
| `type` | `"string"` only in v1. Omitted for file fields. |
| `writeOnly: true` | Secret: masked input, never echoed, never in the result. |
| `examples` | The first entry is shown as a greyed placeholder, e.g. `"sk-proj-…"`. |
| `default` | Prefill. Not allowed with `writeOnly`. |
| `enum` | Renders a select. |
| `pattern` | **Warn only.** Rust `regex` syntax: linear time, no look-around or backreferences. Unanchored, as in JSON Schema, so authors write `^…$`. Max 200 chars and must compile. |
| `minLength`, `maxLength` | Warn only. |
| `format` | `uuid`, `email`, `uri`. Warn only. |

Extensions:

| Keyword | Meaning |
|---|---|
| `x-widget` | `text` (default), `textarea` (multi-line, e.g. PEM) or `file`. A select comes from `enum`. |
| `x-accept` | File extensions for a file field, e.g. `[".p8"]`. A mismatch warns. |
| `x-max-bytes` | File size limit. Default 1 MiB. This is a guard, so it blocks. |
| `x-prefix` | Expected prefixes, e.g. `["sk-"]`. Warn only. |
| `x-known` | Registry entry by env-var name, e.g. `"OPENAI_API_KEY"`. Adds a pattern, docs URL and steps. Without it, the name a `dotenv`, `gh-secret` or `vercel-env` sink writes to is looked up automatically. |
| `x-docs-url` | Where to get the value. `https` only. A registry URL wins over this one. |
| `x-generate` | `"hex:32"`, `"base64:32"` or `"uuid"`. passdrop generates the value; nobody sees it. |
| `x-sink` | Destination (§7). |
| `x-echo: true` | Return the value to the agent, where it is visible in the chat. Not allowed with `writeOnly`. Can be combined with `x-sink`. |

Every field needs `x-sink`, `x-echo: true`, or both. Nothing is echoed implicitly, so a value can only reach the agent when the spec says so and the window shows it (§8).

### 6.3 Spec errors (exit 2, nothing shown)

- A field name that doesn't match `^[A-Za-z_][A-Za-z0-9_]{0,63}$`.
- `writeOnly` combined with `default` or `x-echo`.
- A field with neither `x-sink` nor `x-echo: true`.
- `writeOnly` without `x-sink`, because the value would go nowhere.
- An unknown `x-` keyword, so typos such as `x-snk` fail loudly. Unknown standard keywords only warn.
- A `pattern` that doesn't compile or is longer than 200 chars.
- Text longer than its limit.
- A sink that fails its own checks or a destination guard (§7.2).

### 6.4 Example

```json
{
  "title": "App Store submission",
  "x-purpose": "Upload builds and metadata with the App Store Connect API",
  "type": "object",
  "properties": {
    "asc_key": {
      "title": "App Store Connect API key (.p8)",
      "description": "App Store Connect → Users and Access → Integrations → App Store Connect API → Team Keys → +. The file can be downloaded only once.",
      "writeOnly": true,
      "x-widget": "file",
      "x-accept": [".p8"],
      "x-sink": { "kind": "file", "dir": "~/.appstoreconnect/private_keys/", "name": "original", "mode": "600" }
    },
    "issuer_id": {
      "title": "Issuer ID",
      "type": "string",
      "format": "uuid",
      "description": "Shown above the Team Keys table.",
      "x-echo": true
    },
    "review_phone": {
      "title": "Phone number for App Review",
      "type": "string",
      "writeOnly": true,
      "x-sink": { "kind": "dotenv", "path": ".env.local", "key": "REVIEW_PHONE" }
    },
    "openai_key": {
      "title": "OpenAI API key",
      "type": "string",
      "writeOnly": true,
      "x-known": "OPENAI_API_KEY",
      "x-sink": { "kind": "dotenv", "path": ".env.local", "key": "OPENAI_API_KEY" }
    },
    "session_secret": {
      "title": "Session secret",
      "writeOnly": true,
      "x-generate": "hex:32",
      "x-sink": { "kind": "dotenv", "path": ".env.local", "key": "SESSION_SECRET" }
    }
  },
  "required": ["asc_key", "issuer_id"]
}
```

## 7. Sinks

### 7.1 Allowlist

> **Branch status:** on `main`, only `dotenv` and `file` are implemented; `keychain`, `gh-secret` and `vercel-env` live on the `remote-sinks` branch and return `not_implemented` on `main`.

The list is compiled into the binary. A future config file may only narrow it, and only from the user's global config (`~/.config/passdrop/`), never from a repository.

| `kind` | Shape | How it writes | How `check` detects "already set" |
|---|---|---|---|
| `dotenv` | `{ "kind": "dotenv", "path": ".env.local", "key": "NAME" }` | In process (§7.3). | Key present and non-empty. |
| `file` | `{ "kind": "file", "dir": "…/", "name": "original" \| "<name>", "mode": "600" }` or `{ "kind": "file", "path": "…", "mode": "600" }` | In process. Copies a dropped file, or writes a text value to a file. | Path exists. |
| `keychain` | `{ "kind": "keychain", "service": "…", "account": "…" }` | `security-framework`, in process, macOS login keychain (macOS only in v1). Read back only for items passdrop just created, which can't prompt. | Attributes-only search; never reads the secret, so never prompts. |
| `gh-secret` | `{ "kind": "gh-secret", "name": "NAME", "repo": "owner/repo", "env": "…", "org": "…" }` (`repo`, `env` and `org` optional) | `gh secret set NAME [-R repo] [-e env] [-o org]`, value on stdin. | `gh secret list`. |
| `vercel-env` | `{ "kind": "vercel-env", "name": "NAME", "targets": ["production", "preview"], "sensitive": true }` | `vercel env add NAME <target> [--sensitive] [--force]`, value on stdin, one call per target. | `vercel env ls`. |

Values are never passed as command-line arguments, because `ps` shows them.

### 7.2 Destination guards (these block)

- Expand `~`; resolve relative paths against the cwd; canonicalize the existing part of the path; follow symlinks and check the final target. `..` is not allowed in the part that doesn't exist yet.
- **Direct children refused:** files directly in `/` or in `$HOME` (e.g. `~/.zshrc`, `~/.npmrc`). Subdirectories of home are fine.
- **Everything under these refused:** shared temp (`/tmp`, `/var/tmp`, `/private/tmp`, `/private/var/tmp`); system directories (`/etc`, `/private/etc`, `/usr`, `/bin`, `/sbin`, `/System`, `/Library`, `/Applications`, `/opt`, …); and `~/.ssh`, `~/.aws`, `~/.kube`, `~/.gnupg`, `~/.docker`, `~/.config/gh`, `~/.local`, `~/Library/Keychains`, `~/Library/LaunchAgents`.
- **Protected directory names anywhere in the path:** `.git`, `.github`, `.husky`, `.ssh`, `.aws`, `.kube`, `.gnupg`, `.vscode`, `.idea`, `.cursor`, `.gemini`, `.agents`. These hold credentials, hooks, CI or agent configuration.
- **`.claude` and `.codex`** are refused unless the next component is `worktrees`, so a `.env` in a worktree checkout (e.g. `proj/.claude/worktrees/x/.env`) is writable while `.claude/settings.json` is not, even inside a worktree.
- Comparisons ignore ASCII case (macOS file systems usually do).
- The target must be absent or a regular file owned by the current user. Never a FIFO, device or directory.
- The guards run at plan time and again right before each write, because the window can stay open for minutes. A destination that resolves differently at write time fails with `destination_changed`.
- **Existing value:** the window shows "already set" with **Keep** (default) or **Replace**. The core enforces this: without an explicit Replace, a non-empty different value is never overwritten.
  - Set when the request was planned and no Replace: the field is `kept`.
  - Appeared after planning, or a dropped file's name collides with an existing file: the field fails with `exists`, because the human never saw the choice.
  - `unchanged` only when the existing line is exactly what passdrop would write. A line that merely decodes to the same value is rewritten in passdrop's grammar, because real loaders may read it differently.
- **Git:**
  - If the target is a tracked file, the window shows a red warning that the value will be committed. Submit needs an explicit "write anyway" tick from the human.
  - If the target is untracked and not ignored, a "add to `.gitignore`" checkbox is ticked by default.
  - If git gives an indeterminate answer (e.g. Codex puts an empty `.git` in writable roots), show "git status unknown" and don't crash.

### 7.3 dotenv fidelity

- Edit only the target key's lines: replace the first definition in place, drop later duplicates, or append. Every other byte stays as it was. If an entry's end can't be found (an unterminated quote), refuse to edit (`unparseable`).
- **Quoting grammar.** Chosen by running awkward values (`$`, `\`, `"`, `'`, `#`, spaces, newlines, backticks) through `sh`, `zsh`, Node 25 `process.loadEnvFile`, Bun 1.4 and Deno 2.2 on 2026-11-11:
  1. Unquoted when the value is non-empty and only uses `A-Za-z0-9_./:@+=-`.
  2. Otherwise single quotes when the value has no `'`. All five loaders read single quotes literally, newlines included.
  3. Otherwise double quotes when the value has none of `"`, `$`, backtick, `\`.
  4. Otherwise refuse (`unquotable`) and suggest a file or keychain sink.
  - Double quotes with backslash escapes were rejected: Node keeps `\\` and `\"` literally, and `sh` expands `$` inside them.
  - Bun expands `$VAR` even inside single quotes, so values containing `$` get a `loader_dependent` warning.
  - python-dotenv was not available to test.
- **Preserve:** the `export ` prefix, line endings (LF or CRLF) and the file mode. New files get 0600.
- **Atomic write:** a temp file in the same directory, then fsync, then rename.
- **`verified`** means "read back by passdrop's own parser and equal to what was written".
- **Encrypted files:** if the file carries `DOTENV_PUBLIC_KEY` (dotenvx), refuse to write plaintext and say why.

### 7.4 Sink constraints

Each sink declares limits such as maximum size and whether newlines are allowed. The window warns before Submit instead of failing during the write.

## 8. Window

**Layout, top to bottom**

1. **Header:** title, `x-purpose`, provenance line, timeout countdown.
   - Provenance comes from environment markers: `CLAUDECODE` or `CLAUDE_CODE_ENTRYPOINT` → Claude Code; `CODEX_*` → Codex.
   - Example: "Requested by Claude Code in `~/Projects/app`".
   - It is a hint, not proof: the agent launches passdrop and controls its environment. With no marker, the header says "unknown caller".
2. **Each field:**
   - label and required mark;
   - help text, and a link button that shows the full domain (`platform.openai.com ↗`);
   - the input;
   - amber warnings under the input, which never block;
   - a monospace destination line, e.g. `→ .env.local · OPENAI_API_KEY`. An echoed field says `→ returned to the agent (visible in chat)`. If an echoed value matches a registry pattern, it gets a warning that it looks like a key;
   - a Keep / Replace choice if the value is already set.
3. **Footer:** Cancel (Esc) and Submit (Enter, or ⌘Enter while a textarea has focus).

**Rules**

- **Agent text is untrusted.** Agent-supplied text is rendered as plain text and length-capped. Links must be `https` and show their full domain. For a known key, the registry URL replaces the agent's URL.
- **Masked fields:** a press-and-hold eye button shows the value briefly.
- **Paste normalisation:** trim surrounding whitespace and a trailing newline, and report `trimmed`. Placeholder-looking values (`your-…-here`, `xxx`, `<…>`, `changeme`, under 6 chars) get a warning.
- **Generated fields** have no input. They show "Generated: 32 random bytes (hex)", and the value is never displayed.
- **Activation:** request focus on open; `--on-top` sets the floating level. The spike needed neither from the Claude app; re-test from a terminal and from Codex.
- **Dropped originals:** after a dropped file is copied, the original (e.g. in `~/Downloads`) is still readable by the agent. The window shows a "Move the original to the Trash" checkbox. The result reports `"original": "trashed"` or `"kept"`.
  - The box starts ticked only for a `writeOnly` field whose dropped file looks like the expected one: right extension (`x-accept`) and, for key files, a PEM header.
  - Otherwise it starts unticked, with a note that the original stays.
  - The core enforces the same rule: a file with an `extension` or `format` warning is trashed only with explicit consent (`trash_unexpected`).
  - This came from the live test, where a PNG dropped on the `.p8` field was trashed.
- **Timeout:** runs on its own thread, independent of UI frames (§3). On expiry, that thread prints the `timeout` JSON line and exits the process itself.
- **No display:** on Linux without `DISPLAY` or `WAYLAND_DISPLAY`, or in an SSH session, return `no_gui` (exit 12) without trying to open a window.
- **Accessibility:** keep AccessKit. Masked fields expose bullets only, per the source. Verify this at runtime before release.

## 9. Agent integration

### 9.1 Output contract

```json
{
  "status": "ok",
  "fields": {
    "asc_key":        { "action": "created",   "ref": "file:~/.appstoreconnect/private_keys/AuthKey_ABC123XYZ.p8", "filename": "AuthKey_ABC123XYZ.p8", "verified": true, "original": "trashed" },
    "issuer_id":      { "action": "echoed",    "value": "57246542-96fe-1a63-e053-0824d011072a" },
    "review_phone":   { "action": "created",   "ref": "env-file:.env.local#REVIEW_PHONE", "verified": true },
    "openai_key":     { "action": "updated",   "ref": "env-file:.env.local#OPENAI_API_KEY", "verified": true, "warnings": ["prefix"] },
    "session_secret": { "action": "generated", "ref": "env-file:.env.local#SESSION_SECRET", "verified": true }
  },
  "next": "Values are in place. Do not read or print them; use them by reference."
}
```

- **`action`:** `created`, `updated`, `unchanged`, `kept`, `generated`, `echoed`, `skipped` or `failed`.
- **`ref` forms:**
  - `env-file:<path>#KEY`
  - `file:<path>`
  - `keychain:<service>/<account>`
  - `gh-secret:<owner/repo>#NAME`
  - `vercel-env:<target>#NAME`
- **`warnings` codes:** `pattern`, `prefix`, `length`, `format`, `extension`, `placeholder`, `trimmed`, `registry`, `loader_dependent`, `looks_like_key` (an echoed value that looks like a secret), `gitignore_failed`.
- **`verified`:** only for local sinks (`dotenv`, `file`, `keychain`), which passdrop can read back. Remote sinks report the CLI's success instead.
- **`error`:** an error code, present on `failed`.
- **`next`:** fixed text. It never suggests a command that would print a value.
- **Never in the output:** value length, hash, prefix or fingerprint. `unchanged` already answers "is it the same value?", and `verified` answers "did the write work?" (§12).

### 9.2 `SKILL.md` rules

- Never ask for a secret in the chat, and never offer "or just paste it here".
- Ask for every needed value in one `request` call.
- Before running, tell the user in one line that a window is opening.
- Never invent a stand-in value.
- Never read or print a written value. Confirm by behaviour (does the build or API call work?), not by echoing.
- If the user declines (`cancelled`), don't re-ask in the same task. Make the feature optional or stop.
- Use `x-known` for well-known keys, and `description` for anything else the human may not know how to obtain.
- **Claude Code:** run in the foreground with `timeout: 600000`. If the call times out it moves to the background and is not killed.
- **Codex:** the call yields within 30 s. Keep polling until the JSON line appears.

### 9.3 Install and permissions

- **Install:** `passdrop init` writes the skill to `~/.claude/skills/passdrop/` and `~/.agents/skills/passdrop/` (Codex reads the latter). `--undo` removes both.
- **Codex:** passdrop must run outside the sandbox. `init` **prints** both options and writes neither:
  - (a) per-run approval through the normal escalation prompt (the default);
  - (b) a prefix rule in `~/.codex/rules/default.rules`: `prefix_rule(pattern=["passdrop"], decision="allow")`. Trade-off: passdrop then always runs unsandboxed without a prompt, so its own window becomes the only approval step.
- **Claude Code:** the sandbox is off by default, so nothing is needed. With the sandbox on, add `passdrop *` to `sandbox.excludedCommands`.
- **Deny rules:** `init` **prints** recommended rules and does not write them; the user's settings are the user's. Examples:
  - Claude Code: `permissions.deny: ["Read(**/.env)", "Read(**/.env.*)", "Read(~/.appstoreconnect/**)"]`.
  - Codex: a permission profile with `"**/*.env" = "deny"`.

## 10. Security model

**What it protects against**

- The value entering the agent's context or the transcript.
- The agent choosing an arbitrary or dangerous destination (fixed allowlist plus guards).
- Agent text posing as something else: visible domains, registry URLs win over agent URLs, every destination shown, echo always explicit.
- Exposure through command-line arguments (stdin only).
- Lingering copies in memory, on a best-effort basis (`secrecy`, `zeroize`).

**What it does not protect against**

- The agent reading the destination afterwards. Mitigated by deny rules (§9.3).
- The original of a dropped file, if the human keeps it.
- A spoofed caller: the provenance header is a hint, since the agent controls passdrop's environment.
- Other malicious local processes.
- The human submitting a wrong value despite warnings.

**Tests**

- **Canary leak test in CI:** a test-only build feature replaces the window with a JSON input file. CI runs the real binary with a fake value and asserts it never appears in stdout, stderr, `ps` arguments, temp files or the result.
- **Round-trip tests** for the dotenv writer: quoting, CRLF, `export`, duplicates, mode.
- **Registry tests:** every pattern compiles.

## 11. Tech stack

| Area | Choice | Note |
|---|---|---|
| Language | Rust | The machine has 1.93. eframe 0.36+ needs 1.95 (§13). |
| GUI | `eframe` / `egui` | wgpu renderer (Metal on macOS) by default. glow saves about 2.3 MB but uses OpenGL, which is deprecated on macOS. Measure before switching. |
| Spec | `serde`, `serde_json`, `schemars` | `schemars` generates the output of `passdrop schema`. |
| Validation | `regex` | Linear time, so an agent-written pattern can't hang passdrop. |
| Secrets in memory | `secrecy`, `zeroize` | |
| Keychain | `security-framework` | macOS only in v1. `keyring` 4.x was considered; its new store-based API was not worth the risk for one platform. |
| Files | `tempfile` (atomic write), `dirs`, `trash` | `trash` moves dropped originals to the Trash. |
| Generation | `getrandom`, `base64`, `uuid` | |
| External CLIs | `std::process` | For `gh` and `vercel`. No async runtime. |

Release profile: `lto = true`, `codegen-units = 1`, `strip = true`, `panic = "abort"`.

## 12. Decisions made

- **Window:** native egui window, not a localhost web form. No port, no token, and out of reach of browser page-reading tools.
- **Validation:** warn only. Hard stops are limited to spec errors, destination guards and size limits.
- **Output:** no length, hash or fingerprint (§9.1).
- **Echo:** always explicit (`x-echo: true`) and shown in the window as visible to the agent.
- **Sinks:** a fixed allowlist compiled in. Any future configuration comes only from the user's global config, never from a repository.
- **Registry:** our own file, seeded from tokenstash's `providers.json` (MIT, with attribution). Prefix-only patterns, warn only, with a source and verified date per entry. A spec can add to or override entries.
- **`pattern`:** follows JSON Schema semantics (unanchored).
- **dotenv quoting:** the tested grammar in §7.3 (single quotes first), not the untested "double quotes only" rule from the prior-art survey.
- **`x-known`:** takes an env-var name. The registry has no provider ids, and some providers have several entries.
- **Registry import:** generic entries whose upstream URL points at one vendor or a reference page (`DATABASE_URL`, `SESSION_SECRET`, `JWT_SECRET`, `ENCRYPTION_KEY`) keep their pattern and steps but lose the URL, so they can't override an agent's correct URL.
- **Worktrees:** `.claude/worktrees/**` and `.codex/worktrees/**` are writable; the rest of `.claude` and `.codex` is not.
- **Window behaviour (milestone 2):**
  - **Keep/Replace gates the input:** a field that is already set shows no input until Replace (or Regenerate) is chosen, so typing is never silently dropped.
  - **Drop routing:** macOS sends no mouse-move events during an external file drag, and winit's drop events carry no position. passdrop reads the global mouse position from CoreGraphics (`CGEventCreate` + `CGEventGetLocation`, no permission needed) and maps it into the window. The drop goes to the field under the cursor, and only that zone is highlighted. Without a position (other platforms), it falls back to egui's pointer if it moved, then to the only empty file field, then to asking.
  - **Trash:** macOS uses the NSFileManager method. Finder's method would ask for an extra automation permission. The trade-off is that "Put Back" may be missing on some systems.
  - **Links:** a host check rejects userinfo (`@`), backslashes and non-ASCII. The displayed host is the real one.
  - **Look:** SF Pro and SF Mono when present, with a symbol fallback font (egui's own fonts lack `→`); cards and chips; separate light and dark palettes.
  - **Wide fallback font:** Arial Unicode (23 MB) is loaded only when the spec contains scripts SF Pro lacks (CJK, Arabic, Hebrew, Indic, …). It cost about 47 MB of peak memory in a screenshot-mode measurement.
  - **Renderer:** wgpu stays. glow saved about 12 MB resident and 2.3 MB of binary in an empty window, which doesn't justify OpenGL, deprecated on macOS.
  - **Test-only paths:** `--test-input` and `--screenshot` exist only with the `test-harness` cargo feature, and the release binary doesn't contain them.
- **Overwrite:** Keep is the default in the core, not only in the window. Replace must be explicit, and a value that appears while the window is open is never overwritten (§7.2).
- **Platforms:** macOS first. The core uses Unix APIs (modes, uid), so Linux should work but is untested; Windows is out of scope for v1.
- **Rust:** milestone 1 needs no GUI crate, so the toolchain question (§13.2) waits for milestone 2.

## 13. Open questions

1. Final name.
2. Update Rust to 1.95 and use eframe 0.37, or stay on 0.35?
3. v1 platforms: macOS only, or macOS + Linux?
4. Codex default: recommend per-run approval or the prefix rule?
5. ~~Does egui's `TextEdit` keep extra copies of masked text?~~ **Answered:** yes, its undo history keeps copies. passdrop clears it every frame for masked fields. A headless egui test confirms this, and also confirms the accessibility tree exposes only bullets.
6. Window activation when launched from Terminal or iTerm, and from Codex.
7. `vercel env add` stdin and `--force` behaviour was checked only on Vercel CLI 54.14.2.
8. Keychain items created by passdrop: other apps or CLIs reading them will likely see a one-time macOS access prompt ("Always Allow"), because passdrop is the creating app. Unverified on a real keychain.

## 14. Milestones

1. **Core:** spec parsing and checks, warnings, registry, `dotenv` and `file` sinks, guards, result JSON, unit tests. *Done 2026-11-11, plus a CLI with `validate`, `check` and `schema`. Remote sinks are typed but return `not_implemented`; moving dropped originals to the Trash waits for the window.*
2. **Window:** egui form, provenance header, Keep / Replace, timeout thread, `no_gui`. *Live-tested 2026-11-12:*
  - *Confirmed:* saving through the window; a real drag onto the second of two file fields (after a fix: the drop position is now read from macOS); the real Trash; launching from Terminal.app (the window came to the front by itself); the light theme.
  - *Found and fixed:* drops were routed by asking instead of by position; a wrong file type defaulted to the Trash.
  - *Not explicitly confirmed by the tester:* hold-to-show, Enter/Esc, the link button, the Keep/Replace toggle (the result showed `kept`).
  - *Not tested:* launching from Codex.

  *Built 2026-11-11. Includes `passdrop request`, moving dropped originals to the Trash, the "registry URL wins" rule, a design pass (system fonts, cards, light and dark themes), and a test harness behind a cargo feature. Release binary 8.7 MB. About 96 MB resident with the full form open, against 107 MB for an empty wgpu window and 95 MB for an empty glow window. First frame 140–320 ms. Not yet verified: Save through the real window, a real drag onto one of two file fields, the real Trash, launching from Terminal or Codex, and the light theme.*
3. **Remote sinks:** `keychain`, `gh-secret`, `vercel-env`. *On the `remote-sinks` branch, not merged into `main`.* *Built 2026-11-12 and tested against fakes: an in-memory keychain and shell scripts standing in for `gh` and `vercel`. Not yet run against the real keychain, GitHub or Vercel.*
   - Values go to `gh` and `vercel` on stdin only. Their error output is relayed as one line with the value redacted.
   - `vercel env ls --format json` includes the values of plain variables. passdrop parses it in memory for key names only and zeroizes the buffer.
   - GitHub and Vercel can't be read back (`verified` absent), so a dropped original is never trashed for them. A keychain write is verified only when passdrop created the item.
   - Already set + no Replace → `kept`; set after planning → `exists`; with Replace → `--force` for Vercel and an update for the others.
   - `--sensitive` is passed only for production and preview; Vercel rejects it for development.
   - GitHub's 48 KB secret limit is checked before calling `gh`.
   - A missing `gh` or `vercel` becomes a plan warning (`tool_missing`) and a field error.
4. **Agent integration:** `SKILL.md`, `init` / `--undo`, permission docs. *Done 2026-11-12: `skills/passdrop/SKILL.md` (shared by the Claude Code plugin and `init`); `.claude-plugin/` plugin and marketplace manifests, validated with `claude plugin validate --strict`; `init` prints permission suggestions and never edits settings.*
5. **Hardening and release:** canary test, accessibility check, CI builds, Homebrew tap. *Done 2026-11-12: canary leak test through the real binary; headless egui test that the accessibility tree and undo history hold no secret; CI on macos-latest (macos-14 runners are retired); v0.1.0 released as a universal, ad-hoc-signed macOS binary with checksums; `brew install akcware/tap/passdrop`. Not yet tried: installing the plugin from GitHub in a real Claude Code session, and running from Codex.*
