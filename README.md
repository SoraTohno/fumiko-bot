# Discord Book Club Bot

A Rust-powered Discord bot that organizes server-based book clubs, integrates with the Google Books API, and automates the logistics of reading polls, deadlines, and user progress. The project prioritizes reliability and maintainability: it uses strongly typed command handlers, persistent state in Postgres, and background workers that react to Discord poll events in real time.

## Architecture Overview

### Runtime topology
- **Entry point (`src/main.rs`)** creates the asynchronous runtime, initializes the Postgres pool via `sqlx`, and wires the bot into Serenity + Poise with the required gateway intents for slash commands, message content, and poll events.
- **Shared application state** is stored in `types::Data` and handed to every command or event handler. It exposes the SQL pool, the cached Google Books client, and an in-memory guild cache protected by `RwLock`.
- **Command framework** is organized into feature modules under `src/commands`. Each module registers a vector of Poise commands; `commands::all_commands()` collates them before the framework boots. This keeps slash command definitions colocated with their business logic.

### Background tasks and event hooks
- **Deadline watcher** (`deadline_handler::spawn_deadline_watcher`) runs every ten minutes, finalizes books whose deadlines have passed, and creates rating polls, pinning them when configured.
- **Selection poll watcher** (`selection_poll_handler::spawn_selection_poll_watcher`) monitors open selection polls so that book choices and upcoming deadlines are posted automatically when polls close.
- **Poll event handler** (`poll_handler::handle_event`) receives Discord poll vote additions/removals through Poise's event stream. It stores rating choices, enforces maturity restrictions, and marks polls complete once expired.
- **Cache statistics logger and warmer** (`google_books_cache::CachedGoogleBooksClient` and `cache_warmer::start_cache_refresh_task`) keep frequently accessed Google Books data hot so command handlers stay responsive.

### Google Books integration
- `google_books::GoogleBooksClient` wraps the REST API, while `google_books_cache::CachedGoogleBooksClient` layers on a `moka` cache with separate TTL/size budgets for search and volume lookups.
- Cache keys are deterministic SHA-256 digests of query parameters, and cache hits are tracked via atomic counters that are periodically logged by a spawned task.
- Batch helpers fetch multiple volume IDs concurrently with rate limiting so background workers can hydrate embeds without saturating Google API quotas.

### Persistence model
- The provided [`schema.sql`](schema.sql) file defines all required tables, indexes, and materialized views. The schema covers:
  - Discord entities (`discord_users`, `discord_servers`) and per-server configuration.
  - Book lifecycle tables (`server_book_queue`, `server_current_book`, `server_completed_books`) plus rating poll metadata.
  - User-centric features such as favorites, reading lists, reading progress, and per-server bans for disruptive users.
- SQLx is used in "offline" mode, so statements are checked at compile time when the corresponding database is available.

### Access control and content filtering
- `access_control::command_gate` ensures commands only execute in allowed contexts (e.g., guild-only commands).
- `maturity_check` integrates Discord NSFW flags with server-level maturity settings, preventing adult-only metadata from leaking into restricted channels. Automated deadline completions reuse these checks before posting embeds or polls.

### Command surface area
The bot exposes a wide set of slash commands grouped by audience:
- **Book discovery** (`/info`, `/explore`, `/isbn`, etc.) pull from Google Books and render rich embeds.
- **Server administration** commands (`/config`, `/adminqueue`, `/select`, `/mature`, …) control queue policies, configure announcement targets, and manage selection polls.
- **Server member features** (`/queue`, `/clubread`, `/clubrating`, `/stats`) help members track the active book, rate finished titles, and view queue state.
- **Personal tracking** (`/progress`, `/readinglist`, `/favorite`, `/numberone`) let individuals maintain their own backlog without leaving Discord.
- **Helper utilities** (`/help`, `/deletedata`) provide self-service documentation and GDPR-friendly data wipes.

## Development Workflow

### Prerequisites
- Rust 1.80+ (edition 2024)
- PostgreSQL 14+
- A Discord bot token and application configured for the "Message Content" and "Polls" privileged intents
- (Optional) Google Books API key for higher quota limits

### Environment
Create a `.env` file with at least:
```env
DISCORD_TOKEN=your_bot_token
DATABASE_URL=postgres://user:password@localhost/book_club
# Optional:
GOOGLE_BOOKS_API_KEY=your_api_key
```

### Database setup
1. Create the target database.
2. Apply [`schema.sql`](schema.sql) using `psql` or your migration tool of choice.
3. Ensure the database user can manage extensions required by SQLx (e.g., `pgcrypto` if you add future migrations).

### Running the bot locally
```bash
cargo run
```
The process starts the Serenity gateway, registers slash commands globally, launches background watchers, and begins logging cache statistics every five minutes.

### Tests
Unit tests focus on Google Books caching semantics and can be run with:
```bash
cargo test
```

## Hosting considerations
The bot is a single binary that depends on Discord's gateway connection plus Postgres. In production you should:
- Run it on a long-lived Tokio-friendly host (Linux container or VM).
- Keep `.env` secrets in your orchestrator's secret manager.
- Monitor logs for cache stats and poll watcher errors; structured logging can be added by swapping the `println!` calls for a tracing subscriber.
- Back up the Postgres database regularly—stateful data drives almost every command.

Because slash commands are registered globally, give Discord up to an hour to propagate changes when deploying new builds.
