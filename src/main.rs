mod access_control;
mod cache_warmer;
mod commands;
mod database_helpers;
mod deadline_handler;
mod google_books;
mod google_books_cache;
mod maturity_check;
mod poll_handler;
mod selection_poll_handler;
mod types;
mod util;

use dotenvy;
use google_books_cache::CachedGoogleBooksClient;
use poise::{CreateReply, FrameworkError, serenity_prelude as serenity};
use sqlx::postgres::PgPoolOptions;
use std::{collections::HashSet, env, sync::Arc};
use tokio::{self, sync::RwLock};
use util::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize environment variables
    dotenvy::dotenv()?;
    let token = env::var("DISCORD_TOKEN").expect("Missing DISCORD_TOKEN in .env");
    let google_api_key = env::var("GOOGLE_BOOKS_API_KEY").ok(); // Optional API key

    // Initialize database connection
    let database_url = env::var("DATABASE_URL").expect("missing DATABASE_URL");
    let database = PgPoolOptions::new()
        .max_connections(10) // was 50
        .min_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(3))
        .idle_timeout(std::time::Duration::from_secs(600)) // was 10s
        .max_lifetime(std::time::Duration::from_secs(1800))
        .connect(&database_url)
        .await?;

    // Initialize Google Books client with caching
    let google_books = CachedGoogleBooksClient::new(google_api_key);
    let google_books_stats = google_books.clone();
    let google_books_warmer = google_books.clone();
    let db_warmer = database.clone();

    // Start a background task to periodically log cache statistics
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(300)); // Every 5 minutes
        loop {
            interval.tick().await;
            google_books_stats.log_cache_info().await;
        }
    });

    // Start cache warming task
    cache_warmer::start_cache_refresh_task(db_warmer, google_books_warmer).await;

    // Setup the poise framework
    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: commands::all_commands(),
            command_check: Some(|ctx| Box::pin(access_control::command_gate(ctx))),
            event_handler: |ctx, event, framework, data| {
                Box::pin(poll_handler::handle_event(ctx, event, framework, data))
            },
            on_error: |error| {
                Box::pin(async move {
                    match error {
                        FrameworkError::MissingUserPermissions {
                            ctx,
                            missing_permissions,
                            ..
                        } if missing_permissions.is_none() => {
                            let response = "I couldn't verify your permissions in this channel. Please run this command somewhere I can read or give me access to view this channel.";

                            if let Err(err) = ctx
                                .send(CreateReply::default().content(response).ephemeral(true))
                                .await
                            {
                                log_error_with_source(
                                    "Error sending missing permissions explanation",
                                    &err,
                                );
                            }
                        }
                        other => {
                            if let Err(err) = poise::builtins::on_error(other).await {
                                log_error_with_source(
                                    "Error while handling framework error with default handler",
                                    &err,
                                );
                            }
                        }
                    }
                })
            },
            ..Default::default()
        })
        .setup(|ctx, _ready, framework| {
            Box::pin(async move {
                poise::builtins::register_globally(ctx, &framework.options().commands).await?;
                let guild_cache = Arc::new(RwLock::new(
                    ctx.cache.guilds().into_iter().collect::<HashSet<_>>(),
                ));

                deadline_handler::spawn_deadline_watcher(
                    ctx.http.clone(),
                    database.clone(),
                    google_books.clone(),
                );

                selection_poll_handler::spawn_selection_poll_watcher(
                    ctx.http.clone(),
                    database.clone(),
                    google_books.clone(),
                );

                Ok(types::Data {
                    database,
                    google_books,
                    guild_cache,
                })
            })
        })
        .build();

    let intents = serenity::GatewayIntents::non_privileged()
        | serenity::GatewayIntents::MESSAGE_CONTENT
        | serenity::GatewayIntents::GUILD_MESSAGE_REACTIONS
        | serenity::GatewayIntents::GUILD_MESSAGE_POLLS;

    let client = serenity::ClientBuilder::new(token, intents)
        .framework(framework)
        .await;

    client.unwrap().start().await.unwrap();
    Ok(())
}
