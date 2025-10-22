use crate::google_books_cache::CachedGoogleBooksClient;
use crate::poll_handler;
use crate::types::Error;
use crate::util::log_error_with_source;
use poise::serenity_prelude as serenity;
use sqlx::PgPool;
use std::sync::Arc;
use tokio::time::{self, Duration};

pub fn spawn_selection_poll_watcher(
    http: Arc<serenity::Http>,
    pool: PgPool,
    google_books: CachedGoogleBooksClient,
) {
    tokio::spawn(async move {
        // Check every 60 seconds for expired polls
        let mut interval = time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            if let Err(err) = check_expired_polls(&http, &pool, &google_books).await {
                log_error_with_source("Selection poll watcher error", &err);
            }
        }
    });
}

async fn check_expired_polls(
    http: &Arc<serenity::Http>,
    pool: &PgPool,
    google_books: &CachedGoogleBooksClient,
) -> Result<(), Error> {
    // Find all expired but unprocessed selection polls
    let expired_polls = sqlx::query!(
        r#"
        SELECT message_id, channel_id
        FROM selection_polls
        WHERE NOT processed
          AND expires_at <= NOW()
        "#
    )
    .fetch_all(pool)
    .await?;

    for poll in expired_polls {
        let channel_id = serenity::ChannelId::new(poll.channel_id as u64);
        let message_id = serenity::MessageId::new(poll.message_id as u64);

        // Use the existing poll completion check
        if let Err(err) = poll_handler::check_poll_for_completion_public(
            http,
            pool,
            google_books,
            channel_id,
            message_id,
        )
        .await
        {
            log_error_with_source("Error processing expired poll", &err);
        }
    }

    Ok(())
}
