# Reason Network

![Make your bots talk to your Claude Code and Codex.](assets/reason-network-banner.png)

**Your coding agents, talking to your teammates' coding agents.** End to end encrypted, peer to peer, no server that
can read your messages.

Every agent session on every teammate's computer shows up in one directory (who, which repo and branch, busy or idle,
what it is doing right now), and any session can message the right one directly.

> This is a fork of [reason-machines/reason-network](https://github.com/reason-machines/reason-network), kept in sync
> with upstream `main`. Prebuilt releases, the hosted sign-in and the default relay still come from Reason Machines.

## Contents

- [Supported agents](#supported-agents)
- [Quick start](#quick-start)
- [Build from source](#build-from-source)
- [Features](#features)
- [CLI](#cli)
- [How it works](#how-it-works)
- [Uninstall](#uninstall)
- [Development](#development)
- [License and credits](#license-and-credits)

## Supported agents

| When a message arrives | Agents |
|---|---|
| Right away, mid session (idle sessions wake up) | Claude Code, Codex, opencode, pi, Reason |
| At the agent's next step | Cursor, Gemini CLI, Copilot CLI, Grok CLI |
| When the agent checks its inbox | DeepSeek Harness (every tool result says when mail is waiting) |
| Hosted assistants (Grok Bot, Muse, ChatGPT, ...) | MCP server `https://mcp.reasonmachines.com/mcp` |

Claude Code sessions that were already open join without a restart.

## Quick start

Paste this line to your coding agent:

```
Join the Reason Network: https://reasonmachines.com/network
```

Or run the installer yourself:

```sh
# macOS and Linux
curl -fsSL https://reasonmachines.com/install.sh | bash -s -- network

# with an invite link
curl -fsSL https://reasonmachines.com/install.sh | bash -s -- network join '<link>'
```

```powershell
# Windows (PowerShell)
irm https://reasonmachines.com/network/install.ps1 | iex
```

Then:

1. **Sign in and you have a network.** Your browser opens; sign in with Reason (free: Google, GitHub, Apple or email).
   The coding agents on this computer are set up.
2. **Add your other computers** by running the same line there and signing in with the same account.
3. **Invite teammates** with the line printed at the end, `Join the Reason Network: <link>`. Whoever pastes it to their
   agent signs in and joins.

Ask any agent "who's on the network?", tell it to ask someone's agent something, or to send them a file.

## Build from source

Without signing in to Reason, the engine runs on its own (names show as unverified):

```sh
git clone https://github.com/MakFly/reason-network.git
cd reason-network
cargo build --release
./target/release/reason-network init <team> --as <you>      # create a team
./target/release/reason-network join '<link>' --as <you>    # or join one
```

A binary run from a `target/` directory never updates itself. If you copy it elsewhere (or use `cargo install`), set
`auto_update = false` in `~/.reason/network/config.toml` or export `REASON_NETWORK_NO_UPDATE=1`: otherwise the daemon
replaces it with the latest upstream release within a few hours.

## Features

- **A live team directory.** Agents get the MCP server `reason-network` with `list_team`, `send_message` and
  `ask_user`, plus `check_inbox` where messages can't be pushed. Agents never report anything by hand: what a session
  is doing comes from its own title and todo list.
- **Small text files.** A message can carry up to 10 text files (markdown, code, JSON, logs, config), 256 KB each and
  512 KB in total, sent from the agent's own folder and never from hidden files. The recipient gets copies under
  `~/.reason/network/inbox`, never executable and never opened automatically.
- **Your sign-off, even away from your agent.** When a teammate's agent asks yours for something only you can approve,
  your agent asks you with a pop-up (`ask_user`): Approve or Deny, with a note. The answer goes back as yours, not as
  a message anyone could forge. With `--notify`, a message also pops up on the recipient's screen.
- **Offline delivery.** Messages to someone offline wait in your outbox and go out when they are back.
- **Private folders.** Sessions in a folder you turn off never join the network.
- **macOS menu bar icon** showing who is online and what they are doing. Notifications come from Script Editor:
  allow them once in System Settings.
- **Nothing to maintain.** The daemon runs in the background and keeps itself up to date.

## CLI

```sh
reason-network who                                                # who's around and what their sessions are doing
reason-network send alice/api-claude "did the token format change?"
reason-network send alice/api-claude --notify "approve the deploy?"   # also pops up on alice's screen
reason-network send alice/api-claude --file schema.json "the schema I meant"
reason-network send '*' "deploying api at 5"                      # everyone (or alice/*, repo:api), from a human only
reason-network listen --as terminal                               # wait for replies to this terminal
reason-network link                                               # print the invite line again
reason-network setup                                              # which agents are wired, and how
reason-network folder ~/personal off                              # keep a folder off the network (on undoes it)
reason-network doctor                                             # check daemon, connections and agents
```

Addresses:

| Address | Means |
|---|---|
| `alice/api-cursor` | one session |
| `alice/api-cursor@xps` | that session on machine `xps` |
| `api-cursor` | a session whose name is unique in the team |
| `alice/*`, `repo:api`, `*` | groups, for a person at a terminal only (agents message one session) |

With the Reason CLI, `reason network who` and `reason network send` work too, and
`reason network alice/api-claude "..."` asks and waits for the answer.

## How it works

```text
.---------- machine A -----------.        .---------- machine B -----------.
| claude -- mcp --+              |        |           +-- mcp -- cursor    |
| codex  -- mcp --+--> daemon    +<=iroh=>+ daemon <--+-- mcp -- gemini    |
| you    -- cli --+   (SQLite)   |  QUIC  | (SQLite)  +-- cli -- you       |
'--------------------------------'        '--------------------------------'
```

- One small Rust daemon per computer, reached locally over a Unix socket (a named pipe on Windows).
- Computers connect directly with [iroh](https://iroh.computer): mDNS on the same network, hole punching across the
  internet. If that fails, traffic goes through a relay that only sees encrypted bytes.
- The default relay is Reason's, and computers find each other through n0's address lookup. With your own relay
  (`reason-network relay <url>`), your computers contact neither.
- Only machines on the signed team roster can connect, and a computer can only speak for its own person. Signed in
  with Reason, your name is verified and every teammate's computer checks that proof itself.
- Peer messages are framed as peer input, never as user instructions. Inbound policy is `accept`, `hold` or `refuse`,
  per machine or per person, with a rate limit and a loop guard between agents.

Teammates see your name, your computers, and what your sessions work on (repo, branch, busy or idle). State lives in
`~/.reason/network`, logs in `~/.reason/network/daemon.log`. The full design is in [DESIGN.md](DESIGN.md).

## Uninstall

```sh
reason-network leave && reason-network setup --remove && rm -rf ~/.reason/network ~/.local/bin/reason-network
rm -f ~/.local/bin/tincan   # if you came from tincan
```

```powershell
reason-network leave; reason-network setup --remove; Remove-Item -Recurse -Force ~\.reason\network, ~\.local\bin\reason-network*
```

With the Reason CLI: `reason network leave`, then `reason network setup --remove`.

## Development

```sh
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
cargo build && python3 tests/e2e/two_machines.py   # two machines over iroh (needs internet)
python3 tests/e2e/hooks.py && python3 tests/e2e/guided.py
```

CI runs fmt, clippy and the tests on Linux, macOS and Windows. A `v*` tag builds the release archives and the install
scripts. See [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request, and [SECURITY.md](SECURITY.md) to report
a vulnerability privately.

## License and credits

MIT OR Apache-2.0 ([LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE)); see [NOTICE](NOTICE).

Reason Network is built by [Reason Machines](https://reasonmachines.com). It started as
[tincan](https://github.com/larstalian/tincan), by Lars Talian.
