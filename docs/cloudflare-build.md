# Cloudflare build strategy

Cloudflare Workers Builds runs only the web build:

```bash
npm run build
```

The generated browser WebAssembly package under `web/public/pkg` is committed to the repository. GitHub Actions regenerates it with Rust and `wasm-pack` whenever the Rust core or its build configuration changes.

This keeps the Cloudflare build environment Node-only and avoids installing the Rust toolchain and `wasm-pack` during every deployment.

For local development after changing the Rust core, run:

```bash
npm run dev:full
```

For a full production build including WASM regeneration, run:

```bash
npm run build:full
```
