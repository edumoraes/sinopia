# Omawhite

Whiteboard local-first para Omarchy, com export para o agente. Motor nativo em
Rust (winit + wgpu). Ver [ARCHITECTURE.md](ARCHITECTURE.md) — **rascunho**: a
arquitetura de verdade emerge do desenvolvimento.

## Estado

Scaffolding (§15 do rascunho, itens 1–2 esboçados):

- `cargo build` limpo, `cargo test` com 46 testes.
- Janela Wayland + wgpu renderizando retângulos do documento.
- Documento JSON versionado (schema 1) + persistência XDG (0700/0600, save atômico).
- Protocolo IPC §5 (schema fechado) + single-instance por socket.
- CLI: `--new`, `--open <id>`, `--export <dir>`, `--shutdown`, `--socket <path>`.

Ainda não existe: ferramentas de desenho/input, undo, export, plugin Omarchy,
thumbs, tema via socket aplicado de ponta a ponta.

## Rodar

```sh
cargo run                      # board mais recente (ou um novo)
cargo run -- --new             # board novo
cargo test                     # suíte completa
```

Smoke test (abre janela, renderiza 3 frames, sai):

```sh
XDG_DATA_HOME=/tmp/omawhite-smoke cargo run -- \
  --socket /tmp/omawhite-smoke.sock --smoke-frames 3
```

## Layout

```
src/main.rs      dispatch CLI → forward ou instância principal
src/cli.rs       flags (clap), ações mutuamente exclusivas
src/doc.rs       documento §6.1 (dado puro, serde)
src/store.rs     ~/.local/share/omawhite: boards/, index.json, perms §9.3
src/ipc/         §5: proto (parser estrito), client (forward), server (socket 0600)
src/scene.rs     câmera e documento → instâncias de retângulo (puro, testado)
src/gfx.rs       wgpu 30: pipeline de quads instanciados
src/app.rs       winit: janela, eventos, ponte socket → event loop
```

Dados do usuário: `~/.local/share/omawhite/`. Socket:
`$XDG_RUNTIME_DIR/omawhite.sock`.
