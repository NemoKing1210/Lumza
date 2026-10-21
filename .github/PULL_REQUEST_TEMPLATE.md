## What and why

<!-- A short description. Link the issue if there is one. -->

## Checklist

- [ ] Title follows [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `docs:` …)
- [ ] `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass
- [ ] In `ui/`: `npm run lint`, `npm run typecheck` and `npm test` pass
- [ ] New user-visible strings are in **both** `locales/ru.json` and `locales/en.json`
- [ ] Anything that writes to monitors goes through the controller (coalescing, throttling), never straight from the UI
- [ ] Tested on real hardware (monitor model and connection: …) or on the mock backend (`LUMZA_MOCK=1`)
