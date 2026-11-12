---
name: passdrop
description: Ask the human for secrets and private values (API keys, tokens, .p8/.pem key files, passwords, phone numbers) through a local passdrop window that writes them straight to .env files or key paths, so the values never enter the chat. Use whenever a task needs a credential or private value only the user has, instead of asking them to paste it.
allowed-tools: Bash(passdrop --version) Bash(passdrop request *) Bash(passdrop validate *) Bash(passdrop check *) Bash(passdrop schema)
---

# passdrop

When a task needs a value only the human has, such as an API key, a token, a `.p8` or
`.pem` key file, a password or a private phone number, **never ask for it in chat**.
Use passdrop instead:

1. You write a small JSON spec: which fields you need, and where each value goes.
2. `passdrop request spec.json` opens a native window. The human fills it in.
3. passdrop writes each value to its destination and prints **one JSON line of
   metadata**. You never see the values.

## Rules

1. Never ask the user to paste a secret in chat, and never offer "or just paste it here".
2. Ask for every value the task needs in **one** request.
3. Before running, tell the user in one line that a passdrop window is opening.
4. Never invent stand-in values (`your-key-here`, `sk-test-123`).
5. Never read or print a written value: no `cat .env`, `grep KEY .env`, `echo $KEY` or
   `printenv`. Confirm by behaviour instead, by running the build, test or API call
   that uses the value.
6. If the result is `cancelled` or `timeout`, don't ask again in the same task. Make the
   feature optional, or stop and say what is missing.
7. Mark secrets `"writeOnly": true`. Use `"x-echo": true` only for non-secret values you
   need to see, such as a public ID. Echoed values appear in the chat.

## Before the first request

Run `passdrop --version`. If the command is not found, tell the user how to install it
and stop:

```bash
brew install akcware/tap/passdrop
```

## Write the spec

```json
{
  "title": "OpenAI and App Store keys",
  "x-purpose": "Call the OpenAI API and upload builds with the App Store Connect API",
  "type": "object",
  "properties": {
    "openai_key": {
      "title": "OpenAI API key",
      "type": "string",
      "writeOnly": true,
      "x-sink": { "kind": "dotenv", "path": ".env.local", "key": "OPENAI_API_KEY" }
    },
    "asc_key": {
      "title": "App Store Connect API key (.p8)",
      "description": "App Store Connect → Users and Access → Integrations → Team Keys → +",
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
    "session_secret": {
      "title": "Session secret",
      "writeOnly": true,
      "x-generate": "hex:32",
      "x-sink": { "kind": "dotenv", "path": ".env.local", "key": "SESSION_SECRET" }
    }
  },
  "required": ["openai_key", "asc_key", "issuer_id"]
}
```

Every field needs `x-sink`, `"x-echo": true`, or both.

**Destinations (`x-sink`)**

| `kind` | Shape | Notes |
|---|---|---|
| `dotenv` | `{ "kind": "dotenv", "path": ".env.local", "key": "NAME" }` | Edits only that key's lines. New files are created with mode 600. |
| `file` | `{ "kind": "file", "dir": "…/", "name": "original", "mode": "600" }` or `{ "kind": "file", "path": "…" }` | `"name": "original"` keeps the dropped file's name. |

Paths are relative to the current directory, or start with `~/`. Protected locations
(`~/.ssh`, `.git/`, `.github/`, agent config folders, `/etc`, …) are refused.

**Field keywords**

| Keyword | Meaning |
|---|---|
| `title` | Label. Required. |
| `description` | Help text, e.g. where to get the value. `https://` links become clickable. |
| `writeOnly: true` | Secret: masked, never returned to you. |
| `x-echo: true` | Return the value to you (visible in chat). Not allowed with `writeOnly`. |
| `x-widget` | `"text"` (default), `"textarea"` (multi-line) or `"file"` (drag and drop). |
| `x-accept` | Expected file extensions, e.g. `[".p8"]`. |
| `x-generate` | `"hex:32"`, `"base64:32"` or `"uuid"`. passdrop generates the value; nobody types or sees it. |
| `pattern`, `x-prefix`, `minLength`, `maxLength`, `format` | Format hints. They **only warn**, never block, because key formats change. |
| `x-known` | Registry name such as `"OPENAI_API_KEY"`. A dotenv key with a well-known name is looked up automatically, and the window then shows where to get the key. |
| `enum` | A select list. |
| `default`, `examples` | Prefill or placeholder. `default` is not allowed on secrets. |

Top level: `title` and `x-purpose` are required. `x-purpose` is one line on why you need
the values, shown to the human. `required` lists the fields that must be filled.
Run `passdrop schema` for the full JSON Schema.

## Run it

```bash
passdrop validate spec.json   # spec and destination errors, no window
passdrop request spec.json    # opens the window; prints one JSON line when done
```

- **Claude Code:** run `passdrop request` in the foreground with a 10-minute timeout
  (600000 ms). The human may take a while.
- **Codex:** the window can't open inside the sandbox. Request escalation for this command,
  with a justification such as "open the passdrop window so the user can enter the
  keys". If the call returns while the window is still open, keep polling until the
  JSON line appears.
- `passdrop check spec.json` reports which values are already set, without values and
  without a window.

## Read the result

```json
{"status":"ok","fields":{"openai_key":{"action":"created","ref":"env-file:.env.local#OPENAI_API_KEY","verified":true},"issuer_id":{"action":"echoed","value":"57246542-…"}},"next":"Values are in place. Do not read or print them; use them by reference."}
```

| `status` | Exit | What to do |
|---|---|---|
| `ok` | 0 | Continue. Use the values by reference: the env var name or the file path. |
| `partial` / `failed` | 20 / 21 | Read each field's `error` and `message`. Fix the spec or ask the user to run the request again. Don't ask for values in chat. |
| `cancelled` | 10 | The user closed the window. See rule 6. |
| `timeout` | 11 | Nothing was written. See rule 6. |
| `no_gui` | 12 | No screen is available (e.g. SSH). Tell the user to run the same command on their machine. |
| `invalid_spec` / `usage` | 2 | Fix the spec. Each error has a `path` and a `code`. |

Per-field `action`:

- `created`, `updated`, `unchanged`, `generated`: written.
- `kept`: already set, and the user chose Keep.
- `skipped`: optional and left empty.
- `echoed`: returned to you.
- `failed`: see `error`.

`warnings` such as `registry` or `placeholder` mean the value looked unusual. The user
saw the same warning and saved anyway.
