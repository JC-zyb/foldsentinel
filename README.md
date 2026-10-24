# FoldSentinel

FoldSentinel is an open-source Windows folder protection project built with Rust.

The first usable CLI provides reversible file-attribute protection:

- `protect`: recursively apply `hidden`, `readonly`, or `hidden-readonly` to a file or directory tree;
- `unprotect`: restore the attributes captured before protection;
- `status`: inspect the tracked protection records;
- `state-path`: print the state file location.

Protection records are stored in `%APPDATA%\\FoldSentinel\\state.json`. The state file keeps the original attributes so an unprotect operation can restore them. The current implementation is a reversible convenience layer, not a cryptographic boundary: Windows directory read-only attributes do not prevent every write path, and the project does not yet include a kernel minifilter or encrypted vault.

## Development

```powershell
cargo run -- state-path
cargo run -- protect C:\\Private\\Notes --mode hidden-readonly
cargo run -- status C:\\Private\\Notes
cargo run -- unprotect C:\\Private\\Notes
cargo test
```

## Planned architecture

- Rust core for protection rules, state transitions, file operations, and crypto integration;
- Tauri desktop UI for the Windows application;
- a small Windows service for privileged operations when required;
- a separately reviewed Windows file-system minifilter for transparent folder policies.

## License

MIT. See [LICENSE](LICENSE).
