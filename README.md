# FoldSentinel

FoldSentinel is an open-source Windows folder protection project built with Rust.

The project is intended to provide two complementary protection modes:

- an encrypted vault for protecting file contents at rest;
- optional Windows file-system integration for locking, hiding, and read-only policies on existing folders.

The current repository is an initial Rust workspace. The security model, recovery behavior, and supported Windows versions will be documented before user data is handled by a release build.

## Development

```powershell
cargo run
cargo test
```

## Planned architecture

- Rust core for protection rules, state transitions, file operations, and crypto integration;
- Tauri desktop UI for the Windows application;
- a small Windows service for privileged operations when required;
- a separately reviewed Windows file-system minifilter for transparent folder policies.

## License

MIT. See [LICENSE](LICENSE).
