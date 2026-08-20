# The Five Sins of Traditional SSH Tools

The SSH protocol was published in 1995. The protocol itself is hard to fault — OpenSSH remains a textbook of security engineering to this day. What's rotten is the layer of tooling wrapped around the protocol: thirty years on, several historical mistakes haven't just gone uncorrected — they've been curated into "tradition."

This is not a rant about UI taste. UI is a matter of preference; these five sins are all structural: wrong abstractions, wrong trust model, wrong data ownership. Behind each sin stands one wrong default assumption:

1. Between AI and the terminal, a human acting as the data cable is the natural order
2. Terminal output is a byte stream, and the terminal is nothing more than a byte-stream renderer
3. Wanting a modern experience means handing over your keys
4. One machine equals one IP
5. Your data is really the tool's data

Let's try them one by one.

---

## Sin 1: Making the human a porter for AI

**The charge.**

It's 2026, and the mainstream way of "AI-assisted debugging" still looks like this:

```text
run a command in the terminal → read the output with your own eyes
  → switch to the browser, copy, paste it to the AI
  → AI replies with a command → copy, switch back to the terminal, paste, hit Enter
  → copy the output back to the AI → ……
```

A human stands between the LLM and the terminal, doing the job of a USB cable. And the loop ships with three free bonuses:

- Tokens, passwords, and internal IPs go out verbatim — nothing gets redacted for you
- ANSI escapes, wide characters, progress-bar spam — pasted into a web page, it's all garbage
- Once the context grows long, truncation kicks in and the earlier diagnostic steps are gone

**The root cause.**

AI and the terminal live in two separate worlds, and the only bridge is the clipboard. The clipboard doesn't understand commands, doesn't understand output boundaries, and doesn't do redaction — it's just a byte-carrying trough.

Yet the SSH client already sits in the middle of every command's I/O — every byte in and out passes through its hands. What only it can do, nobody has done for thirty years.

**rssh's fix.**

![welcome-ai.gif](welcome-ai.gif)

Give the LLM four tools and let it drive the diagnosis itself; the human only adjudicates:

```text
run_command(cmd, explain, side_effect, timeout_s?)
download_file(remote_path, max_mb)
analyze_locally(local_path, task)
load_skill(id)
```

Every command arrives as a card: the command itself, a one-sentence plain-English explanation, an explicit side-effect declaration (run `jmap -histo:live` and it must say "triggers a Full GC"), plus Approve / Reject. Only after you click approve is the command pasted into your active terminal and executed — in your own interactive terminal, fully visible, no backend injection. The exit code is recovered via a sentinel echo (`cmd; echo "<uuid>:$?"`), and the output is automatically sliced, redacted, and truncated before going back to the LLM.

Security doesn't depend on prompt self-discipline — all four walls are enforced in Rust: structural validation blocks dangerous commands, every command requires human confirmation, payloads are redacted locally before leaving the machine, and GB-scale output is truncated with large files diverted to local analysis.

The human goes from porter to approver. That's the division of labor 2026 deserves.

---

## Sin 2: Forty years on, the terminal is still a byte renderer

**The charge.**

`\x1b[31mHello\x1b[0m\r\n` comes in, render a red Hello, done. Who said "red"? Which command's output is this? Which line does it start on, where does it end? The terminal doesn't know — **it only knows bytes.**

So everyday operations all turn into archaeology:

- Copy the previous command's output: scroll up, find the starting line by eye
- Paste into a GitHub issue: drag along a screenful of ANSI escapes and soft line breaks
- Ask the AI "what's this error": all you can hand over is a screenful of bytes
- Audit what ran yesterday: there is no unit called "a command," only scrollback

**The root cause.**

The wrong abstraction was chosen. For forty years the terminal's outward interface has been a byte renderer; "a command" has never been an object. Half of Sin 1 — the AI being fed byte garbage — is rooted right here: if even humans can't cut the boundaries accurately, don't expect an LLM to.

Some terminals try to fix this by installing shell-integration scripts on the server. But machines behind a bastion host give you no sudo, a customer's firefighting box doesn't allow polluted dotfiles, the pod you `kubectl exec` into doesn't have your script at all — are you going to install it on ten thousand machines one by one? That's not an answer, that's relocating the lesion.

**rssh's fix.**

![welcome-blocks.gif](welcome-blocks.gif)

The client does the cutting itself, with zero server-side changes: the user presses Enter, record a marker; the next Enter closes the previous block and opens a new one. The two endpoints are xterm.js `IMarker`s — they migrate with the scrollback automatically and survive resizes. The core implementation is 117 lines.

What matters is that this cut yields an **object**: `block.start..block.end`. And then the capabilities grow out of it on their own:

- **Copy as text**: strip ANSI from those cells, merge soft line breaks — what you paste is immediately usable
- **Copy as image**: hand the same slice to a canvas redraw — colors, bold, CJK wide characters all intact
- **Fold**: genuinely splice that stretch out of the buffer — not CSS fooling the UI while the buffer stays fat
- **AI execution**: the segment before the sentinel is exactly this command's output — a block by nature
- **Audit**: record per block; what you read back is intent, not 12 KB of ANSI

---

## Sin 3: Want a modern experience? Hand over your private keys

**The charge.**

OpenSSH itself doesn't have this problem — keys sit in `~/.ssh/`, and nobody can take them. The problem is that in 2026 you have reasonable expectations of "nice to use": a GUI, multi-device sync, connecting from your phone. The mainstream answer: register an account, upload your private keys, passwords, and passphrases to the vendor's servers, and take their word for it that it's "end-to-end encrypted, don't worry."

How do you verify that? The client is a closed-source binary, and you can't see the server-side storage format. The day the vendor gets breached, the SSH credentials of their entire user base are out in the open together — no imagination needed, the precedents are all over the news.

What does the subscription buy you? An additional attack surface. Your private key used to live only in your own `~/.ssh/`; now there's a second copy sitting in somebody else's storage bucket.

**The root cause.**

The industry's implicit assumption: **to sync, there must be a central node that keeps custody of your data.** That assumption is wrong. Custody and sync are two different things, bundled together and sold for a decade-plus.

**rssh's fix.**

![welcome-sync.gif](welcome-sync.gif)

Unbundle them.

- **Custody**: passwords and private-key passphrases go into the operating system's keychain — macOS Keychain, Windows Credential Manager, Linux Secret Service. The rssh process holds no long-term keys; it asks the keychain each time it needs one. The passphrase cache is wrapped in `Zeroizing` and explicitly zeroed on Drop
- **Sync**: profiles, forward rules, and snippets are encrypted and pushed to **your own** private GitHub repo or WebDAV server — not rssh's server; rssh doesn't have a server at all
- **Private keys stay off the cloud by default**: each credential has its own `save_to_remote` toggle, off by default. Private keys don't change for a decade — copy one over with a USB stick and you're done; if you really want to push it, the knob is in your hand

The encryption implementation is 100 lines (`src-tauri/src/crypto.rs`): Argon2id key derivation (19 MiB / 2 iterations, OWASP baseline), ChaCha20-Poly1305 authenticated encryption, parameters pinned as constants. What GitHub receives is ciphertext only — without your password, it stays locked.

The day you're done with rssh, the backup file is sitting in your own repo, and the wire format is documented in the source comments. No lock-in, because there was never a lock.

---

## Sin 4: Forcing static IPs onto an elastic world

**The charge.**

The traditional client's core model is from the 1990s: create a Profile → fill in host, port, username → save → next time, click it from a list.

That model assumes machines are stable, few, and long-lived. Today's reality:

- 12 containers in Docker today; tomorrow their names and IDs have all changed
- Kubernetes rolling deploys — every Pod rebuild gets a new IP
- One laptop carrying multiple docker contexts and multiple kube contexts
- Elastic scaling in the cloud — the internal IP is a temporary attribute

Asking people to hand-maintain an "IP → machine" list in this world is navigating a highway with a paper map: the list is always stale, dead config piles ever higher, and the long-lived connections that actually matter drown in the noise.

**The root cause.**

"Connection target" was modeled as a static address. But today's targets — containers, Pods, cloud resources — are **identities**, not addresses. An address is a volatile implementation detail; the identity is what's stable. The data model is stuck in 1995 while the world went elastic long ago.

**rssh's fix.**

![welcome-discovery.gif](welcome-discovery.gif)

Dynamic discovery, two principles:

**One: discover identities, don't record IPs.** rssh reuses your local `docker` / `kubectl` CLI contexts to discover running containers and Pods — it invents no new probing protocol, installs no agent on any server, and bypasses no auth chain you've already configured. What appears in Home is "the targets alive right now under this context": a container restarts and the old target disappears; a new container comes up and a new target appears. There is no dead config to clean up.

**Two: discovery results are not config.** rssh persists only the "discovery source" (platform + context + namespace), never the discovery results. Dynamic targets don't enter the Profile database and can't be favorited — favoriting a Pod that's about to vanish is meaningless. Opening one produces a `docker exec -it` / `kubectl exec -it` connector that sits in the same list, as a peer of static Profiles and Forwards, going through the same search-and-open logic.

What gets configured is the stable entry point; what changes is the result. That's what earns the word "elastic."

---

## Sin 5: Walled gardens in proprietary formats — your data can't leave

**The charge.**

Open a mainstream SSH tool and you're surrounded by walls:

- It keeps its own host-key database — a host the command-line `ssh` already trusts needs confirming again in the GUI; one fact, two sources of truth
- Session recordings are a proprietary format, openable only by the vendor's own player
- Connection data is locked inside the GUI. Want to use it in tmux, CI, a colleague's terminal? There's no exit — at best you're thrown an "export CSV"

**The root cause.**

Not a technical limitation — a business-model prohibition. The day data is fully interoperable is the day switching tools costs nothing. Proprietary formats aren't a missing feature; they are carefully engineered exit costs.

**rssh's fix.**

Standardize everything that can be standardized, share everything that can be shared:

- Host keys read and write the system's `~/.ssh/known_hosts` directly — one set of data with the command line `ssh`
- Host configs can be imported from `~/.ssh/config`
- CLI and GUI read the same SQLite (`~/.rssh/rssh.db`): maintain profiles in the GUI, and `rssh profile open foo` connects from inside your tmux; aliases, Makefiles, CI — write them freely
- The JetBrains plugin shares the same `~/.rssh` as the desktop app — the IDE tool window shows the same hosts, keys, and settings
- Session recording uses asciicast v2 (asciinema's open format) — any player that supports it can play it
- The sync backup is just an encrypted file in your own repo, wire format documented in the source comments

The tool doesn't own your data; it's only borrowing it. You can leave at any time — that's a design goal, not a deficiency.

---

## The verdict

On the surface, five feature complaints. Dig down and it's one disease: **the tool stands between you and your server, collecting a toll.**

| Sin | Wrong default assumption | rssh's approach |
|---|---|---|
| Human as porter | AI and terminal can only meet via the clipboard | AI built into the I/O path, four walls, human only approves |
| Bytes only | Output is a byte stream, not an object | Enter cuts out command blocks; capabilities grow out of them |
| Hand over the keys | Sync = central custody | Keychain + your own repo, no server |
| Worship the static IP | Target = static address | Dynamic discovery; targets are identities, not addresses |
| Format land grab | Data belongs to the tool | known_hosts / asciicast / one shared SQLite |

rssh's entire design is these five assumptions flipped one by one: AI runs the errands inside the I/O path and you only approve; command objects are cut out of the byte stream; secrets go into your keychain and config into your repo; dynamic targets are discovered live; data formats are all open.

**One sentence**: a good SSH tool should let you forget it exists — not keep you waiting on it hand and foot every day.
