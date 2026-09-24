# Agent logos

The marks the export dialog shows beside each agent it lists, as each
vendor ships them. `tools/agent-logos.sh` sheets them into `logos.png` —
one 40 px cell per agent, in `agents::KNOWN`'s order — and that sheet is
all the binary embeds.

| file | mark | where it came from |
| --- | --- | --- |
| `claude.svg` | the Claude spark, in Clay `#D97757` | Anthropic's press kit, <https://anthropic.com/press-kit>: `Claude logos/3 Claude Spark/SVG/Claude Spark - Clay.svg`. The kit's Claude Code logo is this spark beside a wordmark. |
| `codex.webp` | the Codex app icon | OpenAI's developer site, <https://developers.openai.com/images/codex/icons/codex-app-ga-logo.webp> |
| `opencode.svg` | the opencode favicon | <https://opencode.ai/favicon.svg> |
| `crush.png` | the icon Crush puts on its own notifications | [charmbracelet/crush](https://github.com/charmbracelet/crush) at `f8da538`, `internal/ui/notification/crush-icon-solo.png`. Crush's logo proper is a wordmark. |
| `gemini.png` | the Gemini CLI icon | <https://geminicli.com/icon.png>, byte for byte the `packages/vscode-ide-companion/assets/icon.png` of [google-gemini/gemini-cli](https://github.com/google-gemini/gemini-cli) |

Each one reads on a light panel and a dark one alike, which is why these
and not the OpenAI blossom or opencode's square wordmark: both of those
ship as a black and a white file, and one of the two disappears on
whichever ground the desktop's theme is not.

Every mark is its owner's trademark — Anthropic, OpenAI, Anomaly,
Charm, Google — used unaltered and only to name the product it belongs
to.
