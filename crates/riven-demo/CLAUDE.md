# crates/riven-demo

The launcher's main screens on made-up data, compiled to `wasm32-unknown-unknown` and shown on bxteam.org in place of screenshots. Depends on gpui, GPUI Base and moonglass only: no other `riven-*` crate builds for the browser, so it repeats the layout of `src/gui` instead of sharing it. Screens without data show a placeholder with a download button.

## Build

```bash
cargo run -p riven-demo              # the same window natively, for working on it
crates/riven-demo/build-web.sh       # dist/: index.html, riven_demo.js, riven_demo_bg.wasm
```

`build-web.sh` needs a nightly toolchain with the `wasm32-unknown-unknown` target and `wasm-bindgen-cli` at the exact version in `Cargo.lock`; `wasm-opt` is used when present.

## Publish

The site serves the demo from `apps/web/public/demo/riven/` in [BX-Team/code](https://github.com/BX-Team/code) and writes its own page around it: after a change, copy `dist/riven_demo.js` and `dist/riven_demo_bg.wasm` there. `web/index.html` is only for trying the build locally.

## Browser gotchas

- `std::time::Instant::now()` panics on `wasm32-unknown-unknown`; don't call it, nor moonglass APIs that do (theme fades).
- The browser has no fonts gpui can read; the bundled Inter is the theme font.
- gpui puts its canvas on `<body>`, so the demo is always its own page (an iframe on the site).
