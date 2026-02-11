<div align="center">

# 👻 Kiro Gateway

**Proxy gateway for Kiro API (Amazon Q Developer / AWS CodeWhisperer)**

Made with ❤️ by [@Jwadow](https://github.com/jwadow)

[![License: AGPL v3](https://img.shields.io/badge/License-AGPL%20v3-blue.svg)](https://www.gnu.org/licenses/agpl-3.0)
[![Python 3.10+](https://img.shields.io/badge/python-3.10+-blue.svg)](https://www.python.org/downloads/)
[![FastAPI](https://img.shields.io/badge/FastAPI-0.100+-green.svg)](https://fastapi.tiangolo.com/)
[![Sponsor](https://img.shields.io/badge/💖_Sponsor-Support_Development-ff69b4)](#-support-the-project)

*Use Claude models from Kiro with Claude Code, OpenCode, Codex app, Cursor, Cline, Roo Code, Kilo Code, Obsidian, OpenAI SDK, LangChain, Continue, and other OpenAI or Anthropic compatible tools.*

[Models](#-available-models) • [Features](#-features) • [Quick Start](#-quick-start) • [Configuration](#%EF%B8%8F-configuration) • [💖 Sponsor](#-support-the-project)

</div>

---

## 🤖 Available Models

> ⚠️ **Important:** Model availability depends on your Kiro tier (free/paid). The gateway provides access to whatever models are available in your IDE or CLI based on your subscription. The list below shows models commonly available on the **free tier**.

> 🔒 **Claude Opus 4.5** was removed from the free tier on January 17, 2026. It may be available on paid tiers — check your IDE/CLI model list.

| Model | Description | Credits |
|-------|-------------|---------|
| 🚀 **Claude Sonnet 4.5** | Balanced performance. Great for coding, writing, and general-purpose tasks. | ~1.3 |
| ⚡ **Claude Haiku 4.5** | Lightning fast. Perfect for quick responses, simple tasks, and chat. | ~0.4 |
| 📦 **Claude Sonnet 4** | Previous generation. Still powerful and reliable for most use cases. | ~1.3 |
| 📦 **Claude 3.7 Sonnet** | Legacy model. Available for backward compatibility. | ~1.0 |

> 💡 **Smart Model Resolution:** Use any model name format — `claude-sonnet-4-5`, `claude-sonnet-4.5`, or even versioned names like `claude-sonnet-4-5-20250929`. The gateway normalizes them automatically.

---

## ✨ Features

| Feature | Description |
|---------|-------------|
| 🔌 **OpenAI-compatible API** | Works with any OpenAI-compatible tool |
| 🔌 **Anthropic-compatible API** | Native `/v1/messages` endpoint |
| 🌐 **VPN/Proxy Support** | HTTP/SOCKS5 proxy for restricted networks |
| 🧠 **Extended Thinking** | Reasoning support exclusive to this project |
| 👁️ **Vision Support** | Send images to the model |
| 🛠️ **Tool Calling** | Supports function calling |
| 💬 **Full message history** | Passes complete conversation context |
| 📡 **Streaming** | Full SSE streaming support |
| 🔄 **Retry Logic** | Automatic retries on errors (403, 429, 5xx) |
| 📋 **Extended model list** | Including versioned models |
| 🔐 **Smart token management** | Automatic refresh before expiration |

---

## 🚀 Quick Start

**Choose your deployment method:**
- 🐍 **Native Python** — Full control, easy debugging
- 🐳 **Docker** — Isolated environment, easy deployment → [jump to Docker](#-docker-deployment)

### Prerequisites

- Python 3.10+
- One of the following:
  - [Kiro IDE](https://kiro.dev/) with a logged-in account, OR
  - [Kiro CLI](https://kiro.dev/cli/) with AWS SSO (AWS IAM Identity Center, OIDC) — free Builder ID or corporate account

### Installation

```bash
# Clone the repository (requires Git)
git clone https://github.com/Jwadow/kiro-gateway.git
cd kiro-gateway

# Or download ZIP: Code → Download ZIP → extract → open kiro-gateway folder

# Install dependencies
pip install -r requirements.txt

# Configure (see Configuration section)
cp .env.example .env
# Edit .env with your credentials

# Start the server
python main.py

# Or with a custom port (if 8000 is busy)
python main.py --port 9000
```

The server will be available at `http://localhost:8000`.

---

## ⚙️ Configuration

### Option 1: JSON Credentials File (Kiro IDE / Enterprise)

Specify the path to the credentials file. Works with:
- **Kiro IDE** (standard) — for personal accounts
- **Enterprise** — for corporate accounts with SSO

```env
KIRO_CREDS_FILE="~/.aws/sso/cache/kiro-auth-token.json"

# Password to protect YOUR proxy server (use any secure string)
# You'll use this as api_key when connecting to your gateway
PROXY_API_KEY="my-super-secret-password-123"
```

**📄 JSON file format:**

```json
{
  "accessToken": "eyJ...",
  "refreshToken": "eyJ...",
  "expiresAt": "2025-01-12T23:00:00.000Z",
  "profileArn": "arn:aws:codewhisperer:us-east-1:...",
  "region": "us-east-1",
  "clientIdHash": "abc123..."
}
```

> **Note:** If you have two JSON files in `~/.aws/sso/cache/` (e.g., `kiro-auth-token.json` and a file with a hash name), use `kiro-auth-token.json` in `KIRO_CREDS_FILE`. The gateway will automatically load the other file.

### Option 2: Environment Variables (.env file)

Create a `.env` file in the project root:

```env
# Required
REFRESH_TOKEN="your_kiro_refresh_token"

# Password to protect YOUR proxy server
PROXY_API_KEY="my-super-secret-password-123"

# Optional
PROFILE_ARN="arn:aws:codewhisperer:us-east-1:..."
KIRO_REGION="us-east-1"
```

### Option 3: AWS SSO Credentials (kiro-cli / Enterprise)

If you use `kiro-cli` or Kiro IDE with AWS SSO (AWS IAM Identity Center), the gateway will automatically detect and use the appropriate authentication. Works with both free Builder ID accounts and corporate accounts.

```env
KIRO_CREDS_FILE="~/.aws/sso/cache/your-sso-cache-file.json"

# Password to protect YOUR proxy server
PROXY_API_KEY="my-super-secret-password-123"

# Note: PROFILE_ARN is NOT needed for AWS SSO (Builder ID and corporate accounts)
```

**📄 AWS SSO JSON file format:**

AWS SSO credentials files (from `~/.aws/sso/cache/`) contain:

```json
{
  "accessToken": "eyJ...",
  "refreshToken": "eyJ...",
  "expiresAt": "2025-01-12T23:00:00.000Z",
  "region": "us-east-1",
  "clientId": "...",
  "clientSecret": "..."
}
```

**Note:** AWS SSO (Builder ID and corporate accounts) users do NOT need `profileArn`. The gateway will work without it (if specified, it will be ignored).

**🔍 How authentication detection works:**

The gateway automatically detects the authentication type based on the credentials file:

- **Kiro Desktop Auth** (default): Used when `clientId` and `clientSecret` are NOT present
  - Endpoint: `https://prod.{region}.auth.desktop.kiro.dev/refreshToken`

- **AWS SSO (OIDC)**: Used when `clientId` and `clientSecret` ARE present
  - Endpoint: `https://oidc.{region}.amazonaws.com/token`

No additional configuration needed — just point to your credentials file!

### Option 4: kiro-cli SQLite Database

If you use `kiro-cli` and prefer to use its SQLite database directly:

```env
KIRO_CLI_DB_FILE="~/.local/share/kiro-cli/data.sqlite3"

# Password to protect YOUR proxy server
PROXY_API_KEY="my-super-secret-password-123"
```

**📄 Database locations:**

| CLI Tool | Database Path |
|----------|---------------|
| kiro-cli | `~/.local/share/kiro-cli/data.sqlite3` |
| amazon-q-developer-cli | `~/.local/share/amazon-q/data.sqlite3` |

The gateway reads credentials from the `auth_kv` table, which stores:
- `kirocli:odic:token` or `codewhisperer:odic:token` — access token, refresh token, expiration
- `kirocli:odic:device-registration` or `codewhisperer:odic:device-registration` — client ID and secret

Both key formats are supported for compatibility with different kiro-cli versions.

### Getting Credentials

**For Kiro IDE users:** Log in to Kiro IDE and use Option 1 above (JSON credentials file). The credentials file is created automatically after login.

**For Kiro CLI users:** Log in with `kiro-cli login` and use Option 3 or Option 4 above. No manual token extraction needed!

**🔧 Advanced: Manual token extraction**

If you need to manually extract the refresh token (e.g., for debugging), you can intercept Kiro IDE traffic. Look for requests to: `prod.us-east-1.auth.desktop.kiro.dev/refreshToken`

---

## 🐳 Docker Deployment

> **Docker-based deployment.** Prefer native Python? See [Quick Start](#-quick-start) above.

### Quick Start

```bash
# 1. Clone and configure
git clone https://github.com/Jwadow/kiro-gateway.git
cd kiro-gateway
cp .env.example .env
# Edit .env with your credentials

# 2. Run with docker-compose
docker-compose up -d

# 3. Check status
docker-compose logs -f
curl http://localhost:8000/health
```

### Docker Run (Without Compose)

**Using Environment Variables:**

```bash
docker run -d \
  -p 8000:8000 \
  -e PROXY_API_KEY="my-super-secret-password-123" \
  -e REFRESH_TOKEN="your_refresh_token" \
  --name kiro-gateway \
  ghcr.io/jwadow/kiro-gateway:latest
```

**Using Credentials File — Linux/macOS:**

```bash
docker run -d \
  -p 8000:8000 \
  -v ~/.aws/sso/cache:/home/kiro/.aws/sso/cache:ro \
  -e KIRO_CREDS_FILE=/home/kiro/.aws/sso/cache/kiro-auth-token.json \
  -e PROXY_API_KEY="my-super-secret-password-123" \
  --name kiro-gateway \
  ghcr.io/jwadow/kiro-gateway:latest
```

**Using Credentials File — Windows (PowerShell):**

```powershell
docker run -d `
  -p 8000:8000 `
  -v ${HOME}/.aws/sso/cache:/home/kiro/.aws/sso/cache:ro `
  -e KIRO_CREDS_FILE=/home/kiro/.aws/sso/cache/kiro-auth-token.json `
  -e PROXY_API_KEY="my-super-secret-password-123" `
  --name kiro-gateway `
  ghcr.io/jwadow/kiro-gateway:latest
```

**Using .env File:**

```bash
docker run -d -p 8000:8000 --env-file .env --name kiro-gateway ghcr.io/jwadow/kiro-gateway:latest
```

### Docker Compose Configuration

Edit `docker-compose.yml` and uncomment volume mounts for your OS:

```yaml
volumes:
  # Kiro IDE credentials (choose your OS)
  - ~/.aws/sso/cache:/home/kiro/.aws/sso/cache:ro              # Linux/macOS
  # - ${USERPROFILE}/.aws/sso/cache:/home/kiro/.aws/sso/cache:ro  # Windows

  # kiro-cli database (choose your OS)
  - ~/.local/share/kiro-cli:/home/kiro/.local/share/kiro-cli:ro  # Linux/macOS
  # - ${USERPROFILE}/.local/share/kiro-cli:/home/kiro/.local/share/kiro-cli:ro  # Windows

  # Debug logs (optional)
  - ./debug_logs:/app/debug_logs
```

### Management Commands

```bash
docker-compose logs -f      # View logs
docker-compose restart      # Restart
docker-compose down         # Stop
docker-compose pull && docker-compose up -d  # Update
```

**Building from Source:**

```bash
docker build -t kiro-gateway .
docker run -d -p 8000:8000 --env-file .env kiro-gateway
```

---

## 🌐 VPN/Proxy Support

**For users in China, corporate networks, or regions with connectivity issues to AWS services.**

The gateway supports routing all Kiro API requests through a VPN or proxy server. This is essential if you experience connection problems to AWS endpoints or need to use a corporate proxy.

### Configuration

Add to your `.env` file:

```env
# HTTP proxy
VPN_PROXY_URL=http://127.0.0.1:7890

# SOCKS5 proxy
VPN_PROXY_URL=socks5://127.0.0.1:1080

# With authentication (corporate proxies)
VPN_PROXY_URL=http://username:password@proxy.company.com:8080

# Without protocol (defaults to http://)
VPN_PROXY_URL=192.168.1.100:8080
```

### Supported Protocols

- ✅ **HTTP** — Standard proxy protocol
- ✅ **HTTPS** — Secure proxy connections
- ✅ **SOCKS5** — Advanced proxy protocol (common in VPN software)
- ✅ **Authentication** — Username/password embedded in URL

### When You Need This

| Situation | Solution |
|-----------|----------|
| Connection timeouts to AWS | Use VPN/proxy to route traffic |
| Corporate network restrictions | Configure your company's proxy |
| Regional connectivity issues | Use a VPN service with proxy support |
| Privacy requirements | Route through your own proxy server |

### Popular VPN Software with Proxy Support

Most VPN clients provide a local proxy server you can use: **Sing-box**, **Clash** (usually `http://127.0.0.1:7890`), **V2Ray**, **Shadowsocks**, or your corporate VPN (check with IT).

Leave `VPN_PROXY_URL` empty (default) if you don't need proxy support.

---

## 📡 API Reference

### Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/` | GET | Health check |
| `/health` | GET | Detailed health check |
| `/v1/models` | GET | List available models |
| `/v1/chat/completions` | POST | OpenAI Chat Completions API |
| `/v1/messages` | POST | Anthropic Messages API |

### Authentication

| API | Header |
|-----|--------|
| OpenAI | `Authorization: Bearer {PROXY_API_KEY}` |
| Anthropic | `x-api-key: {PROXY_API_KEY}` + `anthropic-version: 2023-06-01` |

---

## 💡 Usage Examples

### OpenAI API

#### 🔹 Simple cURL Request

```bash
curl http://localhost:8000/v1/chat/completions \
  -H "Authorization: Bearer my-super-secret-password-123" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "claude-sonnet-4-5",
    "messages": [{"role": "user", "content": "Hello!"}],
    "stream": true
  }'
```

> **Note:** Replace `my-super-secret-password-123` with the `PROXY_API_KEY` you set in your `.env` file.

#### 🔹 Streaming Request

```bash
curl http://localhost:8000/v1/chat/completions \
  -H "Authorization: Bearer my-super-secret-password-123" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "claude-sonnet-4-5",
    "messages": [
      {"role": "system", "content": "You are a helpful assistant."},
      {"role": "user", "content": "What is 2+2?"}
    ],
    "stream": true
  }'
```

#### 🛠️ With Tool Calling

```bash
curl http://localhost:8000/v1/chat/completions \
  -H "Authorization: Bearer my-super-secret-password-123" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "claude-sonnet-4-5",
    "messages": [{"role": "user", "content": "What is the weather in London?"}],
    "tools": [{
      "type": "function",
      "function": {
        "name": "get_weather",
        "description": "Get weather for a location",
        "parameters": {
          "type": "object",
          "properties": {
            "location": {"type": "string", "description": "City name"}
          },
          "required": ["location"]
        }
      }
    }]
  }'
```

#### 🐍 Python OpenAI SDK

```python
from openai import OpenAI

client = OpenAI(
    base_url="http://localhost:8000/v1",
    api_key="my-super-secret-password-123"  # Your PROXY_API_KEY from .env
)

response = client.chat.completions.create(
    model="claude-sonnet-4-5",
    messages=[
        {"role": "system", "content": "You are a helpful assistant."},
        {"role": "user", "content": "Hello!"}
    ],
    stream=True
)

for chunk in response:
    if chunk.choices[0].delta.content:
        print(chunk.choices[0].delta.content, end="")
```

#### 🦜 LangChain

```python
from langchain_openai import ChatOpenAI

llm = ChatOpenAI(
    base_url="http://localhost:8000/v1",
    api_key="my-super-secret-password-123",  # Your PROXY_API_KEY from .env
    model="claude-sonnet-4-5"
)

response = llm.invoke("Hello, how are you?")
print(response.content)
```

### Anthropic API

#### 🔹 Simple cURL Request

```bash
curl http://localhost:8000/v1/messages \
  -H "x-api-key: my-super-secret-password-123" \
  -H "anthropic-version: 2023-06-01" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "claude-sonnet-4-5",
    "max_tokens": 1024,
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

> **Note:** Anthropic API uses `x-api-key` header instead of `Authorization: Bearer`. Both are supported.

#### 🔹 With System Prompt

```bash
curl http://localhost:8000/v1/messages \
  -H "x-api-key: my-super-secret-password-123" \
  -H "anthropic-version: 2023-06-01" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "claude-sonnet-4-5",
    "max_tokens": 1024,
    "system": "You are a helpful assistant.",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

> **Note:** In the Anthropic API, `system` is a separate field, not a message.

#### 📡 Streaming

```bash
curl http://localhost:8000/v1/messages \
  -H "x-api-key: my-super-secret-password-123" \
  -H "anthropic-version: 2023-06-01" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "claude-sonnet-4-5",
    "max_tokens": 1024,
    "stream": true,
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

#### 🐍 Python Anthropic SDK

```python
import anthropic

client = anthropic.Anthropic(
    api_key="my-super-secret-password-123",  # Your PROXY_API_KEY from .env
    base_url="http://localhost:8000"
)

# Non-streaming
response = client.messages.create(
    model="claude-sonnet-4-5",
    max_tokens=1024,
    messages=[{"role": "user", "content": "Hello!"}]
)
print(response.content[0].text)

# Streaming
with client.messages.stream(
    model="claude-sonnet-4-5",
    max_tokens=1024,
    messages=[{"role": "user", "content": "Hello!"}]
) as stream:
    for text in stream.text_stream:
        print(text, end="", flush=True)
```

---

## 🔧 Debugging

Debug logging is **disabled by default**. To enable, add to your `.env`:

```env
# Debug logging mode:
# - off: disabled (default)
# - errors: save logs only for failed requests (4xx, 5xx) — recommended for troubleshooting
# - all: save logs for every request (overwrites on each request)
DEBUG_MODE=errors
```

### Debug Modes

| Mode | Description | Use Case |
|------|-------------|----------|
| `off` | Disabled (default) | Production |
| `errors` | Save logs only for failed requests (4xx, 5xx) | **Recommended for troubleshooting** |
| `all` | Save logs for every request | Development/debugging |

### Debug Files

When enabled, requests are logged to the `debug_logs/` folder:

| File | Description |
|------|-------------|
| `request_body.json` | Incoming request from client (OpenAI format) |
| `kiro_request_body.json` | Request sent to Kiro API |
| `response_stream_raw.txt` | Raw stream from Kiro |
| `response_stream_modified.txt` | Transformed stream (OpenAI format) |
| `app_logs.txt` | Application logs for the request |
| `error_info.json` | Error details (only on errors) |
