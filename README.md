# Parallax: The Sovereign LLM Proxy

Parallax is a high-integrity, stateful Rust proxy designed to bridge **Cursor** and **OpenRouter/Google Gemini**. It solves common issues with streaming tool calls (like the "Gemini 400" error) and maintains a **Sovereign Conversation State** in a local SQLite database.

## 🌟 Key Features

- **The Gemini Fix**: Automatically captures and re-injects encrypted thought signatures required by Gemini for reliable multi-turn tool calling.
- **Sovereign State**: Keeps conversation history and tool metadata in a local `parallax.db`, preventing state corruption in the model's eyes.
- **Protocol Normalization**: A "Hub and Spoke" architecture that translates between OpenAI, Anthropic, and Gemini schemas mid-flight.
- **Pulse Surgeon**: A real-time SSE stream processor that repairs malformed JSON and intercepts reasoning tokens for on-device persistence.
- **TUI Dashboard**: A built-in terminal UI to monitor requests, costs, and provider health in real-time.

## 🚀 Quick Start

### 1. Installation
Currently, Parallax is built from source. Binaries will be available soon in GitHub Releases.

```bash
git clone https://github.com/youruser/parallax.git
cd parallax
cargo build --release
```

### 2. Configuration
Copy the example environment file and add your OpenRouter API key.

```bash
cp .env.example .env
# Edit .env and set OPENROUTER_API_KEY=sk-or-...
```

### 3. Running
Start the proxy. By default, it listens on `127.0.0.1:8080`.

```bash
./target/release/parallax
```

### 4. Connect Cursor
In Cursor settings (**Models > OpenAI API Key**):
1. Set the **API Key** to any string (e.g., `parallax`).
2. Set the **Base URL** to `http://127.0.0.1:8080/v1`.
3. Enable the models you want to use via OpenRouter (e.g., `google/gemini-2.0-flash-001`).

## ☁️ Advanced Connectivity (HTTPS Tunnels)

Cursor requires an **HTTPS** connection for custom OpenAI endpoints. Since setting up local SSL certificates can be cumbersome, we recommend using **Cloudflare Tunnel (cloudflared)** to handle the HTTPS termination and routing for you.

### Choice 1: Quick Testing Tunnel (Ephemeral)
Use this for a one-off session. 
*Note: These tunnels are often flakey and the URL changes every time you restart it.*

```bash
cloudflared tunnel --url http://127.0.0.1:8080
```
Copy the `.trycloudflare.com` URL provided in the terminal and use it as your Cursor Base URL (append `/v1`).

### Choice 2: Named Tunnel (Recommended)
Persistent tunnels are much more stable and provide a permanent URL.

1. **Create the tunnel**:
   ```bash
   cloudflared tunnel create parallax-proxy
   ```
2. **Configure routing**:
   ```bash
   # Replace <DOMAIN> with a domain you own on Cloudflare
   cloudflared tunnel route dns parallax-proxy parallax.<DOMAIN>
   ```
3. **Run the tunnel**:
   ```bash
   cloudflared tunnel run --url http://127.0.0.1:8080 parallax-proxy
   ```

### ⚠️ Cursor & HTTP/2 Note
Cursor's networking stack sometimes defaults to HTTP/2, which can cause issues with SSE (streaming) over certain tunnel providers or when connecting to local proxies.

If you experience "connection lost" or streaming failures:
- Go to Cursor **Settings > Advanced**.
- Ensure **HTTP/2 Support** is turned **OFF**.
- Restart Cursor.

## 🛡️ Security & Privacy

- **Local First**: All conversation state and logs stay on your machine in `parallax.db`.
- **Safe Defaults**: The server binds to `127.0.0.1` by default. Use `--host 0.0.0.0` only if you trust your network.
- **Redaction**: Logs and traces are redacted by default to prevent leaking API keys or sensitive content.
- **Admin Access**: The `/admin` endpoints are restricted to loopback (localhost) connections only.

## 🛠️ Advanced Usage

```bash
# Change port and database location
./parallax --port 9000 --database my-project.db

# Enable debug capture (Warning: writes unredacted snapshots to disk)
./parallax --enable-debug-capture

# Disable the "Rescue" layer (prevents automatic retries/repairs)
./parallax --disable-rescue
```

## ⚖️ License

Apache License 2.0. See [LICENSE](LICENSE) for details.
