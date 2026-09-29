# Third-party material

The MIT license in [LICENSE](LICENSE) covers Sinopia's source code and the
art made for it. The binary also embeds material that belongs to others,
and that license does not cover it:

| What | Where | Terms |
| --- | --- | --- |
| Liberation Sans, the chrome's fallback face and the board's default text face, in its four styles | `assets/fonts/LiberationSans-{Regular,Bold,Italic,BoldItalic}.ttf` | SIL Open Font License 1.1, with Reserved Font Name Liberation — [its license](assets/fonts/LiberationSans-LICENSE.txt) travels with every package. |
| Each agent's mark in the export dialog | `assets/agents/` | Trademarks of Anthropic, OpenAI, Anomaly, Charm and Google, used unaltered and only to name the product each belongs to. Where each one came from is in [assets/agents/README.md](assets/agents/README.md). |

The Rust crates the binary is built from are each under a licence of
their own — MIT, Apache 2.0, BSD, ISC, zlib, Unicode and the like, nothing
copyleft — and [THIRD-PARTY-CRATES.md](THIRD-PARTY-CRATES.md) lists every
one with its licence's text. It travels with every package too.

Sketchbook's brush sets are not among them. They are Sketchbook's,
published for use with its own app, so Sinopia ships none of them:
`sinopia brushes import` reads a copy the person downloaded into their
own data directory.
