# Private Linux candidate

This is a local packaging workflow, not a public release or redistribution approval for bundled characters. Linux x86_64 on the build host is the verification target. Windows, macOS, older Linux system libraries, live services and full visual/motion acceptance remain separate.

Build with the pinned dependency graph:

```sh
CARGO_BUILD_JOBS=1 cargo build --offline --locked --release --bin waifu-studio
python tools/package_linux.py --repo . --binary /absolute/path/to/release/waifu-studio --output /absolute/path/to/new-private-package
python tools/package_linux.py --verify /absolute/path/to/new-private-package --archive /absolute/path/to/new-private-package.tar.gz
```

The builder refuses existing destinations. It copies the executable, runtime assets, author documentation and license/attribution files. It excludes profiles, credentials, caches and source checkouts; it rejects links, hidden entries and non-regular files in copied trees. The experimental generated motion library on external storage is outside these inputs. Preserve model metadata and keep the resulting candidate private until redistribution rights are established.

Launch `start.sh` from the candidate. No Cargo invocation is required. System libraries, graphics drivers and any interpreters named by optional integrations remain external requirements; `DEPENDENCIES.txt` records observed shared-library and ELF symbol requirements, not cross-distribution acceptance. Installed extensions and profile writes belong in the user's separate app data directory.

Characters, motions, illustrated backgrounds, room textures and built-in integration examples share the executable-adjacent `assets/` root. Release binaries do not implicitly use the original build checkout, even when packaged assets are absent. An explicit `WAIFU_STUDIO_ASSET_ROOT` overrides that root; source `run.sh` sets it to its repository assets. Debug binaries retain development fallback. `WAIFU_STUDIO_BUNDLE` still overrides only character seeding, and `WAIFU_MOTION_LIBRARY` still adds explicitly chosen external motion roots. The old local WebWaifu4 checkout is a development convenience and is not part of a relocated package.

The manifest records hashes, sizes and executable state for every packaged file except itself. Verification detects corruption, missing/extra files and links. It is a copy-integrity check, not a signature or authenticity guarantee. Keep the archive hash separately when transferring a candidate.

Verification should extract the archive to a new path with spaces, hide the development/home directories in a private namespace, disable networking and inherited credentials, and run from an unrelated directory with fresh HOME/XDG/profile paths. Mount the package read-only. Check native avatar loading, shipped motion and background/room assets, package-manifest integrity, headless installed-extension replay, restart and preservation of profile edits after a second move. Namespace mounts must never rename or hide the real host checkout from other processes. These fixtures do not replace interactive native, streaming/audio, long-session or platform acceptance.
