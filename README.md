```
 ____  ____  _   _                _     _
/ ___|/ ___|| | | | ___ _ __ __ _| | __| |
\___ \\___ \| |_| |/ _ \ '__/ _` | |/ _` |
 ___) |___) |  _  |  __/ | | (_| | | (_| |
|____/|____/|_| |_|\___|_|  \__,_|_|\__,_|
```

=
                            ABOUT
=

  Multi-session SSH/SFTP client written in Rust.
  Terminal, file manager, port forwarding -- all in one window.
  Pure Rust SSH stack (russh + ring). No OpenSSL, no C deps.
  Single binary, zero installation.

=
                            BUILD
=

  Install dependencies (Debian/Ubuntu/Fedora/Arch):

    $ ./build-release.sh deps

  This will install system libraries (libxcb, gtk3, wayland, etc.),
  Rust toolchain (via rustup) and MinGW-w64 for cross-compilation.

  Build:

    $ ./build-release.sh current       # current platform
    $ ./build-release.sh linux         # Linux x86_64
    $ ./build-release.sh windows       # Windows x86_64 (cross)
    $ ./build-release.sh all           # all of the above

  Binaries go into dist/:

    dist/ssherald-0.1.0-x86_64-unknown-linux-gnu       ~10 MB
    dist/ssherald-0.1.0-x86_64-pc-windows-gnu.exe      ~5.5 MB

  Or just use cargo directly:

    $ cargo build --release

=
                           CONFIG
=

  Session profiles are stored in:

    Linux:    ~/.config/ssherald/sessions.json
    Windows:  %APPDATA%\ssherald\sessions.json
    macOS:    ~/Library/Application Support/ssherald/sessions.json

  Created automatically on first run.
  Passwords are NEVER saved to disk -- prompted on every connect.

===================================================================
