# An SSH Client Built to Kill Every Pain Point

The SSH protocol turned thirty last year. The tooling around it still behaves like it's 1999: a connection list, a byte stream, and your eyeballs. [rssh](https://github.com/shihuili1218/rssh) is an open-source SSH client (Rust + Tauri, MIT, macOS/Windows/Linux/Android/iOS plus a JetBrains plugin) that started from a pain-point list rather than a feature list. Every design decision below exists because something in daily server work hurt, and each fix is applied at the layer that actually caused the pain — not bolted on top with a settings toggle.

Six pain points, six fixes.

---

## Pain 1: "Where does this output begin?"

You run `tail -1000 prod.log`, then three more commands. Now you want yesterday's stack trace. The terminal answer: scroll up and find the first line by eye. Want to paste it into an issue? Enjoy the ANSI escapes and soft line breaks. Want to ask an AI about it? All you can hand over is a screenful of bytes.

The terminal is a byte renderer — it has no concept of "a command". Some modern terminals fix this with shell-integration scripts on the server. That works until you hit a bastion host with no sudo, a customer box that forbids polluted dotfiles, or a `kubectl exec` pod that doesn't have your script. Installing hooks on ten thousand machines is not an answer.

**The fix.** rssh cuts the stream on the client side. Press Enter, and the client records a marker; the next Enter closes one block and opens the next. The two endpoints are xterm.js `IMarker`s, so blocks migrate with the scrollback and survive resizes. The whole module is ~200 lines (`src/lib/terminal/command-blocks.ts`). Nothing is installed anywhere — first second inside someone else's pod, the blocks are already there.

The color-coded left edge is just the visible part. The real payoff is that `block.start..block.end` is an **object**, and everything else grows out of it:

- **Copy as plain text** — ANSI stripped, soft wraps merged; what lands in your clipboard is usable
- **Copy as image** — the same slice redrawn on a canvas; colors, bold, and CJK wide characters intact
- **Fold** — the block is genuinely spliced out of the buffer, not hidden with CSS while the buffer stays fat
- **Audit** — per-block records give you "what ran and what it printed", not 12 KB of scrollback

Finding where an output begins goes from an archaeology skill to a click.

![Command blocks](welcome-blocks.gif)

## Pain 2: Want sync? Hand over your private keys

The moment you want a modern experience — a GUI, your phone, two laptops — the mainstream answer is: register an account and upload your keys, passwords and passphrases to the vendor. Their word that it's "end-to-end encrypted" is the whole guarantee, because the client is a closed binary. When the vendor gets breached, the SSH credentials of their entire user base leak together.

The assumption smuggled in here: sync requires central custody. It doesn't. Custody and sync are two different things.

**The fix.** Unbundle them.

- **Custody**: passwords and key passphrases live in the operating system's keychain — macOS Keychain, Windows Credential Manager, Linux Secret Service. The rssh process holds no long-term secrets; it asks the keychain each time. The in-memory passphrase cache is wrapped in `Zeroizing` and zeroed on drop.
- **Sync**: profiles, forward rules and snippets are encrypted and pushed to **your own** private GitHub repo or WebDAV server. rssh has no server. There is no rssh account.
- **Private keys stay off the cloud by default**: each credential has its own `save_to_remote` toggle, off by default. Keys rarely change — copy one over once and you're done; if you really want it synced, the knob is yours.

The crypto is `src-tauri/src/crypto.rs`, ~230 lines: Argon2id key derivation with parameters pinned as constants (19 MiB / 2 iterations / 1 lane, aligned with the OWASP 2024 baseline — deliberately not `Argon2::default()`, which can drift across crate versions), and ChaCha20-Poly1305 AEAD. What GitHub stores is ciphertext. Lose trust in the tool, and your data is still just an encrypted file in your own repo with a documented wire format. No lock-in, because there was never a lock.

![Sync](welcome-sync.gif)

## Pain 3: GUI data you can't reach from anywhere else

Mainstream tools keep connection data locked inside the GUI. Want that host in tmux? In a Makefile? In CI? Best case you get an "export CSV". Your data turned into the tool's data.

**The fix.** The GUI and the CLI are two views of one SQLite file (`~/.rssh/rssh.db`). Maintain profiles in the GUI on the weekend, then from inside your tmux on Monday:

```console
$ rssh profile list
$ rssh profile open prod-web-1
```

The CLI manages what the GUI manages: `profile list/open/add/edit/rm`, `credential`, `forward`, `group`, plus `config export/import/sync`. Shell completion (zsh, bash, fish, PowerShell) completes *profile names* dynamically, so aliases and scripts Just Work. Bare `rssh` launches the desktop app — one binary, no split brain.

The same principle everywhere else: host keys read and write the system `~/.ssh/known_hosts` directly (one source of truth with the command-line `ssh`), host configs import from `~/.ssh/config`, the JetBrains plugin runs the full UI on the same `~/.rssh`, and session recordings are asciicast v2 — playable by anything that speaks asciinema's open format.

## Pain 4: The same command, twelve machines, one human

The operational reality of "restart the service and check logs" is rarely one machine. The classic answer is a for-loop over `ssh` in a script — where the loop is faster than the authentication, the output interleaves, and you still eyeball twelve panes.

**The fix.** The Workbench (desktop) shows live previews of every connected session in one grid. Select the sessions you care about, open the broadcast editor, and type once — the text goes to all selected terminals simultaneously. The editor is selection-aware: send the whole snippet or exactly the text you highlighted. Multi-cursor selections are joined and sent together. Combined with snippets (reusable command shortcuts, one keystroke away), "run this on the fleet, watch it happen" becomes one action.

This is deliberately *not* a workflow engine. It's the interactive version of orchestration — the operator stays in the loop, sees every terminal, and nothing is scheduled in the dark.

## Pain 5: The human as a USB cable between AI and the server

It's 2026, and "AI-assisted debugging" still means: run a command, read the output, switch to the browser, paste it to the model, get a command back, switch to the terminal, paste, run, copy the output back to the model. A human doing the job of a cable — while tokens, passwords and internal IPs leave the machine verbatim, and ANSI noise garbles whatever survives the trip.

The root cause: the AI and the terminal live in two worlds with only a clipboard between them. But the SSH client sits in the middle of every byte of I/O. It is the natural bridge; nobody built it.

**The fix.** rssh gives the LLM four tools and lets it drive the diagnosis; the human only adjudicates:

```text
run_command(cmd, explain, side_effect, timeout_s?)
download_file(remote_path, max_mb)
analyze_locally(local_path, task)
load_skill(id)
```

Every proposed command arrives as a card: the command, a one-sentence plain-English explanation, and an explicit side-effect declaration — propose `jmap -histo:live` and it must say "triggers a Full GC". Only after you click Approve does the command run — in your own interactive terminal, in full view, executed via a sentinel echo (`cmd; echo "<uuid>:$?"`) so the exit code is captured and the output is sliced at the block boundary (Pain 1's object, consumed here). Before anything leaves your machine it is redacted and truncated; GB-scale output is diverted to `download_file` + local analysis instead of flooding the context.

None of this depends on prompt self-discipline. Four walls are enforced in Rust: structural validation rejects dangerous commands before they're ever shown, every command requires human approval, payloads are redacted locally, and output size is bounded.

And because the AI works through your existing SSH session, there is **nothing to install on any server**. Bastion behind three jumps, customer firewall, throwaway pod — the AI operates them exactly like a human operator would: by reading the terminal and typing.

![AI triage](welcome-ai.gif)

## Pain 6: Static addresses in an elastic world

The connection-list model assumes machines are stable, few, and long-lived. Today: twelve Docker containers whose names all changed overnight, Kubernetes rolling deploys handing every pod a new IP, one laptop carrying three docker contexts and two kube contexts. Hand-maintaining an "IP → machine" list here is navigating a highway with a paper map — the list is always stale and the dead entries pile up.

**The fix.** Dynamic discovery with two rules.

**Discover identities, not addresses.** rssh reuses your local `docker` and `kubectl` CLI contexts to discover running containers and pods. No new probing protocol, no agents on servers, no auth chain bypassed — it sees exactly what your own CLI can see, honoring the contexts you already configured. Home shows the targets that are alive *right now*: a container restarts and the old target disappears; a new one comes up and a new target appears. There is no dead config to clean.

**Discovery results are not config.** rssh persists only the discovery *source* (platform + context + namespace), never the results. Dynamic targets don't enter the profile database and can't be favorited — favoriting a pod that's about to vanish is meaningless. Opening one creates an ephemeral `docker exec -it` / `kubectl exec -it` connector that sits in the same list as static profiles and forwards, behind the same search.

What gets configured is the stable entry point; what changes is discovered live. That's what "elastic" should have meant all along.

![Dynamic discovery](welcome-discovery.gif)

---

## One thread through all six

None of these are features you'd find on a comparison chart. Read them again and there's a single move repeated six times: find the layer where the pain is actually created, and change the data model there.

- The byte stream becomes **block objects**, so locating, copying, folding, auditing and AI consumption all fall out for free.
- Secrets move to **your keychain** and sync becomes **your repo** — the vendor disappears from the trust chain.
- The GUI's database becomes **the CLI's database** — one store, every surface.
- N live terminals become **one broadcast editor** — the operator stays, the drudgery doesn't.
- The AI gets **the same objects the human sees**, behind approval walls enforced in code — the human goes from porter to adjudicator.
- Ephemeral infrastructure becomes **discovered identities**, not hand-maintained addresses.

rssh is open source (MIT), built in Rust with Tauri and xterm.js, speaks SSH via `russh`, and runs on macOS, Windows, Linux, Android and iOS with a JetBrains plugin sharing the same data directory. If any of the six pains sounded like your Tuesday, [the downloads are here](https://github.com/shihuili1218/rssh/releases/latest).
