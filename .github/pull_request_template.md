<!-- Thank you for contributing to foto! -->

## Summary

What does this PR change, and why? Link the issue it fixes
(`fixes #123`) if there is one.

## What kind of change is this?

- [ ] Bug fix (non-breaking)
- [ ] New feature (non-breaking)
- [ ] Refactor (behavior-preserving)
- [ ] Docs / CI / packaging only

## Checklist

- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --all-targets -- -D warnings` passes
- [ ] `cargo test` passes
- [ ] **Manual TUI check**: `cargo run -- --tui` still works
      (launch, filter, arrows, Enter, Tab, Esc)
- [ ] **Manual GUI check** on at least one layer-shell compositor
      (niri / Hyprland / Sway / mango): the overlay floats, keyboard
      is grabbed, rendering is correct
- [ ] New config keys (if any) are: documented in the README, present in
      `config.example.kdl`, and rejected-when-unknown behavior is kept
- [ ] No new heavyweight dependency was added (GTK/Qt/webview are
      forbidden; anything larger than a leaf utility crate needs prior
      approval and a written rationale)

## Notes for the reviewer

Anything surprising, deliberate trade-offs, or follow-up work.
