# First steps

This guide tells you how to install Horizon and use it for the first time. It
describes Horizon as it is on `main`. Some functions work on Linux only. Read
[the platform support](platform-support.md) before you start.

The latest release is v0.2.7 from 2 August 2026. The `main` branch is more
than 800 commits newer. The release does not have browser panels, Device
panels, clouds, tailnets or casting. Some steps in this guide can also be
different in the release. To do all procedures in this guide, build from source.

## What Horizon does

Horizon shows all of your terminals, coding agents, browsers and remote
desktops on one board. The board is a large surface that you can pan and zoom.
A workspace is a group of panels with one working directory. Horizon saves the
board when you close it.

These functions are available:

| Function | What it does | Platforms | In v0.2.7 |
|---|---|---|---|
| Shell panels | A terminal in the workspace directory. | All, with limits on Windows | Yes |
| Agent panels | Claude Code, Codex, Grok and other coding agents in a panel. | Linux and macOS. Not tested on Windows | Yes |
| Browser panels | Chromium, Firefox or Safari on the board. You and an agent use the same page. | All. Safari on macOS only | No |
| Device panels | A VNC view of a desktop on this computer or on an SSH host. | All | No |
| Agent input to an isolated desktop | An agent clicks and types on an Xvfb desktop for an application test. | Linux with X11 | No |
| Remote Hosts | SSH panels for your hosts and Tailscale devices. | Linux and macOS. Not tested on Windows | Yes |
| Clouds | A remote worker with its own shell, agent, browser and Device panels. | Linux and macOS | No |
| Casting | A panel, a workspace or the window on an Apple TV. | Linux | No |
| Speech | Dictation into terminals, editors and browser pages. | Source build only | Source build only |

## Select an install route

| Route | Use it when | Speech |
|---|---|---|
| Release binary | You want to start quickly, and v0.2.7 has the functions that you need. | No |
| Surge installer | You want the in-app update prompt. | No |
| Homebrew (macOS, Linux x64) | You use Homebrew. | No |
| WinGet (Windows) | You use WinGet. | No |
| Source build | You want the functions in this guide, speech or GPU encoder features. | Yes, with a feature |

Release builds use the default features. All release routes install v0.2.7.

## Install a release binary on Linux or macOS

1. Open the [latest release](https://github.com/peters/horizon/releases/latest).
2. Download the raw binary for your platform:

   | Platform | File |
   |---|---|
   | Linux x64 | `horizon-linux-x64.tar.gz` |
   | macOS arm64 | `horizon-osx-arm64.tar.gz` |
   | macOS x64 | `horizon-osx-x64.tar.gz` |

3. Download `SHA256SUMS.txt` from the same release to the same directory.

> **CAUTION:** DO NOT START A FILE WITH AN INCORRECT CHECKSUM. The checksum
> finds an incomplete or damaged download. It does not prove who made the
> file, because the release and `SHA256SUMS.txt` are not signed.

4. If you use Linux, examine the checksum:

   ```bash
   sha256sum -c --ignore-missing SHA256SUMS.txt
   ```

5. If you use macOS, examine the checksum:

   ```bash
   shasum -a 256 -c --ignore-missing SHA256SUMS.txt
   ```

   Result: The command shows `OK` for the file that you downloaded.

6. If the command does not show `OK`, delete the file.
7. Extract the file. Replace `<file>` with its name, for example
   `horizon-osx-arm64.tar.gz`:

   ```bash
   tar -xzf <file>
   ```

8. Make the binary executable:

   ```bash
   chmod +x horizon
   ```

9. Start Horizon:

   ```bash
   ./horizon
   ```

   Result: Horizon opens an empty board.

10. If macOS blocks the start, remove the quarantine attribute:

    ```bash
    xattr -d com.apple.quarantine horizon
    ```

11. If you did step 10, start Horizon again:

    ```bash
    ./horizon
    ```

    Result: Horizon opens an empty board.

A raw binary does not update itself. Download a new release to update it.

## Install a release binary on Windows

1. Open the [latest release](https://github.com/peters/horizon/releases/latest).
2. Download `horizon-windows-x64.exe`.
3. Download `SHA256SUMS.txt` from the same release.

> **CAUTION:** DO NOT START A FILE WITH AN INCORRECT CHECKSUM. The checksum
> finds an incomplete or damaged download. It does not prove who made the
> file, because the release and `SHA256SUMS.txt` are not signed.

4. In PowerShell, get the checksum of the file:

   ```powershell
   Get-FileHash horizon-windows-x64.exe
   ```

   Result: PowerShell shows the SHA-256 hash.

5. Compare the hash with the line for `horizon-windows-x64.exe` in
   `SHA256SUMS.txt`.

   Result: The two hashes are the same.

6. If the hashes are not the same, delete the file.
7. Open `horizon-windows-x64.exe`.

   Result: Horizon opens an empty board.

## Install with the Surge installer on Linux or macOS

The Surge installer gives the in-app update prompt.

1. Open the [latest release](https://github.com/peters/horizon/releases/latest).
2. Download the installer for your platform:

   | Platform | File |
   |---|---|
   | Linux x64 | `horizon-installer-linux-x64.bin` |
   | macOS arm64 | `horizon-installer-osx-arm64.bin` |
   | macOS x64 | `horizon-installer-osx-x64.bin` |

3. Download `SHA256SUMS.txt` from the same release to the same directory.

> **CAUTION:** DO NOT START A FILE WITH AN INCORRECT CHECKSUM. The checksum
> finds an incomplete or damaged download. It does not prove who made the
> file, because the release and `SHA256SUMS.txt` are not signed.

4. If you use Linux, examine the checksum:

   ```bash
   sha256sum -c --ignore-missing SHA256SUMS.txt
   ```

5. If you use macOS, examine the checksum:

   ```bash
   shasum -a 256 -c --ignore-missing SHA256SUMS.txt
   ```

   Result: The command shows `OK` for the installer.

6. If the command does not show `OK`, delete the installer.
7. Make the installer executable. Replace `<installer>` with its name:

   ```bash
   chmod +x <installer>
   ```

8. If you use macOS, remove the quarantine attribute from the installer:

   ```bash
   xattr -d com.apple.quarantine <installer>
   ```

9. Start the installer:

   ```bash
   ./<installer>
   ```

10. Follow the steps in the installer.
11. Start Horizon.

    Result: Horizon opens an empty board. Horizon shows an update prompt when a
    new stable release is available.

## Install with the Surge installer on Windows

1. Open the [latest release](https://github.com/peters/horizon/releases/latest).
2. Download `horizon-installer-win-x64.exe`.
3. Download `SHA256SUMS.txt` from the same release.
4. In PowerShell, get the checksum of the installer:

   ```powershell
   Get-FileHash horizon-installer-win-x64.exe
   ```

5. Compare the hash with the line for `horizon-installer-win-x64.exe` in
   `SHA256SUMS.txt`.

   Result: The two hashes are the same.

6. If the hashes are not the same, delete the installer.
7. Open `horizon-installer-win-x64.exe`.
8. Follow the steps in the installer.
9. Start Horizon.

   Result: Horizon opens an empty board.

## Install with a package manager

On macOS or Linux x64, install Horizon with Homebrew:

```bash
brew install peters/horizon/horizon
```

On Windows, install Horizon with WinGet:

```powershell
winget install Peters.Horizon
```

Use the same package manager to update Horizon. The in-app update prompt is
for Surge installs only.

## Build from source

1. Install the tools for your platform:
   - All platforms: Git, Git LFS and Rust 1.95 or later from [rustup](https://rustup.rs).
   - Linux: the system headers in [AGENTS.md](../AGENTS.md#prerequisites).
   - macOS: the Xcode Command Line Tools.
   - Windows: the MSVC build tools. `rustup` installs them for the `msvc` target.
   - x86_64: NASM 2.15 or later on `PATH`.
2. If you use macOS, install the Xcode Command Line Tools:

   ```bash
   xcode-select --install
   ```

3. Clone the repository:

   ```bash
   git clone https://github.com/peters/horizon.git
   ```

4. Go to the repository directory:

   ```bash
   cd horizon
   ```

5. Set up Git LFS:

   ```bash
   git lfs install
   ```

6. Get the Git LFS files:

   ```bash
   git lfs pull
   ```

7. Build and start Horizon:

   ```bash
   cargo run --release
   ```

   Result: Horizon opens an empty board.

If the build stops with a missing font, do step 6 again. On Linux, a
`pkg-config` or linker error usually tells you that a `-dev` package is missing.

### Speech and GPU features

Speech needs CMake and a C++ compiler. Linux also needs the ALSA headers. The
build does not find a GPU toolkit automatically.

To build with speech, type one of these commands:

```bash
cargo speech          # CPU. Metal on macOS.
cargo speech-cuda     # NVIDIA GPU. Needs the CUDA toolkit.
cargo speech-vulkan   # Any GPU with Vulkan. Needs the Vulkan SDK.
```

To build with the NVENC encoder for casting on Linux, type this command:

```bash
cargo run --release --features cast-nvenc
```

## Use the board

On macOS, use Cmd instead of Ctrl in each step of this procedure.

1. Hold Ctrl and double-click an empty area of the board.

   Result: A list of presets opens.

2. Select **Shell**.
3. Select a working directory.

   Result: Horizon makes a workspace with one shell panel in that directory.

4. Push Ctrl+Shift+N.

   Result: A second panel of the first preset opens.

5. Push Ctrl+Shift+K.

   Result: The command palette opens.

6. Type a workspace name, a panel title or `>`.

   Result: The command palette shows the matches.

7. On the workspace header, click **Rows**, **Cols** or **Grid**.

   Result: The panels move into that layout.

8. Close Horizon.
9. Start Horizon again.

   Result: The board, the layout and the terminal history are the same.

The README lists all keyboard and mouse shortcuts. A shortcut with Ctrl uses
Cmd on macOS.

## Open an agent panel

On Windows, an agent panel needs a POSIX shell in `SHELL`, for example Git
Bash. No test examines this.

1. Install the CLI of the agent, for example Claude Code or Codex.
2. Hold Ctrl and double-click an empty area of the board.
3. Select the preset of the agent.

   Result: The agent starts in a new panel.

## Open a browser panel

Browser panels need a source build from `main`. The **Browser** preset uses a
Chromium browser. A usual macOS installation has only Safari.

1. Install Google Chrome, Chromium or Microsoft Edge.
2. Hold Ctrl and double-click an empty area of the board.
3. Select **Browser**.

   Result: A browser panel opens.

Horizon gives the `horizon-browser` MCP tools automatically to Claude Code,
Codex and Grok panels.

## Open a Device panel

Device panels need a source build from `main`. A Device panel needs a VNC target in the configuration. See
[Watch an app over VNC](../README.md#watch-an-app-over-vnc).

## Start a first cloud

Clouds work on Linux and macOS, with a source build from `main`. Read
[Cloud workspaces](cloud-workspaces.md) for the full setup.

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
| `.horizon/cloud.yml` in the selected commit | Required, or local image-only settings in **More options** | The repository. **New cloud** can open a setup agent that writes it. See [Repository setup](cloud-workspaces.md#repository-setup-and-deployment) |
| Registry push and pull logins | Required for a private image | **Cloud settings > Container registry** |
| Agent API key or subscription login | One for each agent | **Cloud settings** |
| Tailscale auth key | Optional | **Settings > Tailnets** |
| Git push credential for the worker | Optional | `~/.horizon/cloud/settings.json` |

Horizon makes the SSH identity for the worker. **Cloud settings** does not
test a key when you save it. **New cloud** uses the key to get the live worker
catalog. If it cannot get the catalog, you cannot start the cloud.

Deploy sends committed source only. Horizon does not fetch your local clone
from `origin` before deploy. Commit and update the branch before you start.

1. Open a workspace in a Git repository.
2. Click **Cloud**.
3. Click **New cloud…**.

   Result: The **Where is your code?** step opens.

4. If the code is on a Git server, paste the repository link.
5. If the code is on this computer, click **Browse…**.
6. If you clicked **Browse…**, select the folder.
7. Select a worker in the catalog.

   Result: The summary shows the compute price for each hour and an estimated
   cost for the run. The default run time is one hour.

8. Click **Start cloud**.

   Result: The cloud card shows each stage with its time.

## Get help

- [Platform support](platform-support.md)
- [Cloud workspaces](cloud-workspaces.md)
- [Casting](casting.md)
- [Local Network Bridge](local-network-bridge.md)
- [README](../README.md)
