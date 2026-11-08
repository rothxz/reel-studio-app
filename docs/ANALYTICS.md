# Usage data

Reel Studio collects a small amount of **anonymous** usage data, such as
which features are used and whether exports succeed. It tells us what to
improve and what's broken. It never includes your videos, your projects or
anything you type, and you can turn it off at any time.

## How to turn it off

Open **Settings** (⌘, on macOS, or the gear on the home screen), go to
**Privacy**, and switch off **Share anonymous usage data**.

It stops right away: nothing more is sent, and anything not yet sent is
discarded. Switch it back on whenever you like.

## What we collect

**What you do in the app, counted, not recorded.** For example:

- the app was opened; a project was created, opened or deleted
- a video, photo or audio file was imported (its type, size and length,
  not its name or content)
- a clip, text, sticker or audio was added or removed
- an effect, filter or transition was used (its name from our catalog,
  e.g. "Glitch")
- clips were moved, trimmed or split; undo and redo
- an export started, finished, failed or was cancelled (length,
  resolution, frame rate, how long it took, file size)
- at the end of an editing session: how long it lasted and how many edits
  were made

**About your computer:**

- operating system and version (e.g. macOS 15.4)
- processor type (e.g. Apple Silicon)
- Reel Studio version

**An anonymous id:** a random code created the first time the app runs
and stored on your computer. It lets us tell "one person used it twice"
apart from "two people used it once". It isn't linked to your name, email,
account or device serial number.

## What we never collect

- your videos, photos, audio or what's in them
- file names or folder paths
- project names or project contents
- text you type: titles, captions, prompts to the AI agent
- API keys, passwords, your email or your name
- your location (location lookup from your IP address is switched off)
- error messages (they can contain file paths), only that something failed

## Where it goes

Data is sent to [PostHog](https://posthog.com), the product analytics
service we also use for the Reel Studio phone app. It's sent in small
batches every 30 seconds or so while the app is open. If you're offline,
up to 200 events are held and sent later; nothing is kept beyond that.

We use it only to understand how Reel Studio is used and to improve it. We
don't sell it and don't use it for advertising.

## Starting over

To get a new anonymous id, quit Reel Studio and delete the file
`analytics-id` from the app's data folder:

| System | Folder |
| --- | --- |
| macOS | `~/Library/Application Support/Reel Studio` |
| Windows | `%APPDATA%\Reel Studio` |
| Linux | `~/.local/share/reel-studio` |

## Builds you make yourself

If you build Reel Studio from this repository, it doesn't send anything.
Only the official releases on the
[Releases page](https://github.com/rothxz/reel-studio-app/releases) send
usage data.
