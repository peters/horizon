# First steps

This guide tells you how to install Horizon and use it for the first time. It
describes Horizon as it is on `main`. Some functions work on Linux only. Read
[the platform support](platform-support.md) before you start.

## What Horizon does

Horizon shows all of your terminals, coding agents, browsers and remote
desktops on one board. The board is a large surface that you can pan and zoom.
A workspace is a group of panels with one working directory. Horizon saves the
board when you close it.

These functions are available:

| Function | What it does | Platforms |
|---|---|---|
| Shell panels | A terminal in the workspace directory. | All, with limits on Windows |
| Agent panels | Claude Code, Codex, Grok and other coding agents in a panel. | Linux and macOS. Not tested on Windows |
| Browser panels | Chromium, Firefox or Safari on the board. You and an agent use the same page. | All. Safari on macOS only |
| Device panels | A VNC view of a desktop on this computer or on an SSH host. | All |
| Agent input to an isolated desktop | An agent clicks and types on an Xvfb desktop for an application test. | Linux with X11 |
| Remote Hosts | SSH panels for your hosts and Tailscale devices. | Linux and macOS. Not tested on Windows |
| Clouds | A remote worker with its own shell, agent, browser and Device panels. | Linux and macOS |
| Casting | A panel, a workspace or the window on an Apple TV. | Linux |
| Speech | Dictation into terminals, editors and browser pages. | Source build only |

## Select an install route

| Route | Use it when | Speech |
|---|---|---|
| Release binary | You want to start quickly. | No |
| Surge installer | You want the in-app update prompt. | No |
| Homebrew (macOS, Linux x64) | You use Homebrew. | No |
| WinGet (Windows) | You use WinGet. | No |
| Source build | You want speech or GPU encoder features. | Yes, with a feature |

Release builds use the default features. To get speech, use a source build.

## Install a release binary

1. Open the [latest release](https://github.com/peters/horizon/releases/latest).
2. Download the file for your platform:

   | Platform | File |
   |---|---|
   | Linux x64 | `horizon-linux-x64.tar.gz` or `horizon-installer-linux-x64.bin` |
   | macOS arm64 | `horizon-osx-arm64.tar.gz` or `horizon-installer-osx-arm64.bin` |
   | macOS x64 | `horizon-osx-x64.tar.gz` or `horizon-installer-osx-x64.bin` |
   | Windows x64 | `horizon-windows-x64.exe` or `horizon-installer-win-x64.exe` |

3. If you downloaded a `.tar.gz` file, extract it and make `horizon` executable.
4. Start `horizon`.

   Result: Horizon opens an empty board.

The release does not sign or notarize the macOS application.

## Install with a package manager

On macOS or Linux x64, use Homebrew:

```bash
brew install peters/horizon/horizon
```

On Windows, use WinGet:

```powershell
winget install Peters.Horizon
```

Use the same package manager to update Horizon. The in-app update prompt is
for Surge installs only.

## Build from source

1. Install the tools for your platform:
   - All platforms: Git, Git LFS and Rust 1.95 or later from [rustup](https://rustup.rs).
   - Linux: the system headers in [AGENTS.md](../AGENTS.md#prerequisites).
   - macOS: the Xcode Command Line Tools (`xcode-select --install`).
   - Windows: the MSVC build tools. `rustup` installs them for the `msvc` target.
   - x86_64: NASM 2.15 or later on `PATH`.
2. Get the source:

   ```bash
   git clone https://github.com/peters/horizon.git
   cd horizon
   git lfs install
   git lfs pull
   ```

3. Build and start Horizon:

   ```bash
   cargo run --release
   ```

   Result: Horizon opens an empty board.

If the build stops with a missing font, run `git lfs pull` again. On Linux, a
`pkg-config` or linker error usually tells you that a `-dev` package is missing.

### Speech and GPU features

Speech needs CMake and a C++ compiler. Linux also needs the ALSA headers.
Select one command:

| Command | Speech backend | Build needs |
|---|---|---|
| `cargo speech` | CPU. Metal on macOS. | CMake, C++ |
| `cargo speech-cuda` | NVIDIA GPU | CUDA toolkit |
| `cargo speech-vulkan` | Any GPU with Vulkan | Vulkan SDK |

The build does not find a GPU toolkit automatically. Select the command
yourself. For casting with NVENC on Linux, add `--features cast-nvenc`.

## Use the board

1. Hold Ctrl and double-click an empty area of the board.

   Result: A list of presets opens.

2. Select **Shell**.
3. Select a working directory.

   Result: Horizon makes a workspace with one shell panel in that directory.

4. Push Ctrl+Shift+N.

   Result: A second panel of the first preset opens.

5. Push Ctrl+Shift+K and type a workspace name, a panel title or `>`.

   Result: The command palette shows the matches.

6. On the workspace header, click **Rows**, **Cols** or **Grid**.

   Result: The panels move into that layout.

7. Close Horizon and start it again.

   Result: The board, the layout and the terminal history are the same.

The README lists all keyboard and mouse shortcuts.

## Open an agent, a browser and a desktop

- **Agent panel:** Install the agent CLI first, for example Claude Code or Codex.
  Then select its preset. On Windows, an agent panel needs a POSIX shell in
  `SHELL`, for example Git Bash. No test examines this.
- **Browser panel:** Select the **Browser** preset. Horizon gives the
  `horizon-browser` MCP tools automatically to Claude Code, Codex and Grok panels.
- **Device panel:** Add a VNC target to the configuration. See
  [Watch an app over VNC](../README.md#watch-an-app-over-vnc).

## Start a first cloud

Clouds work on Linux and macOS. Read [Cloud workspaces](cloud-workspaces.md)
for the full setup.

> **CAUTION:** STOP OR DELETE EACH CLOUD THAT YOU DO NOT USE. A running worker
> and a stopped volume continue to cost money.

> **CAUTION:** DO NOT PUT AN API KEY IN A LOG, AN ISSUE OR A REPOSITORY. Horizon
> keeps keys in private files and in the system credential store.

A first cloud needs these items today:

| Item | Need | Where |
|---|---|---|
| RunPod API key or Hetzner API token | Required | **Cloud > Cloud settings** |
| Worker image in a registry | Required. There is no public default image. | Build it from [`examples/cloud-worker`](../examples/cloud-worker/README.md) |
| Git, OpenSSH, Docker with buildx | Required | This computer |
| Registry push and pull logins | Required for a private image | **Cloud settings > Container registry** |
| Agent API key or subscription login | One for each agent | **Cloud settings** |
| Tailscale auth key | Optional | **Settings > Tailnets** |
| Git push credential for the worker | Optional | `~/.horizon/cloud/settings.json` |

Horizon makes the SSH identity for the worker. **Cloud settings** does not
test a key when you save it. **New cloud** uses the key to get the live worker
catalog. If it cannot get the catalog, you cannot start the cloud.

1. Open a workspace in a Git repository.
2. Click **Cloud**, then **New cloud…**.

   Result: The **Where is your code?** step opens.

3. Paste a repository link, or select a local folder.
4. Select a worker in the catalog.

   Result: The summary shows the compute cost for each hour and the total for each month.

5. Click **Start**.

   Result: The cloud card shows each stage with its time.

Deploy sends committed source only. Horizon does not fetch your local clone
from `origin` before deploy. Commit and update the branch first.

## Get help

- [Platform support](platform-support.md)
- [Cloud workspaces](cloud-workspaces.md)
- [Casting](casting.md)
- [Local Network Bridge](local-network-bridge.md)
- [README](../README.md)
