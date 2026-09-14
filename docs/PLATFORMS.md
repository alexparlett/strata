# Platforms

Strata is built for macOS and shipped there. This document is the other platforms' honest status:
what compiles, what runs, and what is still macOS-shaped — so that "does it work on Linux" has an
answer in the repository rather than in someone's memory.

The short version: **the workspace builds, its whole test suite passes, and the app runs on Linux**,
with window chrome and a menubar of its own. CI's `linux` job holds the first two. The updater and
packaging are macOS-only by construction, and there is one NVIDIA driver bug to steer around.

## What the platform axis actually is

Every platform difference in this workspace is a `cfg`, and there are not many of them:

| What | Where | macOS | Linux / other Unix | Windows |
|---|---|---|---|---|
| Secret store | `strata_core::secret::open_keystore` | Keychain (login) | Secret Service over D-Bus | Credential Manager |
| Themes directory | `strata_core::theme::user_themes_dir` | `~/Library/Application Support/Strata/themes` | `~/.config/Strata/themes` | same as Linux |
| Log directory | `strata_freya::log_dir` | `~/Library/Logs/Strata` | `~/.local/state/Strata/logs` | same as Linux |
| Reveal a folder | `theme::open_user_themes_dir`, launcher `reveal` | `open` | `xdg-open` | `explorer` |
| Open a URL | `strata_core::update::open_page` | `/usr/bin/open` | `xdg-open` | `explorer` |
| OS dark mode | `strata_core::theme::os_is_dark` | `defaults read -g AppleInterfaceStyle` | the freedesktop appearance portal, over `gdbus` | dark |
| Chord hints | `strata_core::keymap::chord_caps` | `⇧ ⌥ ⌘`, run together | `Shift Alt Ctrl`, joined with `+` | same as Linux |
| Window decoration | `strata_freya::platform::window_attributes` | transparent titlebar, full-size content | **removed** — the chrome is ours | as Linux |
| Window buttons | `components::chrome::WindowControls` | AppKit's, inset into our strip | ours, trailing edge | ours |
| Resize borders | `BorderlessPlugin`, registered in `main` | AppKit's | the plugin's bands | the plugin's bands |
| Menubar | `menu::app_menu` / `components::menu_bar` | muda, installed into `NSApp` | ours, drawn in the title bar | ours |

A `cfg` arm that no CI job compiles is an arm that is only ever read, never checked. That is what
the `linux` job exists for, and why it runs `clippy --all-targets` (which compiles the non-macOS
arms and the tests beside them) rather than only `cargo build`.

## Linux: building and running

```bash
sudo apt-get install build-essential pkg-config libssl-dev libglib2.0-dev libgtk-3-dev libxdo-dev
cargo run
```

The list is what the build **links**, derived from `ldd` on a built `strata-freya`:

- `libgtk-3-dev` and `libxdo-dev` — muda's Linux backend, pulled in by the `menu` feature that
  `strata-freya` asks Freya for. (The menu itself is never installed off macOS — see below — but the
  crate is still compiled and linked.)
- `libssl-dev` — `openssl-sys`, under the `native-tls` that the Postgres and MySQL table providers
  use. The app's own HTTP (`reqwest` in `strata-core`) is rustls and needs nothing.
- `libglib2.0-dev` — named rather than left to gtk's own dependencies because it is the first thing
  every `gtk-sys` probe looks for.

Skia's fontconfig headers arrive with `libgtk-3-dev` (→ `libcairo2-dev` → `libfontconfig-dev`), so
they need no entry of their own.

### NVIDIA + Wayland: use the OpenGL renderer

```bash
FREYA_RENDERER=opengl cargo run
```

Freya picks **Vulkan** by default on Linux, and on an NVIDIA GPU under Wayland that segfaults —
repeatedly, within a minute or two of use. Every core dump is the same and none of them has a Strata
frame in it:

```
#0  0x0000000000000000
#1  libnvidia-glcore.so.615.71.09
#3  ash … acquire_next_image      (vkAcquireNextImageKHR)
#4  freya_winit::drivers::vulkan::present
```

The driver jumps to a null pointer *inside* `vkAcquireNextImageKHR`, before returning any result
code, so no error handling on our side or Freya's can catch it — `present` already handles
`ERROR_OUT_OF_DATE_KHR` and `ERROR_SURFACE_LOST_KHR` correctly. It is a driver bug to steer around,
not one to fix. `FREYA_RENDERER=opengl` avoids the Vulkan path entirely and is stable; running under
XWayland (`WAYLAND_DISPLAY= …`) is the other lever, since NVIDIA's X11 Vulkan path is far healthier.

### File dialogs need a working portal

Open Project, Export and the source-path pickers go through `rfd`'s XDG portal backend, so the
desktop needs an `xdg-desktop-portal` implementation that actually serves
`org.freedesktop.impl.portal.FileChooser`. A portal that registers the interface and exposes no
object answers the request and then never replies, which reads in the app as a button that does
nothing — and rfd's zenity fallback does not help, because the call *succeeded*. Check what is
handling it with `busctl --user introspect org.freedesktop.impl.portal.desktop.<impl>
/org/freedesktop/portal/desktop org.freedesktop.impl.portal.FileChooser`, and point
`~/.config/xdg-desktop-portal/portals.conf` at `gtk` if the configured one is not answering.

## Linux: the window chrome

There is no half-decorated window off macOS — a WM either draws a frame or it does not — so
`window_attributes` takes the decoration off outright and the app draws the whole title bar. Three
pieces make that whole:

- **`components::chrome::WindowControls`** — minimize, maximize/restore and close, at the *trailing*
  edge, which is where Windows and every mainstream Linux desktop put them. They are the fork's
  `TitlebarButton`, themed; close routes through `platform::request_close`, which raises the same
  `CloseRequested` the OS button does, so a window holding itself open (a running query, an unsaved
  form) still gets its say.
- **`BorderlessPlugin`**, registered at launch — invisible bands along the window edges that drive a
  native `drag_resize_window`, putting back the resize borders the decoration took with it.
- **`components::menu_bar::MenuBar`** — the App / File / Edit / Window tree behind one trigger, since
  there is nowhere to install a native menubar (below).

`TRAFFIC_LIGHT_GUTTER` and `WINDOW_CONTROLS_GUTTER` are the two reserves a title bar keeps, and
exactly one of them is non-zero on any platform.

### Why the menubar is drawn rather than installed

muda can attach a menubar to a GTK window (`init_for_gtk_window`) or a Win32 one (`init_for_hwnd`).
Freya's windows are winit's, which are neither, so there is no handle to hand it — the fork installs
the menu only via `init_for_nsapp`. This is a real architectural limit, not a gap to fix in the fork,
and drawing the menu is what VS Code and Zed do for the same reason.

`menu::MENUS` is the structure of record and both surfaces read it, so an item cannot exist on one
platform only; `every_command_is_in_exactly_one_menu` fails the build if one does. `Placement` is
the single thing the two presentations differ on: macOS leads with the App menu because that is where
the platform puts it, while the in-app dropdown puts Settings… and Quit at its foot under a rule, the
way Vivaldi, Firefox and Chrome do.

The menubar is only in the launcher and the project header — the two *workspace* windows. Settings,
Configure, Export and the source editor are panels whose `Gate` allows nothing, so a menu there would
be a list of greyed rows; they still get window controls.

## Linux: what is still rough

- **Minimize is a one-way trip on a tiling compositor.** `set_minimized(true)` on Hyprland (and
  anything else with no minimize concept) unmaps the window with no way to bring it back. Correct on
  GNOME and KDE; a trap elsewhere.
- **The fill button reads "restore" permanently** on a tiling compositor, because `is_maximized()` is
  true for a tiled window.

## Linux: what is macOS-only by construction

The **in-app updater** (`strata_core::update`) swaps one `.app` bundle for another using `ditto`,
`codesign` and `PlistBuddy`. It is inert rather than broken elsewhere: `site()` looks for the first
`.app` above the executable, finds none, answers `Site::Unbundled`, and every updater surface
degrades to a link to the release page. That link is the one part that has to work off macOS, so
`open_page` resolves the desktop's opener per OS. The two tests that drive `ditto` for real are gated
to macOS.

**Packaging** is `scripts/bundle-macos.sh` and the Release workflow around it — a `.app`, a DMG and
the `.app.zip` the updater installs from (see [RELEASING.md](RELEASING.md)). There is no Linux
artifact, and adding one is a separate piece of work: an AppImage or a `.deb` would also need an
update mechanism that is not a bundle swap, or none at all.
