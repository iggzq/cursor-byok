<div align="center">

# cursor-byok
cursor-byok is a local implementation of Cursor's backend.
<br>
<br>

[Download](https://github.com/iggzq/cursor-byok/releases/latest) · [Report an Issue](https://github.com/iggzq/cursor-byok/issues) · [中文版本说明](./README-CN.md)

[![Release](https://img.shields.io/github/v/release/iggzq/cursor-byok?style=flat-square)](https://github.com/iggzq/cursor-byok/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/iggzq/cursor-byok/total?style=flat-square)](https://github.com/iggzq/cursor-byok/releases)
[![License](https://img.shields.io/github/license/iggzq/cursor-byok?style=flat-square)](./LICENSE)
[![Platforms](https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey?style=flat-square)](https://github.com/iggzq/cursor-byok/releases/latest)



</div>

![Connect cursor-byok to a wide range of model APIs](./images/en-brand-1.png)

![cursor-byok dashboard](./images/en-home-1.png)

## About

cursor-byok is an open-source local model gateway for Cursor. It runs a service on your machine that connects Cursor to the model APIs you configure, routes model requests through your own providers, and preserves Cursor Agent capabilities such as tool calling, Skills, and MCP.

You can connect OpenAI- and Anthropic-compatible services, customize endpoints, model IDs, API keys, and request parameters, and use model channels beyond the options built into the platform.

> [!IMPORTANT]
> cursor-byok is free and open source, but the model APIs you connect may charge for usage. This is an independent project and is not affiliated with or endorsed by Cursor or its developers.

## Features

- **Bring your own model channels:** Configure your own API endpoint, credentials, and model IDs.
- **Multiple API protocols:** Use OpenAI- and Anthropic-compatible APIs or a custom endpoint.
- **Model management:** Add, duplicate, edit, reorder, and batch-test multiple model configurations.
- **Connection benchmarks:** Measure time to first token, generation speed, and inspect raw provider responses.
- **Agent workflows:** Keep tool calling, Skills, MCP, and multi-turn conversations available.
- **Session metrics:** Track token usage, cache hit rate, conversation turns, and estimated value.
- **Cross-platform:** Run on macOS, Windows, and Linux.

## Quick Start

1. Download the latest build for your platform from [GitHub Releases](https://github.com/iggzq/cursor-byok/releases/latest).
2. Launch cursor-byok, open **Model Settings**, and enter the endpoint, API key, and model ID.
3. Test the model configuration. Once it passes, return to the dashboard and start the service.
4. After upgrading Cursor or configuring a model for the first time, quit Cursor completely and restart it, then start a new conversation and select the configured model.

## Model Management

Model configurations support both OpenAI and Anthropic API protocols. Each model channel can independently define its context window, maximum output tokens, reasoning effort, custom headers, and additional request parameters.

![cursor-byok model settings](./images/en-model-1.png)

## How It Works

```text
Cursor client
    │
    │ Agent requests and tool results
    ▼
cursor-byok local service
    │
    │ OpenAI- / Anthropic-compatible requests
    ▼
Your model API
```

cursor-byok handles protocol adaptation, model request forwarding, tool-call coordination, and conversation state on your machine. API keys and application settings are stored locally; requests are still sent to the model provider you configure.

## Cross-Machine Overview Aggregation

Each desktop keeps its usage statistics locally. To merge data from several machines, deploy the standalone collector service on a central server; it is a separate binary that shares no runtime with the desktop app.

```bash
# Build and run on the central host
make build-collector
CURSOR_OVERVIEW_COLLECTOR_API_KEY=change-me ./target/release/cursor-overview-collector
```

To run it on a different architecture (for example an x86_64 Linux server while working on Apple Silicon), package a Linux binary with Docker and copy it over:

```bash
make package-collector                      # cross-builds linux/amd64 into dist/collector/
scp dist/collector/cursor-overview-collector dist/collector/install-collector.sh root@SERVER:/root/
ssh root@SERVER 'sh /root/install-collector.sh /root/cursor-overview-collector "change-me"'
```

`install-collector.sh` creates the `collector` system user, installs the binary to `/opt/cursor-overview-collector`, stores the database in `/var/lib/cursor-collector`, writes and enables a hardened systemd unit, then prints a `/healthz` check. Manage it with `systemctl status|restart cursor-overview-collector` and `journalctl -u cursor-overview-collector -f`.

Or with Docker:

```bash
CURSOR_OVERVIEW_COLLECTOR_API_KEY=change-me docker compose -f docker-compose.collector.yml up -d
```

The collector listens on `0.0.0.0:4399` by default and exposes only:

- `POST /byok/v1/overview/ingest` — members push finished call records
- `GET /byok/v1/overview`, `/models`, `/sources` — read-only aggregates
- `GET /healthz` — health check

Then, in each desktop, open **Settings → 概览数据**, switch to **远程汇总模式**, and fill in the collector address (`http://<server>:4399`), the same API key, and a device name. Every machine, including the central host itself, pushes incrementally; only usage counts and call metadata are transferred, never prompts or responses.

Configuration:

| Variable | Meaning |
| --- | --- |
| `CURSOR_OVERVIEW_COLLECTOR_API_KEY` | Required shared key for members |
| `CURSOR_OVERVIEW_COLLECTOR_ADDR` | Listen address, default `0.0.0.0:4399` |
| `CURSOR_DATABASE_URL` | SQLite URL, default `sqlite://overview-collector.db` |

## Why This Project

Many Agent products bundle their tool capabilities with a fixed set of models, subscriptions, and billing options, leaving users limited to the channels offered by the platform.

cursor-byok is built to return model choice to the user. Developers can make full use of the APIs and credits they already have, choose the models and providers that fit their needs, and self-host related services when required.

## Roadmap

The project will continue to improve model compatibility, Agent tooling, local runtime stability, and the self-hosting experience while exploring support for more IDE, chat, and Agent workflows.

See the [open issues](https://github.com/iggzq/cursor-byok/issues) for plans and progress.

## Community and Support

- [GitHub Issues](https://github.com/iggzq/cursor-byok/issues)

## Development and Contributing

Issues and pull requests are welcome. See the [Contributing Guide](./CONTRIBUTING_EN.md) for prerequisites, build commands, project structure, and contribution guidelines.

## Contributors

<a href="https://github.com/iggzq/cursor-byok/graphs/contributors">
  <img src="https://contrib.rocks/image?repo=iggzq/cursor-byok" />
</a>

## License

This project is open source under the [MIT License](./LICENSE).
