# Security Policy

## Supported Versions

Only the latest version of Parallax is supported for security updates.

## Reporting a Vulnerability

We take the security of Parallax seriously. If you believe you have found a security vulnerability, please report it to us by opening a private security advisory on GitHub or by contacting the maintainers directly.

**Please do not report security vulnerabilities via public GitHub issues.**

When reporting a vulnerability, please provide:
- A description of the vulnerability.
- A proof-of-concept or steps to reproduce.
- Any potential impact.

We will acknowledge your report within 48 hours and provide a timeline for a fix.

## Privacy Note

Parallax stores conversation state in a local SQLite database (`parallax.db` by default). This data is kept on your machine and is never sent to any server other than the configured LLM provider (e.g., OpenRouter). You are responsible for securing access to this database file.

